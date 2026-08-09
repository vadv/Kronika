//! One column of one object over time.
//!
//! This is the one request that opens segments: an index holds one number per
//! object per segment, and a history at full resolution needs the rows
//! themselves. It opens them for one object and one column.

use std::path::Path;

use kronika_reader::{Reader, Row};
use kronika_registry::ColumnClass;
use serde_json::{Value, json};

use super::{ApiError, bounds, column_of, matches};
use crate::route::{SeriesRequest, Window};

/// The history of one object's column over `window`.
///
/// A counter comes back as a rate per second, computed from the point before
/// the window when the segments hold one. Where the difference between two
/// readings is negative the rate is not defined and the value is `null`.
///
/// # Errors
///
/// Returns which part of the request nothing answers, or the reader's error.
pub(crate) fn series(
    root: &Path,
    window: Window,
    request: &SeriesRequest,
) -> Result<Value, ApiError> {
    let (contract, column) = column_of(request.section, &request.column)?;

    let reader = Reader::open(root)?;
    let listing = reader.segments(bounds(window))?;
    let mut readings: Vec<(i64, Option<f64>)> = Vec::new();
    for unit in &listing.segments {
        let segment = reader.open_segment(unit)?;
        let dictionary = segment.dictionary()?;
        for row in segment.rows(request.section)? {
            if !matches(&row, &request.filters, &dictionary) {
                continue;
            }
            let Some(ts) = timestamp(&row) else {
                continue;
            };
            readings.push((ts, number(&row, column.name)));
        }
    }
    readings.sort_by_key(|(ts, _value)| *ts);

    let points = if column.class == ColumnClass::Cumulative {
        rates(&readings)
    } else {
        readings
            .iter()
            .map(|(ts, value)| (*ts, *value))
            .collect::<Vec<_>>()
    };

    let unit = column.unit.map_or("none", kronika_registry::Unit::code);
    Ok(json!({
        "class": column.class.code(),
        "unit": if column.class == ColumnClass::Cumulative {
            format!("{unit}/s")
        } else {
            unit.to_owned()
        },
        "section": contract.name,
        "points": points
            .into_iter()
            .filter(|(ts, _value)| within(window, *ts))
            .map(|(ts, value)| json!([ts, value]))
            .collect::<Vec<_>>(),
    }))
}

/// A counter's readings turned into the rate between each pair.
///
/// The first reading has nothing before it and produces no point at all: a
/// rate needs two readings, and inventing one from a single number would be
/// making it up.
#[allow(
    clippy::cast_precision_loss,
    reason = "an interval of 2^52 microseconds is 142 years"
)]
fn rates(readings: &[(i64, Option<f64>)]) -> Vec<(i64, Option<f64>)> {
    readings
        .windows(2)
        .map(|pair| {
            let (before_ts, before) = pair[0];
            let (ts, now) = pair[1];
            let elapsed = ts.saturating_sub(before_ts);
            let rate = match (before, now) {
                (Some(before), Some(now)) if elapsed > 0 && now >= before => {
                    Some((now - before) / (elapsed as f64 / 1_000_000.0))
                }
                _other => None,
            };
            (ts, rate)
        })
        .collect()
}

/// The row's timestamp.
fn timestamp(row: &Row) -> Option<i64> {
    match row.get("ts") {
        Some(kronika_registry::Cell::Ts(ts)) => Some(*ts),
        _other => None,
    }
}

/// One numeric cell of a row.
#[allow(
    clippy::cast_precision_loss,
    reason = "no counter reaches 2^53 between two snapshots"
)]
fn number(row: &Row, name: &str) -> Option<f64> {
    use kronika_registry::Cell;
    match row.get(name) {
        Some(Cell::I16(value)) => Some(f64::from(*value)),
        Some(Cell::I32(value)) => Some(f64::from(*value)),
        Some(Cell::I64(value) | Cell::Ts(value)) => Some(*value as f64),
        Some(Cell::U32(value)) => Some(f64::from(*value)),
        Some(Cell::U64(value)) => Some(*value as f64),
        Some(Cell::F64(value)) => Some(*value),
        Some(Cell::Bool(value)) => Some(f64::from(u8::from(*value))),
        _other => None,
    }
}

/// Whether one point falls inside the window.
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
