//! Answering a request from the segments and their index files.
//!
//! Nothing is held between requests. Each one opens what it needs and drops
//! it, which is what lets the process sit idle costing nothing.

use std::path::Path;

use kronika_index::Index;
use kronika_reader::{Reader, ReaderError};
use serde_json::{Value, json};

use crate::route::Window;

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
fn index_of(segment: &kronika_reader::Segment) -> Result<Index, ReaderError> {
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
const fn bounds(window: Window) -> (std::ops::Bound<i64>, std::ops::Bound<i64>) {
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
