//! The rows of one section over an interval, as they were recorded.
//!
//! Nothing is reduced here. This is what an operator reads when the question
//! is not "how much" but "what was going on": the sessions, the log lines, the
//! settings a server was running under.

use std::path::Path;

use kronika_reader::Reader;
use kronika_registry::ColumnClass;
use serde_json::{Map, Value, json};

use super::{ApiError, bounds, contract_of, matches, rendered};
use crate::route::{RowsRequest, Window};

/// The rows of `request.section` over `window`.
///
/// The answer carries a header of its columns, because its rows are
/// heterogeneous and one envelope cannot state one class and one unit for all
/// of them. The header describes this answer, not the catalogue.
///
/// # Errors
///
/// Returns [`ApiError::NoSuchSection`] or the reader's error.
pub(crate) fn rows(root: &Path, window: Window, request: &RowsRequest) -> Result<Value, ApiError> {
    let contract = contract_of(request.section).ok_or(ApiError::NoSuchSection)?;

    let reader = Reader::open(root)?;
    let listing = reader.segments(bounds(window))?;
    let mut answered: Vec<Value> = Vec::new();
    for unit in &listing.segments {
        if answered.len() >= request.limit {
            break;
        }
        let segment = reader.open_segment(unit)?;
        let dictionary = segment.dictionary()?;
        for row in segment.rows(request.section)? {
            if answered.len() >= request.limit {
                break;
            }
            if !matches(&row, &request.filters, &dictionary) {
                continue;
            }
            if !within(window, &row) {
                continue;
            }
            answered.push(Value::Array(
                row.iter()
                    .map(|(_name, cell)| rendered(cell, &dictionary))
                    .collect(),
            ));
        }
    }

    let mut columns = Map::new();
    for column in contract.columns {
        let unit = column.unit.map_or("none", kronika_registry::Unit::code);
        columns.insert(column.name.to_owned(), json!([column.class.code(), unit]));
    }
    Ok(json!({
        "section": contract.name,
        "columns": columns,
        "order": contract.columns.iter().map(|column| column.name).collect::<Vec<_>>(),
        "rows": answered,
    }))
}

/// Whether a row's timestamp falls inside the window.
///
/// A section with no timestamp column is answered whole: it has no time of its
/// own to compare against.
fn within(window: Window, row: &kronika_reader::Row) -> bool {
    let Some(name) = row
        .contract()
        .columns
        .iter()
        .find(|column| column.class == ColumnClass::Timestamp)
        .map(|column| column.name)
    else {
        return true;
    };
    let Some(kronika_registry::Cell::Ts(ts)) = row.get(name) else {
        return true;
    };
    if let Some(from) = window.from
        && *ts < from
    {
        return false;
    }
    if let Some(to) = window.to
        && *ts > to
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests;
