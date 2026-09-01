//! Composes one hour of timeline data into one response.

use std::ops::Bound::{Included, Unbounded};
use std::path::{Path, PathBuf};

use kronika_index::{finding_keys, resource_selected, series_keys};
use kronika_reader::{Cell, Listing, Reader, SegmentKind, SegmentRef};
use kronika_registry::{ColumnClass, contract};
use serde_json::json;

use super::catalog::{PreparedCatalog, metric_source_bit, source_bit};
use super::history::stream_plans;
use super::index::stream_series;
use super::query::plans;
use super::render::record;
use super::{ApiError, CachePolicy, ResponseMeta};
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::route::{
    ActiveCursor, DataRequest, HourPart, HourRequest, SegmentRequest, SeriesRequest, Window,
};

mod lanes;
mod postgres_summary;
pub(crate) mod process_summary;

#[cfg(test)]
mod tests;

const SERIES: &str = "health";

const HOUR: i64 = 3_600_000_000;
const ALL_SOURCES: u32 = SOURCE_OS | SOURCE_POSTGRESQL;

#[cfg(test)]
thread_local! {
    static LANE_OPERATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    static SOURCE_PRESENCE_SCANS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[derive(Default)]
struct SourcePresence {
    any: u32,
    metrics: u32,
}

pub(crate) struct PreparedHour {
    root: PathBuf,
    reader: Reader,
    catalog: Option<PreparedCatalog>,
    listed: Vec<SegmentRef>,
    segments: Vec<SegmentRef>,
    window: Window,
    hours: Vec<i64>,
    series: Option<SeriesRequest>,
    part: HourPart,
    etag: Option<String>,
}

pub(super) fn prepare(
    root: &Path,
    request: HourRequest,
    configured_sources: u32,
    synthetic_demo: bool,
) -> Result<PreparedHour, ApiError> {
    let started = std::time::Instant::now();
    let requested = request.window;
    let reader = Reader::open(root)?;
    let discovery = reader.catalog_discovery()?;
    let hours = if request.part == HourPart::Lanes {
        Vec::new()
    } else {
        hours_of_ranges(discovery.ranges())
    };
    let window = requested.from.map_or_else(
        || latest_hour(&hours),
        |from| Window {
            from: Some(from),
            to: Some(requested.to.unwrap_or_else(|| hour_end(from))),
        },
    );
    let stored = if request.series.is_some() {
        discovery.segments(..)?
    } else {
        discovery.segments((
            window.from.map_or(Unbounded, Included),
            window.to.map_or(Unbounded, Included),
        ))?
    };
    let listed = stored.segments;
    let mut segments = listed
        .iter()
        .filter(|segment| overlaps_window(segment.min_ts(), segment.max_ts(), window))
        .cloned()
        .collect::<Vec<_>>();
    segments.sort_by_key(SegmentRef::min_ts);
    if request.part == HourPart::Lanes {
        let expected = request.segments.as_deref().ok_or(ApiError::BadCursor)?;
        pin_segments(&mut segments, expected, request.active)?;
    }
    super::catalog::log_open(segments.len(), &stored.warnings, started);
    let shape = format!(
        "window={window:?};hours={hours:?};series={:?};part={:?};segments={:?};active={:?};sources={configured_sources};demo={synthetic_demo}",
        request.series, request.part, request.segments, request.active,
    );
    let etag = if request.part == HourPart::Base || segments.is_empty() {
        None
    } else if request.series.is_some() && stored.warnings.is_empty() {
        super::weak_etag(
            "hour",
            &shape,
            listed
                .iter()
                .filter(|segment| window.to.is_none_or(|to| segment.min_ts() <= to)),
        )
    } else if stored.warnings.is_empty() {
        super::weak_etag("hour", &shape, &segments)
    } else {
        None
    };
    let catalog = (request.series.is_none() && request.part != HourPart::Lanes).then(|| {
        PreparedCatalog::from_listing(
            Listing {
                segments: segments.clone(),
                warnings: stored.warnings,
            },
            window,
            configured_sources,
            synthetic_demo,
        )
    });
    Ok(PreparedHour {
        root: root.to_path_buf(),
        reader,
        catalog,
        listed,
        segments,
        window,
        hours,
        series: request.series,
        part: request.part,
        etag,
    })
}

fn pin_segments(
    segments: &mut [SegmentRef],
    expected: &[i64],
    cursor: Option<ActiveCursor>,
) -> Result<(), ApiError> {
    if !segments
        .iter()
        .map(SegmentRef::id)
        .eq(expected.iter().copied())
    {
        return Err(ApiError::BadCursor);
    }
    let active = segments
        .iter()
        .position(|segment| segment.kind() == SegmentKind::Active);
    match (active, cursor) {
        (None, None) => Ok(()),
        (Some(_), None) | (None, Some(_)) => Err(ApiError::BadCursor),
        (Some(index), Some(cursor)) => {
            if segments[index].id() != cursor.segment_id {
                return Err(ApiError::BadCursor);
            }
            segments[index] = segments[index]
                .at_active_position(cursor.wal_position)
                .map_err(|_error| ApiError::BadCursor)?;
            Ok(())
        }
    }
}

fn overlaps_window(min_ts: i64, max_ts: i64, window: Window) -> bool {
    window.from.is_none_or(|from| max_ts >= from) && window.to.is_none_or(|to| min_ts <= to)
}

fn hours_of_ranges(ranges: impl IntoIterator<Item = (i64, i64)>) -> Vec<i64> {
    let mut hours = Vec::new();
    for (min_ts, max_ts) in ranges {
        if min_ts > max_ts {
            continue;
        }
        for bucket in min_ts.div_euclid(HOUR)..=max_ts.div_euclid(HOUR) {
            if let Some(hour) = bucket.checked_mul(HOUR) {
                hours.push(hour);
            }
        }
    }
    hours.sort_unstable();
    hours.dedup();
    hours
}

fn latest_hour(hours: &[i64]) -> Window {
    hours.last().map_or_else(Window::default, |from| Window {
        from: Some(*from),
        to: Some(hour_end(*from)),
    })
}

const fn hour_end(from: i64) -> i64 {
    from.saturating_add(HOUR - 1)
}

impl PreparedHour {
    pub(super) fn meta(&self) -> ResponseMeta {
        let settled = self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
        ResponseMeta::ok_with_etag(
            if self.etag.is_some() {
                CachePolicy::Immutable
            } else if settled {
                CachePolicy::Revalidate
            } else {
                CachePolicy::NoStore
            },
            self.etag.clone(),
        )
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        let count = self.segments.len();
        if cancelled() {
            return Ok(());
        }
        if self.part != HourPart::Lanes
            && !emit(record(json!({
                "record": "hour",
                "from": self.window.from.map(|value| value.to_string()),
                "to": self.window.to.map(|value| value.to_string()),
                "available_hours": self.hours.iter().map(ToString::to_string).collect::<Vec<_>>(),
            }))?)
        {
            return Ok(());
        }
        let Self {
            root,
            reader,
            catalog,
            listed,
            segments,
            window,
            series,
            part,
            ..
        } = self;
        if let Some(series) = series {
            if series.section == postgres_summary::SECTION {
                postgres_summary::validate(&series)?;
                let segments = postgres_summary::with_previous(&listed, segments);
                return postgres_summary::stream(&reader, &segments, window, emit, cancelled);
            }
            if series.group.is_some() {
                return super::snapshot::stream_relation_history(
                    &reader, &listed, window, &series, emit, cancelled,
                );
            }
            if series.section == process_summary::SECTION {
                let segments = process_summary::with_predecessors(&listed, segments);
                return process_summary::stream(
                    &reader, &segments, window, &series, emit, cancelled,
                );
            }
            for segment in &segments {
                if cancelled() || !emit_series(&reader, segment, window, &series, emit, cancelled)?
                {
                    return Ok(());
                }
            }
            return Ok(());
        }
        if let Some(catalog) = catalog {
            let presence = source_presence(&reader, &segments, window, cancelled)?;
            catalog
                .with_present_sources(presence.any, presence.metrics)
                .stream(emit, cancelled)?;
        }
        let mut lane_state = lanes::State::default();
        for segment in &segments {
            if cancelled() {
                return Ok(());
            }
            if part != HourPart::Lanes
                && !emit_index(&root, &reader, segment, window, emit, cancelled)?
            {
                return Ok(());
            }
            if part != HourPart::Base
                && !emit_lanes(
                    &reader,
                    segment,
                    window,
                    &mut lane_state,
                    part == HourPart::Lanes,
                    emit,
                    cancelled,
                )?
            {
                return Ok(());
            }
        }
        eprintln!(
            "kronika-web: hour segments={count} elapsed_us={}",
            started.elapsed().as_micros(),
        );
        Ok(())
    }
}

fn emit_index(
    root: &Path,
    reader: &Reader,
    segment: &SegmentRef,
    window: Window,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    let mut keys = series_keys(segment, SERIES);
    keys.extend(series_keys(segment, "pg_stat_activity"));
    keys.extend(finding_keys(segment));
    keys.sort_unstable();
    keys.dedup();
    let resource = resource_selected(root, reader, segment, &keys)?;
    if !emit(record(json!({
        "record": "index",
        "segment": { "id": segment.id().to_string() },
        "logical_name": SERIES,
        "checksum": resource.index.checksum.map(|value| format!("{value:08x}")),
    }))?) {
        return Ok(false);
    }
    stream_series(SERIES, resource, Some(window), emit, cancelled)
}

fn source_presence(
    reader: &Reader,
    segments: &[SegmentRef],
    window: Window,
    cancelled: &impl Fn() -> bool,
) -> Result<SourcePresence, ApiError> {
    let mut presence = SourcePresence::default();
    for segment_ref in segments
        .iter()
        .filter(|segment| window.contains(segment.min_ts()) && window.contains(segment.max_ts()))
    {
        if cancelled() || (presence.any == ALL_SOURCES && presence.metrics == ALL_SOURCES) {
            break;
        }
        for section in segment_ref
            .sections()
            .iter()
            .filter(|section| section.rows > 0)
        {
            if presence.any == ALL_SOURCES && presence.metrics == ALL_SOURCES {
                break;
            }
            let Some(any) = source_bit(section.type_id) else {
                continue;
            };
            if contract(section.type_id).is_some_and(|contract| {
                contract
                    .columns
                    .iter()
                    .any(|column| column.class == ColumnClass::Timestamp)
            }) {
                presence.any |= any;
                if let Some(metrics) = metric_source_bit(section.type_id) {
                    presence.metrics |= metrics;
                }
            }
        }
    }
    for segment_ref in segments
        .iter()
        .filter(|segment| !window.contains(segment.min_ts()) || !window.contains(segment.max_ts()))
    {
        if cancelled() || (presence.any == ALL_SOURCES && presence.metrics == ALL_SOURCES) {
            break;
        }
        let needed = segment_ref.sections().iter().any(|section| {
            let Some(any) = source_bit(section.type_id) else {
                return false;
            };
            presence.any & any == 0
                || metric_source_bit(section.type_id).is_some_and(|bit| presence.metrics & bit == 0)
        });
        if !needed {
            continue;
        }
        let segment = reader.open_segment(segment_ref)?;
        for section in segment_ref.sections() {
            if cancelled() {
                break;
            }
            let type_id = section.type_id;
            let Some(any) = source_bit(type_id) else {
                continue;
            };
            let metrics = metric_source_bit(type_id);
            if presence.any & any != 0 && metrics.is_none_or(|bit| presence.metrics & bit != 0) {
                continue;
            }
            let Some(timestamp) = contract(type_id).and_then(|contract| {
                contract
                    .columns
                    .iter()
                    .find(|column| column.class == ColumnClass::Timestamp)
            }) else {
                continue;
            };
            let mut found = false;
            #[cfg(test)]
            SOURCE_PRESENCE_SCANS.set(SOURCE_PRESENCE_SCANS.get() + 1);
            segment.visit_rows(type_id, &[timestamp.name], 0, usize::MAX, |_ordinal, row| {
                if cancelled() {
                    return false;
                }
                if matches!(row.get(timestamp.name), Some(Cell::Ts(value)) if window.contains(*value))
                {
                    found = true;
                    return false;
                }
                true
            })?;
            if found {
                presence.any |= any;
                if let Some(metrics) = metrics {
                    presence.metrics |= metrics;
                }
            }
        }
    }
    Ok(presence)
}

fn emit_series(
    reader: &Reader,
    segment_ref: &SegmentRef,
    window: Window,
    series: &SeriesRequest,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    let segment = reader.open_segment(segment_ref)?;
    let request = DataRequest {
        segment: SegmentRequest {
            segment_id: segment_ref.id(),
            section: series.section.clone(),
        },
        fields: series.fields.clone(),
        filters: series.filters.clone(),
        type_id: series.type_id,
        after: None,
    };
    match plans(&segment, &request, true) {
        Ok(plans) => {
            if !emit(record(json!({
                "record": "series_segment",
                "segment": { "id": segment_ref.id().to_string() },
            }))?) {
                return Ok(false);
            }
            stream_plans(
                &segment,
                &series.section,
                &plans,
                Some(window),
                emit,
                cancelled,
            )
        }
        Err(ApiError::NoSuchSection) => Ok(true),
        Err(error) => Err(error),
    }
}

fn emit_lanes(
    reader: &Reader,
    segment_ref: &SegmentRef,
    window: Window,
    state: &mut lanes::State,
    include_context: bool,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    #[cfg(test)]
    LANE_OPERATIONS.set(LANE_OPERATIONS.get() + 1);
    let segment = reader.open_segment(segment_ref)?;
    let (points, postgresql_interval_seconds) = lanes::collect(&segment, window, state)?;
    if include_context
        && !emit(record(json!({
            "record": "lane_context",
            "segment_id": segment_ref.id().to_string(),
            "postgresql_interval_seconds": postgresql_interval_seconds.map(|value| value.to_string()),
        }))?)
    {
        return Ok(false);
    }
    for point in points {
        if cancelled()
            || !emit(record(json!({
                "record": "lane",
                "segment_id": segment_ref.id().to_string(),
                "lane": point.key,
                "ts": point.ts.to_string(),
                "value": point.value,
            }))?)
        {
            return Ok(false);
        }
    }
    Ok(true)
}

#[cfg(test)]
pub(crate) fn reset_operations() {
    LANE_OPERATIONS.set(0);
    SOURCE_PRESENCE_SCANS.set(0);
}

#[cfg(test)]
pub(crate) fn operations() -> (usize, usize) {
    (LANE_OPERATIONS.get(), SOURCE_PRESENCE_SCANS.get())
}
