//! The objects of one section over a window, ordered by one column.
//!
//! Splitting the window into columns turns the same answer into a heatmap, so
//! there is one request rather than two with a shared half.
//!
//! Only index files are read. The reduction each of them already holds — a
//! counter as its delta over the segment, a gauge as its last reading — is
//! what makes a day ninety-six files instead of ninety-six segments.

use std::collections::BTreeMap;
use std::path::Path;

use kronika_index::Value as Cell;
use kronika_reader::{Reader, ReaderError};
use kronika_registry::{ColumnClass, TypeContract, registry};
use serde_json::{Value, json};

use super::{bounds, index_of};
use crate::route::{TopRequest, Window};

/// Why a request for the objects of a section could not be answered.
#[derive(Debug)]
pub(crate) enum TopError {
    /// No section carries that id.
    NoSuchSection,
    /// The section has no column of that name that holds a number.
    NoSuchColumn,
    /// The data root or a segment could not be read.
    Unreadable(ReaderError),
}

impl From<ReaderError> for TopError {
    fn from(error: ReaderError) -> Self {
        Self::Unreadable(error)
    }
}

/// What one object accumulated over the window.
#[derive(Debug)]
struct Totals {
    labels: Vec<String>,
    /// One slot per bucket; `None` until a segment lands in it.
    buckets: Vec<Option<f64>>,
}

/// How long the segments that landed in each bucket actually covered.
///
/// A counter is divided by this rather than by the width of the bucket: a
/// segment that covers a third of a column did not spend the whole column
/// producing what it produced.
type Observed = Vec<i64>;

/// The objects of `request.section`, ordered by `request.column`.
///
/// # Errors
///
/// Returns which part of the request nothing answers, or the reader's error.
pub(crate) fn top(root: &Path, window: Window, request: &TopRequest) -> Result<Value, TopError> {
    let contract = contract_of(request.section).ok_or(TopError::NoSuchSection)?;
    let (at, class, unit) =
        numeric_column(contract, &request.column).ok_or(TopError::NoSuchColumn)?;
    let identity = identity_positions(contract);

    let reader = Reader::open(root)?;
    let listing = reader.segments(bounds(window))?;
    let span = span_of(window, &listing);
    let buckets = request.buckets.max(1);

    let mut totals: BTreeMap<Vec<String>, Totals> = BTreeMap::new();
    let mut observed: Observed = vec![0; buckets];
    for unit_ref in &listing.segments {
        let bucket = bucket_of(span, buckets, unit_ref.max_ts());
        if let Some(covered) = observed.get_mut(bucket) {
            *covered =
                covered.saturating_add(unit_ref.max_ts().saturating_sub(unit_ref.min_ts()).max(1));
        }
        let segment = reader.open_segment(unit_ref)?;
        let index = index_of(&segment)?;
        let Some(section) = index
            .objects
            .iter()
            .find(|section| section.type_id == request.section)
        else {
            continue;
        };
        for object in &section.objects {
            let Some(number) = number(object.values.get(at)) else {
                continue;
            };
            let key: Vec<String> = identity
                .iter()
                .filter_map(|position| object.labels.get(*position).cloned())
                .collect();
            let slot = totals.entry(key).or_insert_with(|| Totals {
                labels: object.labels.clone(),
                buckets: vec![None; buckets],
            });
            slot.labels.clone_from(&object.labels);
            let Some(cell) = slot.buckets.get_mut(bucket) else {
                continue;
            };
            *cell = Some(match class {
                // A counter's deltas add up across the segments of a bucket.
                ColumnClass::Cumulative => cell.unwrap_or(0.0) + number,
                // A gauge's last reading wins, and segments arrive oldest first.
                _other => number,
            });
        }
    }

    let mut rows: Vec<(f64, Value)> = totals
        .into_values()
        .map(|object| {
            let values: Vec<Option<f64>> = object
                .buckets
                .iter()
                .enumerate()
                .map(|(at, cell)| {
                    cell.map(|number| scale(class, number, seconds_of(&observed, at)))
                })
                .collect();
            let order = values.iter().flatten().sum::<f64>();
            (order, json!({ "labels": object.labels, "values": values }))
        })
        .collect();
    rows.sort_by(|left, right| right.0.total_cmp(&left.0));
    rows.truncate(request.limit);

    Ok(json!({
        "class": class.code(),
        "unit": unit,
        "section": contract.name,
        "buckets": bucket_edges(span, buckets),
        "rows": rows.into_iter().map(|(_order, row)| row).collect::<Vec<_>>(),
    }))
}

