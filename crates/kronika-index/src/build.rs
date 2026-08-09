//! Streaming construction of health and physical-layout index blocks.

use std::collections::{BTreeMap, HashSet};

use kronika_reader::{Cell, Dictionary, ReaderError, Resolved, Row, Segment};
use kronika_registry::instance_metadata::Environment;
use kronika_registry::{ColumnClass, TypeContract, contract};

use crate::file::Index;
use crate::health::{Stall, health};
use crate::summary::{IdentityValue, Number, ObjectSummary, Observation, Sample, SectionSummary};

/// Reserved generic index layout for the derived health gauge.
pub const DERIVED_HEALTH_TYPE_ID: u32 = 0;
/// `type_id` of `instance_metadata`.
pub const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_001;
/// `type_id` of `os_psi`.
pub const OS_PSI_TYPE_ID: u32 = 1_107_001;

const CPU: u32 = 0;
const MEMORY: u32 = 1;
const IO: u32 = 2;
const HOST: u32 = 0;
const POD: u32 = 1;
const CONTAINER: u32 = 3;

/// Why a segment could not be reduced into an exact index.
#[derive(Debug)]
pub enum BuildError {
    /// The production reader rejected a selected body.
    Reader(ReaderError),
    /// A declared identity refers to a dictionary value absent from the
    /// captured segment.
    UnresolvedIdentity {
        /// Physical layout.
        type_id: u32,
        /// Identity column.
        column: &'static str,
        /// Raw unresolved dictionary id.
        id: u64,
    },
}

impl std::fmt::Display for BuildError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Reader(error) => error.fmt(f),
            Self::UnresolvedIdentity {
                type_id,
                column,
                id,
            } => write!(
                f,
                "section {type_id} identity {column:?} has unresolved dictionary id {id}"
            ),
        }
    }
}

impl std::error::Error for BuildError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Reader(error) => Some(error),
            Self::UnresolvedIdentity { .. } => None,
        }
    }
}

impl From<ReaderError> for BuildError {
    fn from(error: ReaderError) -> Self {
        Self::Reader(error)
    }
}

#[derive(Debug)]
struct Seen {
    identity: Vec<Cell>,
    observations: Vec<Accum>,
}

#[derive(Debug, Clone, Copy, Default)]
struct Accum {
    count: u64,
    first: Option<Sample>,
    last: Option<Sample>,
    cumulative: bool,
}

impl Accum {
    fn new(class: ColumnClass) -> Self {
        Self {
            cumulative: matches!(class, ColumnClass::Cumulative),
            ..Self::default()
        }
    }

    fn observe(&mut self, ts: i64, cell: Option<&Cell>) {
        let Some(value) = cell.and_then(number) else {
            return;
        };
        let sample = Sample { ts, value };
        if self.first.is_none_or(|first| ts < first.ts) {
            self.first = Some(sample);
        }
        if self.last.is_none_or(|last| ts >= last.ts) {
            self.last = Some(sample);
        }
        self.count = self.count.saturating_add(1);
    }

    fn finish(self) -> Observation {
        let observed_us = match (self.first, self.last) {
            (Some(first), Some(last)) => {
                u64::try_from(last.ts.saturating_sub(first.ts)).unwrap_or(0)
            }
            _ => 0,
        };
        let nonnegative_delta = if self.cumulative && self.count >= 2 {
            self.first
                .zip(self.last)
                .and_then(|(first, last)| difference(first.value, last.value))
        } else {
            None
        };
        Observation {
            count: self.count,
            first: self.first,
            last: self.last,
            nonnegative_delta,
            observed_us,
        }
    }
}

/// Build every physical layout block in a segment.
///
/// # Errors
///
/// Returns a production-reader failure or an unresolved declared identity.
pub fn build(segment: &Segment, sources: u32) -> Result<Index, BuildError> {
    let mut type_ids = vec![DERIVED_HEALTH_TYPE_ID];
    type_ids.extend(
        segment
            .type_ids()
            .filter(|type_id| contract(*type_id).is_some()),
    );
    build_selected(segment, sources, &type_ids)
}

/// Build only selected physical layouts for a captured active response.
///
/// # Errors
///
/// Returns a production-reader failure or an unresolved declared identity.
pub fn build_selected(
    segment: &Segment,
    sources: u32,
    type_ids: &[u32],
) -> Result<Index, BuildError> {
    let mut selected: Vec<u32> = type_ids.to_vec();
    selected.sort_unstable();
    selected.dedup();
    let mut sections = Vec::with_capacity(selected.len());
    for type_id in selected {
        if type_id == DERIVED_HEALTH_TYPE_ID {
            sections.push(health_summary(segment)?);
        } else if segment.rows_of(type_id).is_some() {
            sections.push(section_summary(segment, type_id)?);
        }
    }
    Ok(Index { sources, sections })
}

