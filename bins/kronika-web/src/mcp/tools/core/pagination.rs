use std::borrow::Borrow;
use std::cell::Cell;
use std::collections::{BTreeMap, HashSet};
use std::ops::Bound::{Included, Unbounded};
use std::path::Path;

use base64::Engine as _;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use kronika_reader::{Reader, SegmentKind};
use serde_json::{Value, json};
use sha2::{Digest as _, Sha256};

use super::{Failure, HOUR_US, MAX_SEGMENTS, api_failure};
use crate::api;
use crate::mcp::State;
use crate::route::{HourRequest, Route, Window};

const CURSOR_VERSION: u8 = 1;
const DIGEST_BYTES: usize = 16;
const CURSOR_BODY_BYTES: usize = 2 + 3 * DIGEST_BYTES + size_of::<u64>();
const CURSOR_CHECK_BYTES: usize = 8;
const CURSOR_BYTES: usize = CURSOR_BODY_BYTES + CURSOR_CHECK_BYTES;
const MAX_METADATA_RECORDS: usize = 512;
const MAX_WARNING_RECORDS: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
enum Surface {
    Hours = 1,
    Findings = 2,
    Timeline = 3,
}

impl Surface {
    const fn parse(value: u8) -> Option<Self> {
        match value {
            1 => Some(Self::Hours),
            2 => Some(Self::Findings),
            3 => Some(Self::Timeline),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Fingerprint([u8; DIGEST_BYTES]);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PositionKey(Fingerprint);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
    position: PositionKey,
    offset: u64,
}

#[derive(Debug, Clone, Copy)]
struct PageStart {
    offset: u64,
    expected_position: Option<PositionKey>,
}

#[derive(Debug)]
pub(super) struct PageInfo {
    pub(super) returned: usize,
    pub(super) truncated: bool,
    pub(super) next_cursor: Option<String>,
    pub(super) stop_reason: &'static str,
}

#[derive(Debug)]
pub(super) struct HoursPage {
    pub(super) hours: Vec<Value>,
    pub(super) page: PageInfo,
}

#[derive(Debug)]
pub(super) struct FindingsPage {
    pub(super) findings: Vec<Value>,
    pub(super) semantics: Vec<Value>,
    pub(super) warnings: Vec<Value>,
    pub(super) page: PageInfo,
}

#[derive(Debug)]
pub(super) struct TimelinePage {
    pub(super) lanes: Vec<Value>,
    pub(super) markers: Vec<Value>,
    pub(super) semantics: Vec<Value>,
    pub(super) warnings: Vec<Value>,
    pub(super) page: PageInfo,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct HourRange {
    first: i64,
    last: i64,
}

#[derive(Debug)]
struct HourCatalog {
    ranges: Vec<HourRange>,
    source: Fingerprint,
}

#[derive(Debug)]
struct SourceSnapshot {
    binding: Fingerprint,
    active_positions: BTreeMap<i64, Option<u64>>,
}

#[derive(Debug)]
struct IndexedMetadata {
    current: Option<IndexAnchor>,
    semantics: Vec<Value>,
    warnings: Vec<Value>,
    layouts: HashSet<(String, String)>,
    source_truncated: bool,
}

#[derive(Debug, Clone)]
struct IndexAnchor {
    segment_id: String,
    active_position: Option<u64>,
    checksum: Value,
}

#[derive(Debug)]
enum TimelineItem {
    Lane(Value),
    Marker(Value),
}

impl TimelineItem {
    const fn record(&self) -> &Value {
        match self {
            Self::Lane(record) | Self::Marker(record) => record,
        }
    }
}

#[derive(Debug)]
struct Accumulator<T> {
    start: PageStart,
    limit: usize,
    seen: u64,
    observed_position: Option<PositionKey>,
    items: Vec<Positioned<T>>,
    has_more: bool,
}

#[derive(Debug)]
struct Accumulated<T> {
    items: Vec<Positioned<T>>,
    has_more: bool,
}

#[derive(Debug)]
struct Positioned<T> {
    item: T,
    position: PositionKey,
}

struct Fingerprinter(Sha256);

impl Fingerprinter {
    fn new(domain: &[u8]) -> Self {
        let mut hasher = Self(Sha256::new());
        hasher.part(b"domain", domain);
        hasher
    }

    fn part(&mut self, tag: &[u8], bytes: &[u8]) {
        self.0
            .update(u64::try_from(tag.len()).unwrap_or(u64::MAX).to_le_bytes());
        self.0.update(tag);
        self.0
            .update(u64::try_from(bytes.len()).unwrap_or(u64::MAX).to_le_bytes());
        self.0.update(bytes);
    }

    fn optional_i64(&mut self, tag: &[u8], value: Option<i64>) {
        self.part(
            tag,
            &value.map_or([0; 9], |value| {
                let mut encoded = [0; 9];
                encoded[0] = 1;
                encoded[1..].copy_from_slice(&value.to_le_bytes());
                encoded
            }),
        );
    }

    fn finish(self) -> Fingerprint {
        let digest = self.0.finalize();
        let mut shortened = [0; DIGEST_BYTES];
        shortened.copy_from_slice(&digest[..DIGEST_BYTES]);
        Fingerprint(shortened)
    }
}

impl Cursor {
    fn parse(raw: &str) -> Result<Self, Failure> {
        let decoded = URL_SAFE_NO_PAD.decode(raw).map_err(|_error| bad_cursor())?;
        let bytes: [u8; CURSOR_BYTES] = decoded.try_into().map_err(|_bytes| bad_cursor())?;
        let (body, offered_check) = bytes.split_at(CURSOR_BODY_BYTES);
        if cursor_check(body).as_slice() != offered_check {
            return Err(bad_cursor());
        }
        if body[0] != CURSOR_VERSION {
            return Err(bad_cursor());
        }
        let surface = Surface::parse(body[1]).ok_or_else(bad_cursor)?;
        let query = fingerprint_at(body, 2)?;
        let source = fingerprint_at(body, 2 + DIGEST_BYTES)?;
        let position = PositionKey(fingerprint_at(body, 2 + 2 * DIGEST_BYTES)?);
        let offset_at = 2 + 3 * DIGEST_BYTES;
        let offset = u64::from_le_bytes(
            body[offset_at..offset_at + size_of::<u64>()]
                .try_into()
                .map_err(|_bytes| bad_cursor())?,
        );
        if offset == 0 {
            return Err(bad_cursor());
        }
        Ok(Self {
            surface,
            query,
            source,
            position,
            offset,
        })
    }

    fn encode(self) -> String {
        let mut body = Vec::with_capacity(CURSOR_BYTES);
        body.push(CURSOR_VERSION);
        body.push(self.surface as u8);
        body.extend_from_slice(&self.query.0);
        body.extend_from_slice(&self.source.0);
        body.extend_from_slice(&self.position.0.0);
        body.extend_from_slice(&self.offset.to_le_bytes());
        let check = cursor_check(&body);
        body.extend_from_slice(&check);
        URL_SAFE_NO_PAD.encode(body)
    }
}

impl<T> Accumulator<T> {
    fn new(start: PageStart, limit: usize) -> Self {
        Self {
            start,
            limit,
            seen: 0,
            observed_position: None,
            items: Vec::with_capacity(limit),
            has_more: false,
        }
    }

    fn push(&mut self, item: T, position: PositionKey) -> Result<(), Failure> {
        let index = self.seen;
        self.seen = self.seen.checked_add(1).ok_or_else(|| {
            Failure::bounded(
                "position_limit_exceeded",
                "The eligible record position exceeds the continuation limit.",
            )
        })?;
        if self.start.offset != 0 && self.seen == self.start.offset {
            self.observed_position = Some(position);
        }
        if index < self.start.offset {
            return Ok(());
        }
        if self.items.len() < self.limit {
            self.items.push(Positioned { item, position });
        } else {
            self.has_more = true;
        }
        Ok(())
    }

    fn finish(self) -> Result<Accumulated<T>, Failure> {
        if self.start.expected_position != self.observed_position {
            return Err(bad_cursor());
        }
        if self.start.offset != 0 && self.items.is_empty() {
            return Err(bad_cursor());
        }
        Ok(Accumulated {
            items: self.items,
            has_more: self.has_more,
        })
    }
}

impl IndexedMetadata {
    fn new() -> Self {
        Self {
            current: None,
            semantics: Vec::new(),
            warnings: Vec::new(),
            layouts: HashSet::new(),
            source_truncated: false,
        }
    }

    fn set_index(&mut self, record: &Value, source: &SourceSnapshot) -> Result<(), Failure> {
        let segment_id = record
            .pointer("/segment/id")
            .and_then(Value::as_str)
            .ok_or_else(index_locator_failure)?;
        let parsed = segment_id
            .parse::<i64>()
            .map_err(|_error| index_locator_failure())?;
        let active_position = source
            .active_positions
            .get(&parsed)
            .copied()
            .ok_or_else(source_changed)?;
        self.current = Some(IndexAnchor {
            segment_id: segment_id.to_owned(),
            active_position,
            checksum: record.get("checksum").cloned().unwrap_or(Value::Null),
        });
        Ok(())
    }

    fn attach_index(&self, record: &mut Value) -> Result<(), Failure> {
        let anchor = self.current.as_ref().ok_or_else(index_locator_failure)?;
        let object = record.as_object_mut().ok_or_else(index_locator_failure)?;
        object.insert("source".to_owned(), json!("kronika_index"));
        object.insert("segment_id".to_owned(), json!(anchor.segment_id.clone()));
        object.insert(
            "active_wal_position".to_owned(),
            anchor
                .active_position
                .map_or(Value::Null, |position| json!(position.to_string())),
        );
        object.insert("index_checksum".to_owned(), anchor.checksum.clone());
        stringify_integer(object, "field_ordinal");
        stringify_integer(object, "row_ordinal");
        Ok(())
    }

    fn attach_lane(record: &mut Value, source: &SourceSnapshot) -> Result<(), Failure> {
        let segment_id = record
            .get("segment_id")
            .and_then(Value::as_str)
            .ok_or_else(index_locator_failure)?;
        let parsed = segment_id
            .parse::<i64>()
            .map_err(|_error| index_locator_failure())?;
        let active_position = source
            .active_positions
            .get(&parsed)
            .copied()
            .ok_or_else(source_changed)?;
        let object = record.as_object_mut().ok_or_else(index_locator_failure)?;
        object.insert("source".to_owned(), json!("kronika_derived"));
        object.insert(
            "active_wal_position".to_owned(),
            active_position.map_or(Value::Null, |position| json!(position.to_string())),
        );
        Ok(())
    }

    fn push_layout(&mut self, mut record: Value) -> Result<(), Failure> {
        let logical_name = record
            .pointer("/layout/logical_name")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let type_id = record
            .pointer("/layout/type_id")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        if self.layouts.insert((logical_name, type_id)) {
            self.attach_index(&mut record)?;
            self.push_semantic(record)?;
        }
        Ok(())
    }

    fn push_finding_summary(&mut self, mut record: Value) -> Result<(), Failure> {
        self.source_truncated |= record.get("truncated").and_then(Value::as_bool) == Some(true);
        self.attach_index(&mut record)?;
        let object = record.as_object_mut().ok_or_else(index_locator_failure)?;
        stringify_integer(object, "total_hits");
        self.push_semantic(record)
    }

    fn push_semantic(&mut self, record: Value) -> Result<(), Failure> {
        if self.semantics.len() >= MAX_METADATA_RECORDS {
            return Err(Failure::bounded(
                "metadata_limit_exceeded",
                "The indexed semantic metadata exceeds its bounded result limit.",
            ));
        }
        self.semantics.push(record);
        Ok(())
    }

    fn push_warning(&mut self, record: Value) -> Result<(), Failure> {
        if self.warnings.len() >= MAX_WARNING_RECORDS {
            return Err(Failure::bounded(
                "warning_limit_exceeded",
                "Store warnings exceed their bounded result limit.",
            ));
        }
        self.warnings.push(record);
        Ok(())
    }
}

pub(super) fn hours(
    root: &Path,
    window: Window,
    cursor: Option<&str>,
    limit: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<HoursPage, Failure> {
    let query = hours_query(window);
    let catalog = hour_catalog(root, window, cancelled)?;
    let start = page_start(cursor, Surface::Hours, query, catalog.source)?;
    let total = hour_count(&catalog.ranges, cancelled)?;
    if start.offset >= total && start.offset != 0 {
        return Err(bad_cursor());
    }
    if let Some(expected) = start.expected_position {
        let previous =
            hour_at(&catalog.ranges, start.offset - 1, cancelled)?.ok_or_else(bad_cursor)?;
        if hour_position(previous) != expected {
            return Err(bad_cursor());
        }
    }
    let mut hours = Vec::with_capacity(limit);
    let mut index = start.offset;
    while hours.len() < limit {
        check_cancelled(cancelled)?;
        let Some(bucket) = hour_at(&catalog.ranges, index, cancelled)? else {
            break;
        };
        let start_us = bucket.saturating_mul(HOUR_US);
        hours.push(json!({
            "start_us": start_us.to_string(),
            "end_us": start_us.saturating_add(HOUR_US - 1).to_string(),
        }));
        index = index.checked_add(1).ok_or_else(|| {
            Failure::bounded(
                "position_limit_exceeded",
                "The hour position exceeds the continuation limit.",
            )
        })?;
    }
    let has_more = index < total;
    let next_cursor = if has_more {
        let last = hour_at(&catalog.ranges, index - 1, cancelled)?.ok_or_else(bad_cursor)?;
        Some(
            Cursor {
                surface: Surface::Hours,
                query,
                source: catalog.source,
                position: hour_position(last),
                offset: index,
            }
            .encode(),
        )
    } else {
        None
    };
    let after = hour_catalog(root, window, cancelled)?;
    if after.source != catalog.source {
        return Err(source_changed());
    }
    Ok(HoursPage {
        page: page_info(hours.len(), has_more, false, next_cursor),
        hours,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "Findings binds two filters plus cursor, output budget, and cancellation"
)]
pub(super) fn findings(
    state: &State,
    window: Window,
    surface: Option<&str>,
    kind: Option<&str>,
    cursor: Option<&str>,
    limit: usize,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<FindingsPage, Failure> {
    let query = findings_query(window, surface, kind);
    let source = index_source(&state.data_root, window, cancelled)?;
    let start = page_start(cursor, Surface::Findings, query, source.binding)?;
    let mut page = Accumulator::new(start, limit);
    let mut metadata = IndexedMetadata::new();
    stream_hour(state, window, cancelled, |mut record| {
        match record.get("record").and_then(Value::as_str) {
            Some("index") => metadata.set_index(&record, &source),
            Some("layout") => metadata.push_layout(record),
            Some("findings") => {
                if matches_surface(&record, surface) {
                    metadata.push_finding_summary(record)
                } else {
                    Ok(())
                }
            }
            Some("finding") => {
                if !matches_surface(&record, surface) || !matches_kind(&record, kind) {
                    return Ok(());
                }
                metadata.attach_index(&mut record)?;
                let position = value_position(Surface::Findings, &record)?;
                page.push(record, position)
            }
            Some("warning") => metadata.push_warning(record),
            _ => Ok(()),
        }
    })?;
    ensure_source_unchanged(&state.data_root, window, source.binding, cancelled)?;
    let accumulated = page.finish()?;
    let (returned, semantics, page) = fit_findings_page(
        window,
        Surface::Findings,
        query,
        source.binding,
        start.offset,
        accumulated,
        &metadata.semantics,
        &metadata.warnings,
        metadata.source_truncated,
        budget,
    )?;
    Ok(FindingsPage {
        page,
        findings: returned,
        semantics,
        warnings: metadata.warnings,
    })
}

pub(super) fn timeline(
    state: &State,
    window: Window,
    lanes: &[String],
    cursor: Option<&str>,
    limit: usize,
    budget: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<TimelinePage, Failure> {
    let mut normalized_lanes = lanes.to_vec();
    normalized_lanes.sort_unstable();
    normalized_lanes.dedup();
    let wanted = normalized_lanes
        .iter()
        .map(String::as_str)
        .collect::<HashSet<_>>();
    let query = timeline_query(window, &normalized_lanes);
    let source = index_source(&state.data_root, window, cancelled)?;
    let start = page_start(cursor, Surface::Timeline, query, source.binding)?;
    let mut page = Accumulator::new(start, limit);
    let mut metadata = IndexedMetadata::new();
    stream_hour(state, window, cancelled, |mut record| {
        match record.get("record").and_then(Value::as_str) {
            Some("index") => metadata.set_index(&record, &source),
            Some("layout") => metadata.push_layout(record),
            Some("findings") => metadata.push_finding_summary(record),
            Some("point") => {
                if !matches_lane(&record, &wanted) {
                    return Ok(());
                }
                metadata.attach_index(&mut record)?;
                let position = value_position(Surface::Timeline, &record)?;
                page.push(TimelineItem::Lane(record), position)
            }
            Some("lane") => {
                if !matches_lane(&record, &wanted) {
                    return Ok(());
                }
                IndexedMetadata::attach_lane(&mut record, &source)?;
                let position = value_position(Surface::Timeline, &record)?;
                page.push(TimelineItem::Lane(record), position)
            }
            Some("finding") => {
                metadata.attach_index(&mut record)?;
                let position = value_position(Surface::Timeline, &record)?;
                page.push(TimelineItem::Marker(record), position)
            }
            Some("warning") => metadata.push_warning(record),
            _ => Ok(()),
        }
    })?;
    ensure_source_unchanged(&state.data_root, window, source.binding, cancelled)?;
    let accumulated = page.finish()?;
    let (items, semantics, page) = fit_timeline_page(
        window,
        Surface::Timeline,
        query,
        source.binding,
        start.offset,
        accumulated,
        &metadata.semantics,
        &metadata.warnings,
        metadata.source_truncated,
        budget,
    )?;
    let mut timeline_lanes = Vec::new();
    let mut markers = Vec::new();
    for item in items {
        match item {
            TimelineItem::Lane(record) => timeline_lanes.push(record),
            TimelineItem::Marker(record) => markers.push(record),
        }
    }
    Ok(TimelinePage {
        page,
        lanes: timeline_lanes,
        markers,
        semantics,
        warnings: metadata.warnings,
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the exact envelope fitter must retain every cursor and budget binding"
)]
fn fit_findings_page(
    window: Window,
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
    offset: u64,
    accumulated: Accumulated<Value>,
    semantics: &[Value],
    warnings: &[Value],
    source_truncated: bool,
    budget: usize,
) -> Result<(Vec<Value>, Vec<Value>, PageInfo), Failure> {
    let (retained, page) = fit_page_count(
        window,
        surface,
        query,
        source,
        offset,
        &accumulated,
        warnings,
        source_truncated,
        budget,
        "Finding",
        |count| {
            let definitions = page_semantics(
                semantics,
                accumulated.items[..count]
                    .iter()
                    .map(|positioned| &positioned.item),
            )?;
            Ok(json!({
                "findings": accumulated.items[..count]
                    .iter()
                    .map(|positioned| positioned.item.clone())
                    .collect::<Vec<_>>(),
                "semantics": definitions,
            }))
        },
    )?;
    let returned = accumulated
        .items
        .into_iter()
        .take(retained)
        .map(|positioned| positioned.item)
        .collect::<Vec<_>>();
    let definitions = page_semantics(semantics, &returned)?;
    Ok((returned, definitions, page))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the exact envelope fitter must retain every cursor and budget binding"
)]
fn fit_timeline_page(
    window: Window,
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
    offset: u64,
    accumulated: Accumulated<TimelineItem>,
    semantics: &[Value],
    warnings: &[Value],
    source_truncated: bool,
    budget: usize,
) -> Result<(Vec<TimelineItem>, Vec<Value>, PageInfo), Failure> {
    let (retained, page) = fit_page_count(
        window,
        surface,
        query,
        source,
        offset,
        &accumulated,
        warnings,
        source_truncated,
        budget,
        "Timeline item",
        |count| {
            let definitions = page_semantics(
                semantics,
                accumulated.items[..count]
                    .iter()
                    .map(|positioned| positioned.item.record()),
            )?;
            Ok(timeline_data(&accumulated.items[..count], &definitions))
        },
    )?;
    let returned = accumulated
        .items
        .into_iter()
        .take(retained)
        .map(|positioned| positioned.item)
        .collect::<Vec<_>>();
    let definitions = page_semantics(semantics, returned.iter().map(TimelineItem::record))?;
    Ok((returned, definitions, page))
}

#[allow(
    clippy::too_many_arguments,
    reason = "the generic fitter receives the complete cursor and envelope binding"
)]
fn fit_page_count<T>(
    window: Window,
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
    offset: u64,
    accumulated: &Accumulated<T>,
    warnings: &[Value],
    source_truncated: bool,
    budget: usize,
    item_name: &'static str,
    data: impl Fn(usize) -> Result<Value, Failure>,
) -> Result<(usize, PageInfo), Failure> {
    let anchor = super::anchor(None, window.from, None, None);
    let metadata_page = page_info(0, false, source_truncated, None);
    let metadata_data = data(0)?;
    if envelope_len(&anchor, &metadata_data, &metadata_page, warnings) > budget {
        return Err(result_too_large(
            "The fixed semantic metadata and warnings exceed data_budget_bytes.",
        ));
    }
    if accumulated.items.is_empty() {
        return Ok((0, metadata_page));
    }

    let evaluate = |count| -> Result<(PageInfo, usize), Failure> {
        let page = candidate_page_info(
            surface,
            query,
            source,
            offset,
            accumulated,
            count,
            source_truncated,
        )?;
        let encoded = envelope_len(&anchor, &data(count)?, &page, warnings);
        Ok((page, encoded))
    };

    let available = accumulated.items.len();
    let (complete_page, complete_bytes) = evaluate(available)?;
    if complete_bytes <= budget {
        return Ok((available, complete_page));
    }

    let (first_page, first_bytes) = evaluate(1)?;
    if first_bytes > budget {
        return Err(result_too_large(format!(
            "The first selected {item_name} exceeds data_budget_bytes."
        )));
    }

    let mut fitting = (1, first_page);
    let mut lower = 2;
    let mut upper = available;
    while lower < upper {
        let middle = lower + (upper - lower) / 2;
        let (page, encoded) = evaluate(middle)?;
        if encoded <= budget {
            fitting = (middle, page);
            lower = middle.saturating_add(1);
        } else {
            upper = middle;
        }
    }
    Ok(fitting)
}

fn candidate_page_info<T>(
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
    offset: u64,
    accumulated: &Accumulated<T>,
    retained: usize,
    source_truncated: bool,
) -> Result<PageInfo, Failure> {
    let has_more = accumulated.has_more || retained < accumulated.items.len();
    let position = accumulated
        .items
        .get(retained.saturating_sub(1))
        .map(|positioned| positioned.position);
    let next_cursor = next_cursor(surface, query, source, offset, retained, position, has_more)?;
    Ok(page_info(retained, has_more, source_truncated, next_cursor))
}

fn timeline_data(items: &[Positioned<TimelineItem>], semantics: &[Value]) -> Value {
    let mut lanes = Vec::new();
    let mut markers = Vec::new();
    for positioned in items {
        match &positioned.item {
            TimelineItem::Lane(record) => lanes.push(record.clone()),
            TimelineItem::Marker(record) => markers.push(record.clone()),
        }
    }
    json!({"lanes": lanes, "markers": markers, "semantics": semantics})
}

fn page_semantics<T: Borrow<Value>>(
    base: &[Value],
    records: impl IntoIterator<Item = T>,
) -> Result<Vec<Value>, Failure> {
    let mut definitions = base.to_vec();
    definitions.extend(
        crate::mcp::semantics::referenced(records).map_err(|error| Failure {
            code: "semantics_unreadable",
            message: error.to_string(),
            parameter: None,
            retryable: false,
        })?,
    );
    Ok(definitions)
}

fn envelope_len(anchor: &Value, data: &Value, page: &PageInfo, warnings: &[Value]) -> usize {
    let page = super::page(
        page.returned,
        page.truncated,
        page.next_cursor.as_deref(),
        page.stop_reason,
    );
    super::super::structured_envelope_len(anchor, data, &page, warnings)
}

fn result_too_large(message: impl Into<String>) -> Failure {
    Failure {
        code: "result_too_large",
        message: message.into(),
        parameter: Some("data_budget_bytes".to_owned()),
        retryable: false,
    }
}

fn stream_hour(
    state: &State,
    window: Window,
    cancelled: &impl Fn() -> bool,
    mut accept: impl FnMut(Value) -> Result<(), Failure>,
) -> Result<(), Failure> {
    let prepared = api::prepare_for_mcp(
        &state.data_root,
        state.sources,
        state.synthetic_demo,
        Route::Hour(HourRequest {
            window,
            active_segment: None,
            series: None,
        }),
    )
    .map_err(|error| api_failure(&error))?;
    let saw_cancel = Cell::new(false);
    let tracked_cancel = || {
        let stopped = cancelled();
        saw_cancel.set(saw_cancel.get() || stopped);
        stopped
    };
    let mut callback_failure = None;
    prepared
        .stream_values(
            &mut |value| match accept(value) {
                Ok(()) => true,
                Err(failure) => {
                    callback_failure = Some(failure);
                    false
                }
            },
            &tracked_cancel,
        )
        .map_err(|error| api_failure(&error))?;
    if let Some(failure) = callback_failure {
        return Err(failure);
    }
    if saw_cancel.get() {
        return Err(Failure {
            code: "cancelled",
            message: "The historical index scan was cancelled.".to_owned(),
            parameter: None,
            retryable: true,
        });
    }
    Ok(())
}

fn page_start(
    raw: Option<&str>,
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
) -> Result<PageStart, Failure> {
    let Some(raw) = raw else {
        return Ok(PageStart {
            offset: 0,
            expected_position: None,
        });
    };
    let cursor = Cursor::parse(raw)?;
    if cursor.surface != surface || cursor.query != query {
        return Err(bad_cursor());
    }
    if cursor.source != source {
        return Err(source_changed());
    }
    Ok(PageStart {
        offset: cursor.offset,
        expected_position: Some(cursor.position),
    })
}

fn next_cursor(
    surface: Surface,
    query: Fingerprint,
    source: Fingerprint,
    offset: u64,
    returned: usize,
    position: Option<PositionKey>,
    has_more: bool,
) -> Result<Option<String>, Failure> {
    if !has_more {
        return Ok(None);
    }
    let position = position.ok_or_else(bad_cursor)?;
    let returned = u64::try_from(returned).map_err(|_overflow| {
        Failure::bounded(
            "position_limit_exceeded",
            "The returned record count exceeds the continuation limit.",
        )
    })?;
    let offset = offset.checked_add(returned).ok_or_else(|| {
        Failure::bounded(
            "position_limit_exceeded",
            "The next record position exceeds the continuation limit.",
        )
    })?;
    Ok(Some(
        Cursor {
            surface,
            query,
            source,
            position,
            offset,
        }
        .encode(),
    ))
}

const fn page_info(
    returned: usize,
    has_more: bool,
    source_truncated: bool,
    next_cursor: Option<String>,
) -> PageInfo {
    let (truncated, stop_reason) = if has_more {
        (true, "page_limit")
    } else if source_truncated {
        (true, "source_truncated")
    } else {
        (false, "complete")
    };
    PageInfo {
        returned,
        truncated,
        next_cursor,
        stop_reason,
    }
}

fn hours_query(window: Window) -> Fingerprint {
    let mut hash = Fingerprinter::new(b"mcp-hours-query-v1");
    hash.optional_i64(b"from", window.from);
    hash.optional_i64(b"to", window.to);
    hash.finish()
}

fn findings_query(window: Window, surface: Option<&str>, kind: Option<&str>) -> Fingerprint {
    let mut hash = Fingerprinter::new(b"mcp-findings-query-v1");
    hash.optional_i64(b"from", window.from);
    hash.optional_i64(b"to", window.to);
    optional_text(&mut hash, b"surface", surface);
    optional_text(&mut hash, b"kind", kind);
    hash.finish()
}

fn timeline_query(window: Window, lanes: &[String]) -> Fingerprint {
    let mut hash = Fingerprinter::new(b"mcp-timeline-query-v1");
    hash.optional_i64(b"from", window.from);
    hash.optional_i64(b"to", window.to);
    for lane in lanes {
        hash.part(b"lane", lane.as_bytes());
    }
    hash.finish()
}

fn optional_text(hash: &mut Fingerprinter, tag: &[u8], value: Option<&str>) {
    match value {
        Some(value) => {
            hash.part(tag, b"present");
            hash.part(tag, value.as_bytes());
        }
        None => hash.part(tag, b"absent"),
    }
}

fn value_position(surface: Surface, value: &Value) -> Result<PositionKey, Failure> {
    let encoded = serde_json::to_vec(value)
        .map_err(|error| Failure::bounded("record_unencodable", error.to_string()))?;
    let mut hash = Fingerprinter::new(b"mcp-page-position-v1");
    hash.part(b"surface", &[surface as u8]);
    hash.part(b"record", &encoded);
    Ok(PositionKey(hash.finish()))
}

fn hour_position(bucket: i64) -> PositionKey {
    let mut hash = Fingerprinter::new(b"mcp-hour-position-v1");
    hash.part(b"hour", &bucket.to_le_bytes());
    PositionKey(hash.finish())
}

fn cursor_check(body: &[u8]) -> [u8; CURSOR_CHECK_BYTES] {
    let mut hash = Fingerprinter::new(b"mcp-cursor-check-v1");
    hash.part(b"body", body);
    let digest = hash.finish();
    let mut check = [0; CURSOR_CHECK_BYTES];
    check.copy_from_slice(&digest.0[..CURSOR_CHECK_BYTES]);
    check
}

fn fingerprint_at(bytes: &[u8], start: usize) -> Result<Fingerprint, Failure> {
    let mut digest = [0; DIGEST_BYTES];
    digest.copy_from_slice(
        bytes
            .get(start..start + DIGEST_BYTES)
            .ok_or_else(bad_cursor)?,
    );
    Ok(Fingerprint(digest))
}

fn hour_catalog(
    root: &Path,
    window: Window,
    cancelled: &impl Fn() -> bool,
) -> Result<HourCatalog, Failure> {
    check_cancelled(cancelled)?;
    let reader = Reader::open(root).map_err(|error| unreadable(error.to_string()))?;
    let discovery = reader
        .catalog_discovery()
        .map_err(|error| unreadable(error.to_string()))?;
    check_cancelled(cancelled)?;
    let lower = window.from.map(|value| value.div_euclid(HOUR_US));
    let upper = window.to.map(|value| value.div_euclid(HOUR_US));
    let mut ranges = Vec::new();
    for (from, to) in discovery.ranges() {
        check_cancelled(cancelled)?;
        if from > to {
            continue;
        }
        let first = lower.map_or_else(
            || from.div_euclid(HOUR_US),
            |lower| from.div_euclid(HOUR_US).max(lower),
        );
        let last = upper.map_or_else(
            || to.div_euclid(HOUR_US),
            |upper| to.div_euclid(HOUR_US).min(upper),
        );
        if first <= last {
            ranges.push(HourRange { first, last });
        }
    }
    ranges.sort_unstable_by_key(|range| (range.first, range.last));
    let mut merged: Vec<HourRange> = Vec::with_capacity(ranges.len());
    for range in ranges {
        check_cancelled(cancelled)?;
        if let Some(previous) = merged.last_mut()
            && range.first <= previous.last.saturating_add(1)
        {
            previous.last = previous.last.max(range.last);
        } else {
            merged.push(range);
        }
    }
    let mut hash = Fingerprinter::new(b"mcp-hour-source-v1");
    for range in &merged {
        hash.part(b"first", &range.first.to_le_bytes());
        hash.part(b"last", &range.last.to_le_bytes());
    }
    Ok(HourCatalog {
        ranges: merged,
        source: hash.finish(),
    })
}

fn hour_count(ranges: &[HourRange], cancelled: &impl Fn() -> bool) -> Result<u64, Failure> {
    ranges.iter().try_fold(0_u64, |total, range| {
        check_cancelled(cancelled)?;
        let width = range
            .last
            .checked_sub(range.first)
            .and_then(|width| width.checked_add(1))
            .and_then(|width| u64::try_from(width).ok())
            .ok_or_else(|| {
                Failure::bounded(
                    "hour_limit_exceeded",
                    "The calendar range exceeds the hour-list position limit.",
                )
            })?;
        total.checked_add(width).ok_or_else(|| {
            Failure::bounded(
                "hour_limit_exceeded",
                "The hour count exceeds the continuation limit.",
            )
        })
    })
}

fn hour_at(
    ranges: &[HourRange],
    mut index: u64,
    cancelled: &impl Fn() -> bool,
) -> Result<Option<i64>, Failure> {
    for range in ranges {
        check_cancelled(cancelled)?;
        let width = range
            .last
            .checked_sub(range.first)
            .and_then(|width| width.checked_add(1))
            .and_then(|width| u64::try_from(width).ok());
        let Some(width) = width else {
            return Ok(None);
        };
        if index < width {
            let Some(index) = i64::try_from(index).ok() else {
                return Ok(None);
            };
            return Ok(range.first.checked_add(index));
        }
        index -= width;
    }
    Ok(None)
}

fn index_source(
    root: &Path,
    window: Window,
    cancelled: &impl Fn() -> bool,
) -> Result<SourceSnapshot, Failure> {
    check_cancelled(cancelled)?;
    let reader = Reader::open(root).map_err(|error| unreadable(error.to_string()))?;
    let listing = reader
        .catalog_segments((
            window.from.map_or(Unbounded, Included),
            window.to.map_or(Unbounded, Included),
        ))
        .map_err(|error| unreadable(error.to_string()))?;
    if listing.segments.len() > MAX_SEGMENTS {
        return Err(Failure::bounded(
            "segment_limit_exceeded",
            "The interval overlaps more than 64 segments.",
        ));
    }
    let mut hash = Fingerprinter::new(b"mcp-index-source-v1");
    let mut active_positions = BTreeMap::new();
    for segment in listing.segments {
        check_cancelled(cancelled)?;
        hash.part(b"segment", &segment.id().to_le_bytes());
        hash.part(b"min", &segment.min_ts().to_le_bytes());
        hash.part(b"max", &segment.max_ts().to_le_bytes());
        hash.part(
            b"kind",
            &[match segment.kind() {
                SegmentKind::Finished => 0,
                SegmentKind::Active => 1,
            }],
        );
        let active_position = segment.active_position();
        hash.part(
            b"active-position",
            &active_position.unwrap_or(0).to_le_bytes(),
        );
        for section in segment.sections() {
            hash.part(b"type", &section.type_id.to_le_bytes());
            hash.part(b"rows", &section.rows.to_le_bytes());
            hash.part(b"bytes", &section.bytes.to_le_bytes());
        }
        active_positions.insert(segment.id(), active_position);
    }
    Ok(SourceSnapshot {
        binding: hash.finish(),
        active_positions,
    })
}

fn ensure_source_unchanged(
    root: &Path,
    window: Window,
    expected: Fingerprint,
    cancelled: &impl Fn() -> bool,
) -> Result<(), Failure> {
    if index_source(root, window, cancelled)?.binding != expected {
        return Err(source_changed());
    }
    Ok(())
}

fn matches_surface(record: &Value, wanted: Option<&str>) -> bool {
    wanted.is_none_or(|wanted| record.get("logical_name").and_then(Value::as_str) == Some(wanted))
}

fn matches_kind(record: &Value, wanted: Option<&str>) -> bool {
    wanted.is_none_or(|wanted| record.get("kind").and_then(Value::as_str) == Some(wanted))
}

fn matches_lane(record: &Value, wanted: &HashSet<&str>) -> bool {
    wanted.is_empty()
        || record
            .get("lane")
            .or_else(|| record.get("series"))
            .and_then(Value::as_str)
            .is_some_and(|lane| wanted.contains(lane))
}

fn bad_cursor() -> Failure {
    Failure {
        code: "bad_cursor",
        message: "The continuation cursor does not match this normalized query and position."
            .to_owned(),
        parameter: Some("cursor".to_owned()),
        retryable: false,
    }
}

fn source_changed() -> Failure {
    Failure {
        code: "source_changed",
        message: "Source or active WAL position changed; restart this paged query.".to_owned(),
        parameter: Some("cursor".to_owned()),
        retryable: true,
    }
}

fn index_locator_failure() -> Failure {
    Failure::bounded(
        "index_locator_unavailable",
        "An indexed record has no segment locator.",
    )
}

const fn unreadable(message: String) -> Failure {
    Failure {
        code: "unreadable",
        message,
        parameter: None,
        retryable: true,
    }
}

fn stringify_integer(object: &mut serde_json::Map<String, Value>, name: &str) {
    let Some(value) = object.get_mut(name) else {
        return;
    };
    if let Some(number) = value.as_u64() {
        *value = json!(number.to_string());
    } else if let Some(number) = value.as_i64() {
        *value = json!(number.to_string());
    }
}

fn check_cancelled(cancelled: &impl Fn() -> bool) -> Result<(), Failure> {
    if cancelled() {
        return Err(Failure {
            code: "cancelled",
            message: "The historical catalog scan was cancelled.".to_owned(),
            parameter: None,
            retryable: true,
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests;
