//! One hour of the timeline: which segments it touches, its health line and
//! its marks, in a single request.
//!
//! The line spans an hour and the marks come from a derived index per segment,
//! so drawing it used to cost one catalog request plus two per segment. Over a
//! link where a round trip costs more than a second, the count of requests is
//! the whole of the wait, and this is one.

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

/// Microseconds in an hour.
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

/// Every hour any stored segment touches, so that a first load needs no
/// separate catalog to know which hours can be picked.
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
    /// An active hour still grows. A settled hour embeds the catalog and the
    /// available-hour list, so it is revalidated rather than immutable.
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
        // A named section is one object's series across the hour, and it needs
        // neither the catalog nor the lanes drawn beside the line.
        if let Some(series) = series {
            for segment in &segments {
                if cancelled() || !emit_series(&reader, segment, &series, emit, cancelled)? {
                    return Ok(());
                }
            }
            return Ok(());
        }
        catalog.stream(emit, cancelled)?;
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
            if !emit_lanes(&reader, segment, emit, cancelled)? {
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

/// The lanes of one segment, each a share of the ceiling the collector lived
/// under. Computed here because the client has no business summing cores.
fn emit_lanes(
    reader: &Reader,
    segment_ref: &SegmentRef,
    emit: &mut impl FnMut(Vec<u8>) -> bool,
    cancelled: &impl Fn() -> bool,
) -> Result<bool, ApiError> {
    let segment = reader.open_segment(segment_ref)?;
    let facts = lanes::facts(&segment)?;
    for point in lanes::collect(&segment, facts.ticks_per_second, facts.cpu_count)? {
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
