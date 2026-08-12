//! Composes one hour of timeline data into one response.

use std::ops::Bound::{Included, Unbounded};
use std::path::{Path, PathBuf};

use kronika_index::{resource, series_keys};
use kronika_reader::{Reader, SegmentKind, SegmentRef};
use serde_json::json;

use super::catalog::PreparedCatalog;
use super::history::stream_plans;
use super::index::stream_series;
use super::query::plans;
use super::render::record;
use super::{ApiError, CachePolicy, ResponseMeta};
use crate::route::{DataRequest, HourRequest, SegmentRequest, SeriesRequest, Window};

mod lanes;

const SERIES: &str = "health";

const HOUR: i64 = 3_600_000_000;

pub(crate) struct PreparedHour {
    root: PathBuf,
    reader: Reader,
    catalog: PreparedCatalog,
    segments: Vec<SegmentRef>,
    window: Window,
    hours: Vec<i64>,
    series: Option<SeriesRequest>,
}

pub(super) fn prepare(
    root: &Path,
    request: HourRequest,
    configured_sources: u32,
) -> Result<PreparedHour, ApiError> {
    let requested = request.window;
    let reader = Reader::open(root)?;
    let stored = reader.catalog_segments(..)?;
    let hours = hours_of(&stored.segments);
    let window = requested.from.map_or_else(
        || latest_hour(&hours),
        |from| Window {
            from: Some(from),
            to: requested.to.or(Some(from + HOUR - 1)),
        },
    );
    let catalog = super::catalog::prepare(root, window, configured_sources)?;
    let listing = reader.catalog_segments((
        window.from.map_or(Unbounded, Included),
        window.to.map_or(Unbounded, Included),
    ))?;
    let mut segments = listing.segments;
    segments.sort_by_key(SegmentRef::min_ts);
    Ok(PreparedHour {
        root: root.to_path_buf(),
        reader,
        catalog,
        segments,
        window,
        hours,
        series: request.series,
    })
}

fn hours_of(segments: &[SegmentRef]) -> Vec<i64> {
    let mut hours: Vec<i64> = segments
        .iter()
        .flat_map(|segment| [floor_hour(segment.min_ts()), floor_hour(segment.max_ts())])
        .collect();
    hours.sort_unstable();
    hours.dedup();
    hours
}

fn latest_hour(hours: &[i64]) -> Window {
    hours.last().map_or_else(Window::default, |from| Window {
        from: Some(*from),
        to: Some(from + HOUR - 1),
    })
}

const fn floor_hour(timestamp: i64) -> i64 {
    timestamp - timestamp.rem_euclid(HOUR)
}

impl PreparedHour {
    pub(super) fn meta(&self) -> ResponseMeta {
        let settled = self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
        ResponseMeta::ok(if settled {
            CachePolicy::Revalidate
        } else {
            CachePolicy::NoStore
        })
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Vec<u8>) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let started = std::time::Instant::now();
        let count = self.segments.len();
        if cancelled()
            || !emit(record(json!({
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
            segments,
            series,
            ..
        } = self;
        if let Some(series) = series {
            for segment in &segments {
                if cancelled() || !emit_series(&reader, segment, &series, emit, cancelled)? {
                    return Ok(());
                }
            }
            return Ok(());
        }
        catalog.stream(emit, cancelled)?;
        let mut lane_state = lanes::State::default();
        for segment in &segments {
            if cancelled() {
                return Ok(());
            }
            if series_keys(segment, SERIES).is_empty() {
                continue;
            }
            let resource = resource(&root, &reader, segment, SERIES)?;
            if !emit(record(json!({
                "record": "index",
                "segment": { "id": segment.id().to_string() },
                "logical_name": SERIES,
                "checksum": resource.index.checksum.map(|value| format!("{value:08x}")),
            }))?) {
                return Ok(());
            }
            if !stream_series(SERIES, resource, emit, cancelled)? {
                return Ok(());
            }
            if !emit_lanes(&reader, segment, &mut lane_state, emit, cancelled)? {
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

fn emit_series(
    reader: &Reader,
    segment_ref: &SegmentRef,
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
            stream_plans(&segment, &series.section, &plans, emit, cancelled)
        }
        Err(ApiError::NoSuchSection) => Ok(true),
        Err(error) => Err(error),
    }
}

fn emit_lanes(
    reader: &Reader,
    segment_ref: &SegmentRef,
    state: &mut lanes::State,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    let segment = reader.open_segment(segment_ref)?;
    let facts = lanes::facts(&segment)?;
    for point in lanes::collect(&segment, facts.ticks_per_second, facts.cpu_count, state)? {
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
