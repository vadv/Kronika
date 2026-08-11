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
use super::index::stream_series;
use super::render::record;
use super::{ApiError, CachePolicy, ResponseMeta};
use crate::route::Window;

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
}

pub(super) fn prepare(
    root: &Path,
    requested: Window,
    configured_sources: u32,
) -> Result<PreparedHour, ApiError> {
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
    /// An hour holding the active segment still grows, and one already closed
    /// gains no segment, but nothing in the request says which it is until the
    /// segments are known.
    pub(super) fn meta(&self) -> ResponseMeta {
        let settled = self
            .segments
            .iter()
            .all(|segment| segment.kind() == SegmentKind::Finished);
        ResponseMeta::ok(if settled {
            CachePolicy::Immutable
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
        self.catalog.stream(emit, cancelled)?;
        for segment in &self.segments {
            if cancelled() {
                return Ok(());
            }
            if series_keys(segment, SERIES).is_empty() {
                continue;
            }
            let resource = resource(&self.root, &self.reader, segment, SERIES)?;
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
        }
        eprintln!(
            "kronika-web: hour segments={count} elapsed_us={}",
            started.elapsed().as_micros(),
        );
        Ok(())
    }
}
