//! Per-layout projection, typed equality filters, and chunk dictionaries.

use std::collections::HashSet;

use kronika_reader::{Cell, Dictionary, Resolved, Row, Segment, SegmentRef, StrId};
use kronika_registry::{ColumnClass, ColumnType, TypeContract, contract};

use super::ApiError;
use crate::route::{DataRequest, Filter};

/// One output field and whether this physical layout carries it.
#[derive(Debug, Clone)]
pub(super) struct OutputField {
    pub(super) name: String,
    pub(super) column: Option<&'static str>,
}

/// One validated physical-layout query.
pub(super) struct Plan {
    pub(super) type_id: u32,
    pub(super) contract: &'static TypeContract,
    pub(super) fields: Vec<OutputField>,
    pub(super) projection: Vec<&'static str>,
    pub(super) timestamp: Option<&'static str>,
    pub(super) start_row: u64,
    filters: Vec<TypedFilter>,
    matches_none: bool,
    pub(super) rows: u64,
}

#[derive(Debug)]
enum TypedFilter {
    Cell {
        column: &'static str,
        wanted: Cell,
    },
    Bytes {
        column: &'static str,
        wanted: Vec<u8>,
        wanted_id: Option<u64>,
    },
}

/// Build compatible per-physical-layout plans without merging identities.
pub(super) fn plans(
    segment: &Segment,
    request: &DataRequest,
    history_coordinates: bool,
) -> Result<Vec<Plan>, ApiError> {
    let layouts: Vec<(u32, kronika_reader::Section)> = segment
        .layouts(&request.segment.section)
        .filter(|(type_id, _section)| request.type_id.is_none_or(|wanted| wanted == *type_id))
        .collect();
    if layouts.is_empty() {
        return Err(ApiError::NoSuchSection);
    }
    let contracts: Vec<&'static TypeContract> = layouts
        .iter()
        .map(|(type_id, _section)| contract(*type_id).ok_or(ApiError::NoSuchSection))
        .collect::<Result<_, _>>()?;
    let output_names = output_names(&contracts, &request.fields)?;
    validate_filter_names(&contracts, &request.filters)?;

    layouts
        .into_iter()
        .zip(contracts)
        .map(|((type_id, section), contract)| {
            let fields: Vec<OutputField> = output_names
                .iter()
                .map(|name| OutputField {
                    name: name.clone(),
                    column: contract.column(name).map(|column| column.name),
                })
                .collect();
            let mut matches_none = false;
            let filters = request
                .filters
                .iter()
                .filter_map(|filter| match typed_filter(contract, filter) {
                    Ok(Some(filter)) => Some(Ok(filter)),
                    Ok(None) => {
                        matches_none = true;
                        None
                    }
                    Err(error) => Some(Err(error)),
                })
                .collect::<Result<Vec<_>, _>>()?;
            let timestamp = contract
                .columns
                .iter()
                .find(|column| column.class == ColumnClass::Timestamp)
                .map(|column| column.name);
            let projection =
                projection(contract, &fields, timestamp, &filters, history_coordinates);
            Ok(Plan {
                type_id,
                contract,
                fields,
                projection,
                timestamp,
                start_row: 0,
                filters,
                matches_none,
                rows: section.rows,
            })
        })
        .collect()
}

pub(super) fn apply_tail(plans: &mut [Plan], prior: Option<&SegmentRef>) -> Result<(), ApiError> {
    let Some(prior) = prior else {
        return Ok(());
    };
    for plan in plans {
        plan.start_row = prior
            .sections()
            .iter()
            .find(|section| section.type_id == plan.type_id)
            .map_or(0, |section| section.rows);
        if plan.start_row > plan.rows {
            return Err(ApiError::BadCursor);
        }
    }
    Ok(())
}

fn projection(
    contract: &'static TypeContract,
    fields: &[OutputField],
    timestamp: Option<&'static str>,
    filters: &[TypedFilter],
    history_coordinates: bool,
) -> Vec<&'static str> {
    let mut projection: Vec<&'static str> = fields
        .iter()
        .filter_map(|field| field.column)
        .chain(filters.iter().map(TypedFilter::column))
        .collect();
    if history_coordinates {
        projection.extend(contract.identity.iter().copied());
        projection.extend(timestamp);
    }
    projection.sort_unstable();
    projection.dedup();
    projection
}