/// The contract of one section, or nothing when no section carries that id.
fn contract_of(type_id: u32) -> Option<&'static TypeContract> {
    registry()
        .iter()
        .find(|contract| contract.type_id.get() == type_id)
}

/// Where `name` sits among the section's numbers, its class, and its unit.
///
/// The position is the one the index wrote: numeric columns in contract order.
fn numeric_column(
    contract: &'static TypeContract,
    name: &str,
) -> Option<(usize, ColumnClass, String)> {
    contract
        .columns
        .iter()
        .filter(|column| matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge))
        .enumerate()
        .find(|(_at, column)| column.name == name)
        .map(|(at, column)| {
            let unit = column.unit.map_or("none", kronika_registry::Unit::code);
            let unit = if column.class == ColumnClass::Cumulative {
                format!("{unit}/s")
            } else {
                unit.to_owned()
            };
            (at, column.class, unit)
        })
}

/// Where the identity columns sit among the labels the index wrote.
fn identity_positions(contract: &'static TypeContract) -> Vec<usize> {
    let labels: Vec<&str> = contract
        .columns
        .iter()
        .filter(|column| column.class == ColumnClass::Label)
        .map(|column| column.name)
        .collect();
    contract
        .identity
        .iter()
        .filter_map(|name| labels.iter().position(|label| label == name))
        .collect()
}

/// The window the answer covers: what the request asked for, or what the
/// segments turned out to hold.
fn span_of(window: Window, listing: &kronika_reader::Listing) -> (i64, i64) {
    let first = window.from.unwrap_or_else(|| {
        listing
            .segments
            .iter()
            .map(kronika_reader::SegmentRef::min_ts)
            .min()
            .unwrap_or(0)
    });
    let last = window.to.unwrap_or_else(|| {
        listing
            .segments
            .iter()
            .map(kronika_reader::SegmentRef::max_ts)
            .max()
            .unwrap_or(0)
    });
    (first, last.max(first))
}

/// Which column a segment ending at `ts` belongs to.
fn bucket_of(span: (i64, i64), buckets: usize, ts: i64) -> usize {
    let (first, last) = span;
    let width = i128::from(last.saturating_sub(first)).max(1);
    let offset = i128::from(ts.saturating_sub(first)).clamp(0, width);
    let count = i128::try_from(buckets).unwrap_or(1).max(1);
    let scaled = offset * count / width;
    usize::try_from(scaled)
        .unwrap_or(0)
        .min(buckets.saturating_sub(1))
}

/// The start of every bucket, so the interface knows what it is drawing.
fn bucket_edges(span: (i64, i64), buckets: usize) -> Vec<i64> {
    let (first, last) = span;
    let width = i128::from(last.saturating_sub(first)).max(1);
    let count = i128::try_from(buckets).unwrap_or(1).max(1);
    (0..buckets)
        .map(|at| {
            let offset = width * i128::try_from(at).unwrap_or(0) / count;
            first.saturating_add(i64::try_from(offset).unwrap_or(i64::MAX))
        })
        .collect()
}

/// How many seconds of history one bucket actually holds.
///
/// A bucket no segment ended in holds none, and the counter that goes with it
/// is absent rather than divided by nothing.
#[allow(
    clippy::cast_precision_loss,
    reason = "a bucket of 2^52 microseconds is 142 years"
)]
fn seconds_of(observed: &Observed, bucket: usize) -> f64 {
    observed.get(bucket).copied().unwrap_or(0) as f64 / 1_000_000.0
}

/// A counter's total over a bucket becomes a rate; a gauge is what it was.
fn scale(class: ColumnClass, number: f64, seconds: f64) -> f64 {
    if class == ColumnClass::Cumulative && seconds > 0.0 {
        number / seconds
    } else {
        number
    }
}

/// One index value as a number, or nothing where the segment had none.
///
/// A counter's delta over one segment does not reach the width of a mantissa,
/// and neither does a gauge the kernel prints.
#[allow(
    clippy::cast_precision_loss,
    reason = "no counter reaches 2^53 over one segment"
)]
const fn number(value: Option<&Cell>) -> Option<f64> {
    match value {
        Some(Cell::Int(number)) => Some(*number as f64),
        Some(Cell::Float(number)) => Some(*number),
        Some(Cell::Null) | None => None,
    }
}

#[cfg(test)]
mod tests;
