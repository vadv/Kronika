use std::collections::{BTreeMap, HashMap};

use kronika_reader::{Reader, Row, Segment, SegmentRef};
use kronika_registry::{contract, logical_section_name};
use serde_json::{Value, json};

use super::super::{ApiError, render::record};
use crate::route::{SeriesRequest, Window};

mod facts;
#[cfg(test)]
mod tests;

use facts::{FIELDS, Previous, Summary, WANTED, integer};

pub(super) const SECTION: &str = "postgresql_summary";
const SOURCES: [&str; 5] = [
    "pg_stat_statements",
    "pg_store_plans",
    "pg_stat_database",
    "pg_stat_user_tables",
    "pg_stat_user_indexes",
];

struct Point(i64, i64, u8, [Option<f64>; 17]);

pub(super) fn validate(request: &SeriesRequest) -> Result<(), ApiError> {
    let parameter = request
        .filters
        .first()
        .map(|filter| filter.column.as_str())
        .or_else(|| request.type_id.is_some().then_some("type_id"))
        .or_else(|| request.group.is_some().then_some("group"))
        .or_else(|| (!request.fields.is_empty()).then_some("field"));
    parameter.map_or_else(
        || Ok(()),
        |parameter| Err(ApiError::BadFilter(parameter.to_owned())),
    )
}

pub(super) fn with_predecessors(
    all: &[SegmentRef],
    mut selected: Vec<SegmentRef>,
) -> Vec<SegmentRef> {
    let selected_len = selected.len();
    selected.sort_by_key(SegmentRef::id);
    for source in SOURCES {
        let first = selected[..selected_len]
            .iter()
            .find(|segment| has_section(segment, source))
            .map(SegmentRef::id);
        if let Some(first) = first
            && let Some(previous) = all
                .iter()
                .filter(|segment| segment.id() < first)
                .max_by_key(|segment| segment.id())
            && has_section(previous, source)
        {
            selected.push(previous.clone());
        }
    }
    selected.dedup_by_key(|segment| segment.id());
    selected
}

fn has_section(segment: &SegmentRef, name: &str) -> bool {
    segment
        .sections()
        .iter()
        .any(|s| logical_section_name(s.type_id) == Some(name))
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
    let mut previous = HashMap::<Vec<i128>, (i64, Previous)>::new();
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
                let Some(timestamp) = integer(row.get("ts")).and_then(|ts| i64::try_from(ts).ok())
                else {
                    return true;
                };
                let datid = match (surface, integer(row.get("datid"))) {
                    (3, Some(0)) => return true,
                    (4 | 5, datid) => datid.unwrap_or(i128::MIN),
                    _ => 0,
                };
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
                previous.insert(key, (timestamp, Previous::new(surface, &row)));
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
        let updates = by_time.entry(timestamp).or_default();
        updates.push((datid, segment, summary));
    }
    let mut latest = BTreeMap::<i128, Summary>::new();
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
    std::iter::once(i128::from(type_id))
        .chain(
            row.contract()
                .identity
                .iter()
                .map(|name| integer(row.get(name)).unwrap_or(i128::MIN)),
        )
        .collect()
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
        if !window.contains(point.1) {
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
            "values": std::iter::once(json!(point.2))
                .chain(point.3.iter().map(|value| json!(value)))
                .collect::<Vec<_>>(),
        }))?) {
            return Ok(());
        }
        ordinal = ordinal.saturating_add(1);
    }
    Ok(())
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
