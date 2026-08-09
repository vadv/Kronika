//! Answering a request from the segments and their index files.
//!
//! Nothing is held between requests. Each one opens what it needs and drops
//! it, which is what lets the process sit idle costing nothing.

use std::path::Path;

use kronika_index::Index;
use kronika_reader::{Cell, Dictionary, Reader, ReaderError, Resolved, Row};
use kronika_registry::{Column, ColumnClass, TypeContract, registry};
use serde_json::{Value, json};

use crate::route::Window;

mod rows;
mod series;
mod top;

pub(crate) use rows::rows;
pub(crate) use series::series;
pub(crate) use top::top;

/// Why a request could not be answered.
#[derive(Debug)]
pub(crate) enum ApiError {
    /// No section carries that id.
    NoSuchSection,
    /// The section has no column of that name that holds a number.
    NoSuchColumn,
    /// The data root or a segment could not be read.
    Unreadable(ReaderError),
}

impl From<ReaderError> for ApiError {
    fn from(error: ReaderError) -> Self {
        Self::Unreadable(error)
    }
}

/// The contract of one section, or nothing when no section carries that id.
pub(crate) fn contract_of(type_id: u32) -> Option<&'static TypeContract> {
    registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
}

/// The section and the numeric column a request named.
///
/// # Errors
///
/// Says which of the two nothing answers.
pub(crate) fn column_of(
    section: u32,
    column: &str,
) -> Result<(&'static TypeContract, &'static Column), ApiError> {
    let contract = contract_of(section).ok_or(ApiError::NoSuchSection)?;
    let found = contract
        .columns
        .iter()
        .find(|candidate| {
            candidate.name == column
                && matches!(
                    candidate.class,
                    ColumnClass::Cumulative | ColumnClass::Gauge
                )
        })
        .ok_or(ApiError::NoSuchColumn)?;
    Ok((contract, found))
}

/// Whether a row carries every label the request named.
///
/// A filter on a column the section does not have matches nothing, which is
/// the honest answer to a request for rows that cannot exist.
pub(crate) fn matches(row: &Row, filters: &[(String, String)], dictionary: &Dictionary) -> bool {
    filters.iter().all(|(name, wanted)| {
        row.get(name)
            .is_some_and(|cell| rendered_text(cell, dictionary) == *wanted)
    })
}

/// One cell as JSON. Numbers stay numbers so an answer can be filtered on
/// them; a dictionary id becomes the text it stands for.
pub(crate) fn rendered(cell: &Cell, dictionary: &Dictionary) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::I16(value) => json!(value),
        Cell::I32(value) => json!(value),
        Cell::I64(value) | Cell::Ts(value) => json!(value),
        Cell::U32(value) => json!(value),
        Cell::U64(value) => json!(value),
        Cell::F64(value) => json!(value),
        Cell::Bool(value) => json!(value),
        Cell::ListI32(value) => json!(value),
        Cell::StrId(_id) => json!(rendered_text(cell, dictionary)),
    }
}

/// One cell as the text a filter compares against.
fn rendered_text(cell: &Cell, dictionary: &Dictionary) -> String {
    match cell {
        Cell::Null => String::new(),
        Cell::I16(value) => value.to_string(),
        Cell::I32(value) => value.to_string(),
        Cell::I64(value) | Cell::Ts(value) => value.to_string(),
        Cell::U32(value) => value.to_string(),
        Cell::U64(value) => value.to_string(),
        Cell::F64(value) => value.to_string(),
        Cell::Bool(value) => value.to_string(),
        Cell::ListI32(value) => format!("{value:?}"),
        Cell::StrId(id) => match dictionary.resolve(*id) {
            Some(Resolved::Str(bytes)) => String::from_utf8_lossy(bytes).into_owned(),
            Some(Resolved::Blob(blob)) => String::from_utf8_lossy(blob.stored_bytes).into_owned(),
            None => format!("<str {id}>"),
        },
    }
}

/// The health line over `window`.
///
/// Every segment the window touches contributes the points its index holds.
/// An index that is absent or unreadable is built from the segment and written
/// back, because it is derived from the segment beside it.
///
/// # Errors
///
/// Returns the reader's error when the data root or a segment cannot be read.
pub(crate) fn health(root: &Path, window: Window) -> Result<Value, ReaderError> {
    let reader = Reader::open(root)?;
    let listing = reader.segments(bounds(window))?;
    let mut points = Vec::new();
    for unit in &listing.segments {
        let segment = reader.open_segment(unit)?;
        let index = index_of(&segment)?;
        for point in index.points {
            if within(window, point.ts) {
                points.push(json!([point.ts, point.health]));
            }
        }
    }
    Ok(json!({
        "class": "gauge",
        "unit": "percent",
        "points": points,
    }))
}

/// The index of one segment, read from beside it or built and written there.
///
/// The current segment has no file: its points are computed for this answer
/// and thrown away with the rest of the request.
pub(crate) fn index_of(segment: &kronika_reader::Segment) -> Result<Index, ReaderError> {
    let Some(path) = kronika_index::path_of(segment.path()) else {
        return kronika_index::build(segment, 0);
    };
    if let Ok(index) = kronika_index::read(&path) {
        return Ok(index);
    }
    let built = kronika_index::build(segment, 0)?;
    // A root that cannot be written serves the same answer, one segment slower
    // each time. It is the operator's disk, not the request's problem.
    let _written = kronika_index::write(&path, &built);
    Ok(built)
}

/// The range to ask the reader for.
pub(crate) const fn bounds(window: Window) -> (std::ops::Bound<i64>, std::ops::Bound<i64>) {
    use std::ops::Bound::{Included, Unbounded};
    (
        match window.from {
            Some(from) => Included(from),
            None => Unbounded,
        },
        match window.to {
            Some(to) => Included(to),
            None => Unbounded,
        },
    )
}

/// Whether one point falls inside the window.
///
/// A segment overlaps the window, so the points it holds may run past both
/// ends of it.
const fn within(window: Window, ts: i64) -> bool {
    if let Some(from) = window.from
        && ts < from
    {
        return false;
    }
    if let Some(to) = window.to
        && ts > to
    {
        return false;
    }
    true
}

#[cfg(test)]
mod tests;
