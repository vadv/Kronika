//! Whole-hour `PostgreSQL` population facts.

use std::collections::{BTreeMap, HashMap};

use kronika_reader::{Reader, Row, Segment, SegmentRef};
use kronika_registry::{contract, logical_section_name};
use serde_json::{Value, json};

use super::super::{ApiError, render::record};
use crate::route::{SeriesRequest, Window};

mod facts;
#[cfg(test)]
mod tests;

use facts::{FIELDS, Summary, Values, WANTED, integer};

pub(super) const SECTION: &str = "postgresql_summary";
const SOURCES: [&str; 5] = [
    "pg_stat_statements",
    "pg_store_plans",
    "pg_stat_database",
    "pg_stat_user_tables",
    "pg_stat_user_indexes",
];

struct Point(i64, i64, u8, Values);

pub(super) fn validate(request: &SeriesRequest) -> Result<(), ApiError> {
    if let Some(filter) = request.filters.first() {
        return Err(ApiError::BadFilter(filter.column.clone()));
    }
    let parameter = if request.type_id.is_some() {
        "type_id"
    } else if request.group.is_some() {
        "group"
    } else if !request.fields.is_empty() {
        "field"
    } else {
        return Ok(());
    };
    Err(ApiError::BadFilter(parameter.to_owned()))
}

pub(super) fn with_predecessors(
    all: &[SegmentRef],
    mut selected: Vec<SegmentRef>,
) -> Vec<SegmentRef> {
    for source in SOURCES {
        let first = selected
            .iter()
            .filter(|segment| has_section(segment, source))
            .map(SegmentRef::id)
            .min();
        if let Some(first) = first
            && let Some(previous) = all
                .iter()
                .filter(|segment| segment.id() < first && has_section(segment, source))
                .max_by_key(|segment| segment.id())
        {
            selected.push(previous.clone());
        }
    }
    selected.sort_by_key(SegmentRef::id);
    selected.dedup_by_key(|segment| segment.id());
    selected
}

fn has_section(segment: &SegmentRef, name: &str) -> bool {
    segment
        .sections()
        .iter()
        .any(|section| logical_section_name(section.type_id) == Some(name))
}

pub(super) fn stream(
    reader: &Reader,
    segments: &[SegmentRef],
    window: Window,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let mut opened = Vec::with_capacity(segments.len());
    for segment in segments {
        if cancelled() {
            return Ok(());
        }
        opened.push(reader.open_segment(segment)?);
    }
    let mut points = Vec::new();
    for surface in 1..=5 {
        points.extend(surface_points(&opened, surface, cancelled)?);
    }
    points.sort_by_key(|point| (point.1, point.2));
    emit_points(&points, window, emit, cancelled)
}

fn surface_points(
    segments: &[Segment],
    surface: u8,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Point>, ApiError> {
    let source = SOURCES[usize::from(surface - 1)];
    let mut previous = HashMap::<Vec<i128>, (i64, Row)>::new();
    let mut moments = BTreeMap::<(i64, i128), (i64, Summary)>::new();
    for segment in segments {
        for (type_id, _) in segment.sections() {
            if logical_section_name(type_id) != Some(source) {
                continue;
            }
            let Some(layout) = contract(type_id) else {
                continue;
            };
            let mut columns = layout.identity.to_vec();
            columns.extend(
                WANTED
                    .split_ascii_whitespace()
                    .filter_map(|name| layout.column(name).map(|column| column.name)),
            );
            columns.push("ts");
            columns.sort_unstable();
            columns.dedup();
            segment.visit_rows(type_id, &columns, 0, usize::MAX, |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                let Some(timestamp) = timestamp(&row) else {
                    return true;
                };
                let datid = if surface >= 4 {
                    integer(row.get("datid")).unwrap_or(i128::MIN)
                } else {
                    0
                };
                if surface == 3 && integer(row.get("datid")) == Some(0) {
                    return true;
                }
                let key = identity(type_id, &row);
                let before = previous
                    .get(&key)
                    .filter(|(stored, _)| *stored < timestamp)
                    .map(|(_, row)| row);
                moments
                    .entry((timestamp, datid))
                    .or_insert_with(|| (segment.id(), Summary::new(surface)))
                    .1
                    .add(&row, before);
                previous.insert(key, (timestamp, row));
                true
            })?;
        }
    }
    Ok(fold_moments(surface, moments))
}