fn output_names(
    contracts: &[&'static TypeContract],
    requested: &[String],
) -> Result<Vec<String>, ApiError> {
    if !requested.is_empty() {
        for name in requested {
            if !contracts
                .iter()
                .any(|contract| contract.column(name).is_some())
            {
                return Err(ApiError::NoSuchColumn(name.clone()));
            }
        }
        return Ok(requested.to_vec());
    }

    let mut names = Vec::new();
    let mut seen = HashSet::new();
    for contract in contracts {
        for column in contract.columns {
            if seen.insert(column.name) {
                names.push(column.name.to_owned());
            }
        }
    }
    Ok(names)
}

fn validate_filter_names(
    contracts: &[&'static TypeContract],
    filters: &[Filter],
) -> Result<(), ApiError> {
    for filter in filters {
        if !contracts
            .iter()
            .any(|contract| contract.column(&filter.column).is_some())
        {
            return Err(ApiError::NoSuchColumn(filter.column.clone()));
        }
    }
    Ok(())
}

/// Resolve only dictionary ids retained in one bounded physical-row chunk.
pub(super) fn chunk_dictionary(
    segment: &Segment,
    rows: &[(u64, Row)],
) -> Result<Dictionary, ApiError> {
    let ids: HashSet<u64> = rows
        .iter()
        .flat_map(|(_ordinal, row)| row.iter())
        .filter_map(|(_name, cell)| match cell {
            Cell::StrId(id) => Some(*id),
            _ => None,
        })
        .collect();
    resolved_dictionary(segment, &ids)
}

pub(super) fn resolved_dictionary(
    segment: &Segment,
    ids: &HashSet<u64>,
) -> Result<Dictionary, ApiError> {
    let dictionary = segment.dictionary_for(ids)?;
    if let Some(unresolved) = ids
        .iter()
        .copied()
        .find(|id| dictionary.resolve(*id).is_none())
    {
        return Err(unresolved_dictionary(unresolved));
    }
    Ok(dictionary)
}

fn unresolved_dictionary(id: u64) -> ApiError {
    ApiError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("unresolved dictionary id {id}"),
    )))
}

impl Plan {
    pub(super) const fn applies(&self) -> bool {
        !self.matches_none
    }

    pub(super) fn matches(&self, row: &Row, dictionary: &Dictionary) -> bool {
        !self.matches_none
            && self
                .filters
                .iter()
                .all(|filter| filter.matches(row, dictionary))
    }

    pub(super) fn selection_dictionary(
        &self,
        segment: &Segment,
        rows: &[(u64, Row)],
    ) -> Result<Dictionary, ApiError> {
        let mut ids = HashSet::new();
        for (_ordinal, row) in rows {
            self.add_selection_ids(row, &mut ids);
        }
        resolved_dictionary(segment, &ids)
    }

    pub(super) fn exact_filter_dictionary(
        &self,
        segment: &Segment,
    ) -> Result<Dictionary, ApiError> {
        let ids = self
            .filters
            .iter()
            .filter_map(|filter| match filter {
                TypedFilter::Bytes { wanted_id, .. } => *wanted_id,
                TypedFilter::Cell { .. } => None,
            })
            .collect();
        segment.dictionary_for(&ids).map_err(ApiError::from)
    }

    pub(super) fn validate_exact_filter_ids(
        &self,
        row: &Row,
        dictionary: &Dictionary,
    ) -> Result<(), ApiError> {
        for filter in &self.filters {
            let TypedFilter::Bytes {
                column, wanted_id, ..
            } = filter
            else {
                continue;
            };
            let Some(Cell::StrId(actual)) = row.get(column) else {
                continue;
            };
            if *wanted_id == Some(*actual) && dictionary.resolve(*actual).is_none() {
                return Err(unresolved_dictionary(*actual));
            }
        }
        Ok(())
    }

    pub(super) fn add_selection_ids(&self, row: &Row, ids: &mut HashSet<u64>) {
        for filter in &self.filters {
            if let TypedFilter::Bytes { column, .. } = filter
                && let Some(Cell::StrId(id)) = row.get(column)
            {
                ids.insert(*id);
            }
        }
    }