fn section_summary(segment: &Segment, type_id: u32) -> Result<SectionSummary, BuildError> {
    let contract = contract(type_id).ok_or_else(|| {
        BuildError::Reader(ReaderError::Section {
            type_id,
            source: kronika_registry::CodecError::UnknownType { type_id },
        })
    })?;
    let timestamp = contract
        .columns
        .iter()
        .find(|column| column.class == ColumnClass::Timestamp)
        .map(|column| column.name);
    let numeric: Vec<(&'static str, ColumnClass)> = contract
        .columns
        .iter()
        .filter(|column| matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge))
        .map(|column| (column.name, column.class))
        .collect();
    let mut projection: Vec<&str> = timestamp.into_iter().collect();
    projection.extend(contract.identity.iter().copied());
    projection.extend(numeric.iter().map(|(name, _class)| *name));
    projection.sort_unstable();
    projection.dedup();

    let mut seen: BTreeMap<Vec<u8>, Seen> = BTreeMap::new();
    segment.visit_rows(type_id, &projection, 0, usize::MAX, |_ordinal, row| {
        let Some(ts) = timestamp.and_then(|name| timestamp_of(&row, name)) else {
            return true;
        };
        let identity: Vec<Cell> = contract
            .identity
            .iter()
            .map(|name| row.get(name).cloned().unwrap_or(Cell::Null))
            .collect();
        let key = identity_key(&identity);
        let object = seen.entry(key).or_insert_with(|| Seen {
            identity,
            observations: numeric
                .iter()
                .map(|(_name, class)| Accum::new(*class))
                .collect(),
        });
        for (observation, (name, _class)) in object.observations.iter_mut().zip(&numeric) {
            observation.observe(ts, row.get(name));
        }
        true
    })?;

    let ids: HashSet<u64> = seen
        .values()
        .flat_map(|object| object.identity.iter())
        .filter_map(|cell| match cell {
            Cell::StrId(id) => Some(*id),
            _ => None,
        })
        .collect();
    let dictionary = segment.dictionary_for(&ids)?;
    let mut objects = Vec::with_capacity(seen.len());
    for object in seen.into_values() {
        let identity = resolve_identity(type_id, contract, &object.identity, &dictionary)?;
        objects.push(ObjectSummary {
            identity,
            observations: object.observations.into_iter().map(Accum::finish).collect(),
        });
    }
    Ok(SectionSummary { type_id, objects })
}

fn resolve_identity(
    type_id: u32,
    contract: &'static TypeContract,
    cells: &[Cell],
    dictionary: &Dictionary,
) -> Result<Vec<IdentityValue>, BuildError> {
    contract
        .identity
        .iter()
        .zip(cells)
        .map(|(&column, cell)| match cell {
            Cell::I16(value) => Ok(IdentityValue::I16(*value)),
            Cell::I32(value) => Ok(IdentityValue::I32(*value)),
            Cell::I64(value) => Ok(IdentityValue::I64(*value)),
            Cell::U32(value) => Ok(IdentityValue::U32(*value)),
            Cell::U64(value) => Ok(IdentityValue::U64(*value)),
            Cell::F64(value) => Ok(IdentityValue::F64(*value)),
            Cell::Bool(value) => Ok(IdentityValue::Bool(*value)),
            Cell::Ts(value) => Ok(IdentityValue::Ts(*value)),
            Cell::StrId(id) => match dictionary.resolve(*id) {
                Some(Resolved::Str(bytes)) => Ok(IdentityValue::Text(bytes.to_vec())),
                Some(Resolved::Blob(blob)) => Ok(IdentityValue::from_blob(blob)),
                None => Err(BuildError::UnresolvedIdentity {
                    type_id,
                    column,
                    id: *id,
                }),
            },
            Cell::ListI32(values) => Ok(IdentityValue::ListI32(values.clone())),
            Cell::Null => Ok(IdentityValue::Null),
        })
        .collect()
}