fn fold_moments(surface: u8, moments: BTreeMap<(i64, i128), (i64, Summary)>) -> Vec<Point> {
    if surface < 4 {
        return moments
            .into_iter()
            .map(|((timestamp, _), (segment, summary))| {
                Point(segment, timestamp, surface, summary.values(surface))
            })
            .collect();
    }
    let mut by_time = BTreeMap::<i64, Vec<(i128, i64, Summary)>>::new();
    for ((timestamp, datid), (segment, summary)) in moments {
        by_time
            .entry(timestamp)
            .or_default()
            .push((datid, segment, summary));
    }
    let mut latest = HashMap::<i128, Summary>::new();
    let mut points = Vec::with_capacity(by_time.len());
    for (timestamp, updates) in by_time {
        let segment = updates[0].1;
        for (datid, _, summary) in updates {
            latest.insert(datid, summary);
        }
        let mut summary = Summary::new(surface);
        for value in latest.values() {
            summary.merge(value);
        }
        points.push(Point(segment, timestamp, surface, summary.values(surface)));
    }
    points
}

fn identity(type_id: u32, row: &Row) -> Vec<i128> {
    let mut key = vec![i128::from(type_id)];
    key.extend(
        row.contract()
            .identity
            .iter()
            .map(|name| integer(row.get(name)).unwrap_or(i128::MIN)),
    );
    key
}

fn timestamp(row: &Row) -> Option<i64> {
    integer(row.get("ts")).and_then(|value| i64::try_from(value).ok())
}

fn emit_points(
    points: &[Point],
    window: Window,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<(), ApiError> {
    let mut segment = None;
    let mut ordinal = 0_u64;
    for point in points {
        if window.from.is_some_and(|from| point.1 < from)
            || window.to.is_some_and(|to| point.1 > to)
        {
            continue;
        }
        if cancelled() {
            return Ok(());
        }
        if segment != Some(point.0) {
            segment = Some(point.0);
            ordinal = 0;
            if !emit(record(json!({
                "record": "series_segment",
                "segment": { "id": point.0.to_string() },
            }))?)
                || !emit(record(json!({ "record": "layout", "layout": layout() }))?)
            {
                return Ok(());
            }
        }
        if !emit(record(json!({
            "record": "row",
            "type_id": "0",
            "ordinal": ordinal.to_string(),
            "timestamp": point.1.to_string(),
            "identity": [],
            "values": values(point),
        }))?) {
            return Ok(());
        }
        ordinal = ordinal.saturating_add(1);
    }
    Ok(())
}

fn values(point: &Point) -> Vec<Value> {
    let mut values = vec![json!(point.2)];
    values.extend(point.3.iter().map(|value| json!(value)));
    values
}

fn layout() -> Value {
    let columns = std::iter::once(json!({
        "name": "surface", "type": "u8", "class": "gauge", "unit": "count", "nullable": false,
    }))
    .chain(FIELDS.split_ascii_whitespace().map(|name| {
        json!({
            "name": name, "type": "f64", "class": "gauge",
            "unit": if name.ends_with("_pct") { "percent" } else { "number" }, "nullable": true,
        })
    }))
    .collect::<Vec<_>>();
    json!({
        "logical_name": SECTION, "physical_name": "derived_postgresql_summary",
        "type_id": "0", "implementation": "kronika", "identity": [], "columns": columns,
        "provenance": { "inputs": SOURCES },
    })
}