    pub(super) fn needs_selection_dictionary(&self) -> bool {
        self.filters
            .iter()
            .any(|filter| matches!(filter, TypedFilter::Bytes { .. }))
    }

    /// Add physical inputs needed for computation without exposing them as
    /// output fields.
    pub(super) fn add_projection_columns(&mut self, names: &[&'static str]) {
        self.projection.extend(names.iter().copied());
        self.projection.sort_unstable();
        self.projection.dedup();
    }
}

impl TypedFilter {
    const fn column(&self) -> &'static str {
        match self {
            Self::Cell { column, .. } | Self::Bytes { column, .. } => column,
        }
    }

    fn matches(&self, row: &Row, dictionary: &Dictionary) -> bool {
        match self {
            Self::Cell { column, wanted } => row
                .get(column)
                .is_some_and(|actual| cells_equal(actual, wanted)),
            Self::Bytes { column, wanted, .. } => {
                let Some(Cell::StrId(id)) = row.get(column) else {
                    return false;
                };
                match dictionary.resolve(*id) {
                    Some(Resolved::Str(bytes)) => bytes == wanted,
                    Some(Resolved::Blob(blob)) => !blob.truncated && blob.stored_bytes == wanted,
                    None => false,
                }
            }
        }
    }
}

fn typed_filter(
    contract: &'static TypeContract,
    filter: &Filter,
) -> Result<Option<TypedFilter>, ApiError> {
    let Some(column) = contract.column(&filter.column) else {
        return Ok(None);
    };
    let bad = || ApiError::BadFilter(filter.column.clone());
    if column.class != ColumnClass::Label {
        return Err(bad());
    }
    let value = filter.value.as_str();
    let wanted = match column.ty {
        ColumnType::I8 => Cell::I16(i16::from(value.parse::<i8>().map_err(|_error| bad())?)),
        ColumnType::I16 => Cell::I16(value.parse().map_err(|_error| bad())?),
        ColumnType::I32 => Cell::I32(value.parse().map_err(|_error| bad())?),
        ColumnType::I64 => Cell::I64(value.parse().map_err(|_error| bad())?),
        ColumnType::U8 => Cell::U32(u32::from(value.parse::<u8>().map_err(|_error| bad())?)),
        ColumnType::U16 => Cell::U32(u32::from(value.parse::<u16>().map_err(|_error| bad())?)),
        ColumnType::U32 => Cell::U32(value.parse().map_err(|_error| bad())?),
        ColumnType::U64 => Cell::U64(value.parse().map_err(|_error| bad())?),
        ColumnType::F32 => {
            let parsed: f32 = value.parse().map_err(|_error| bad())?;
            if !parsed.is_finite() {
                return Err(bad());
            }
            Cell::F64(f64::from(parsed))
        }
        ColumnType::F64 => {
            let parsed: f64 = value.parse().map_err(|_error| bad())?;
            if !parsed.is_finite() {
                return Err(bad());
            }
            Cell::F64(parsed)
        }
        ColumnType::Bool => Cell::Bool(value.parse().map_err(|_error| bad())?),
        ColumnType::Ts => Cell::Ts(value.parse().map_err(|_error| bad())?),
        ColumnType::StrId => {
            let wanted = value.as_bytes().to_vec();
            return Ok(Some(TypedFilter::Bytes {
                column: column.name,
                wanted_id: StrId::of(&wanted).map(StrId::get),
                wanted,
            }));
        }
        ColumnType::ListI32 => {
            let values = if value.is_empty() {
                Vec::new()
            } else {
                value
                    .split(',')
                    .map(|part| part.parse().map_err(|_error| bad()))
                    .collect::<Result<_, _>>()?
            };
            Cell::ListI32(values)
        }
    };
    Ok(Some(TypedFilter::Cell {
        column: column.name,
        wanted,
    }))
}

fn cells_equal(actual: &Cell, wanted: &Cell) -> bool {
    match (actual, wanted) {
        (Cell::F64(actual), Cell::F64(wanted)) => actual.to_bits() == wanted.to_bits(),
        _ => actual == wanted,
    }
}

#[cfg(test)]
mod tests;