fn identity_key(identity: &[Cell]) -> Vec<u8> {
    let mut key = Vec::with_capacity(identity.len().saturating_mul(12));
    for cell in identity {
        match cell {
            Cell::I16(value) => {
                key.push(1);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::I32(value) => {
                key.push(2);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::I64(value) => {
                key.push(3);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::U32(value) => {
                key.push(4);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::U64(value) => {
                key.push(5);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::F64(value) => {
                key.push(6);
                key.extend_from_slice(&value.to_bits().to_le_bytes());
            }
            Cell::Bool(value) => {
                key.push(7);
                key.push(u8::from(*value));
            }
            Cell::Ts(value) => {
                key.push(8);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::StrId(value) => {
                key.push(9);
                key.extend_from_slice(&value.to_le_bytes());
            }
            Cell::ListI32(values) => {
                key.push(10);
                key.extend_from_slice(&values.len().to_le_bytes());
                for value in values {
                    key.extend_from_slice(&value.to_le_bytes());
                }
            }
            Cell::Null => key.push(0),
        }
    }
    key
}

fn timestamp_of(row: &Row, name: &str) -> Option<i64> {
    match row.get(name) {
        Some(Cell::Ts(value)) => Some(*value),
        _ => None,
    }
}

const fn number(cell: &Cell) -> Option<Number> {
    match cell {
        Cell::I16(value) => Some(Number::I16(*value)),
        Cell::I32(value) => Some(Number::I32(*value)),
        Cell::I64(value) | Cell::Ts(value) => Some(Number::I64(*value)),
        Cell::U32(value) => Some(Number::U32(*value)),
        Cell::U64(value) => Some(Number::U64(*value)),
        Cell::F64(value) => Some(Number::F64(*value)),
        _ => None,
    }
}

fn difference(before: Number, after: Number) -> Option<Number> {
    match (before, after) {
        (Number::I16(before), Number::I16(after)) if after >= before => {
            after.checked_sub(before).map(Number::I16)
        }
        (Number::I32(before), Number::I32(after)) if after >= before => {
            after.checked_sub(before).map(Number::I32)
        }
        (Number::I64(before), Number::I64(after)) if after >= before => {
            after.checked_sub(before).map(Number::I64)
        }
        (Number::U32(before), Number::U32(after)) => after.checked_sub(before).map(Number::U32),
        (Number::U64(before), Number::U64(after)) => after.checked_sub(before).map(Number::U64),
        (Number::F64(before), Number::F64(after)) if after >= before => {
            let difference = after - before;
            difference.is_finite().then_some(Number::F64(difference))
        }
        _ => None,
    }
}

#[derive(Debug, Default)]
struct PartialStall {
    cpu: Option<i64>,
    memory: Option<i64>,
    io: Option<i64>,
}

fn health_summary(segment: &Segment) -> Result<SectionSummary, ReaderError> {
    if segment.rows_of(INSTANCE_METADATA_TYPE_ID).is_none()
        || segment.rows_of(OS_PSI_TYPE_ID).is_none()
    {
        return Ok(empty_health());
    }
    let mut environment = None;
    segment.visit_rows(
        INSTANCE_METADATA_TYPE_ID,
        &["environment"],
        0,
        usize::MAX,
        |_ordinal, row| {
            if let Some(Cell::U32(value)) = row.get("environment") {
                environment = Some(*value);
            }
            true
        },
    )?;
    let mut snapshots: BTreeMap<i64, PartialStall> = BTreeMap::new();
    segment.visit_rows(
        OS_PSI_TYPE_ID,
        &["ts", "resource", "some_total", "scope"],
        0,
        usize::MAX,
        |_ordinal, row| {
            let (
                Some(Cell::Ts(ts)),
                Some(Cell::U32(resource)),
                Some(Cell::I64(total)),
                Some(Cell::U32(scope)),
            ) = (
                row.get("ts"),
                row.get("resource"),
                row.get("some_total"),
                row.get("scope"),
            )
            else {
                return true;
            };
            let matching_scope = match environment {
                Some(value) if value == u32::from(Environment::Machine.as_u8()) => *scope == HOST,
                Some(value) if value == u32::from(Environment::Container.as_u8()) => {
                    matches!(*scope, POD | CONTAINER)
                }
                _ => false,
            };
            if matching_scope {
                let snapshot = snapshots.entry(*ts).or_default();
                match *resource {
                    CPU => snapshot.cpu = Some(*total),
                    MEMORY => snapshot.memory = Some(*total),
                    IO => snapshot.io = Some(*total),
                    _ => {}
                }
            }
            true
        },
    )?;

    let mut summary = Accum::new(ColumnClass::Gauge);
    let mut previous = None;
    for (ts, snapshot) in snapshots {
        let current = match (snapshot.cpu, snapshot.memory, snapshot.io) {
            (Some(cpu), Some(memory), Some(io)) => Some(Stall { cpu, memory, io }),
            _ => None,
        };
        if let Some(value) = previous.and_then(|(before_ts, before)| {
            current.and_then(|after| health(before, before_ts, after, ts))
        }) {
            summary.observe(ts, Some(&Cell::U32(u32::from(value))));
        }
        previous = current.map(|stall| (ts, stall));
    }
    Ok(SectionSummary {
        type_id: DERIVED_HEALTH_TYPE_ID,
        objects: vec![ObjectSummary {
            identity: Vec::new(),
            observations: vec![summary.finish()],
        }],
    })
}

fn empty_health() -> SectionSummary {
    SectionSummary {
        type_id: DERIVED_HEALTH_TYPE_ID,
        objects: vec![ObjectSummary {
            identity: Vec::new(),
            observations: vec![Accum::new(ColumnClass::Gauge).finish()],
        }],
    }
}

#[cfg(test)]
mod tests;
