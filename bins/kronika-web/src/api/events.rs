//! Bounded globally ordered pages over recorded Event streams.

use std::cmp::Reverse;
use std::collections::{BTreeMap, BinaryHeap, HashSet};
use std::ops::Bound::Included;
use std::path::Path;
use std::sync::Arc;

use kronika_reader::{Cell, Dictionary, Reader, ReaderError, Row, Segment, SegmentRef};
use kronika_registry::{contract, logical_section_name};
use serde_json::{Map, Value, json};

use super::query::{plans, resolved_dictionary};
use super::render::cell;
use super::snapshot::EventSearch;
use super::{ApiError, log_warnings};
use crate::product_semantics::{EventTier, ProductSemanticsError, SemanticPolicy};
use crate::route::{DataRequest, Order, SegmentRequest};

const CURSOR_VERSION: &str = "events-v1";
const MAX_SEGMENTS: usize = 64;
const MAX_PAGE_ROWS: usize = 500;
const MAX_ROW_VISITS: u64 = 1_000_000;
const MAX_DECODED_CELLS: u64 = 2_000_000;
const MAX_WARNING_RECORDS: usize = 64;
const ROW_CHUNK: usize = 256;

/// One allowlisted Event stream and its source-specific output projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventSourceRequest {
    pub(crate) logical_name: String,
    pub(crate) fields: Vec<String>,
}

/// One bounded Event query over recorded segment rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EventPageRequest {
    pub(crate) from_us: i64,
    pub(crate) to_us: i64,
    pub(crate) sources: Vec<EventSourceRequest>,
    pub(crate) find: Option<String>,
    pub(crate) direction: Order,
    pub(crate) page_size: usize,
    pub(crate) cursor: Option<String>,
}

/// Why an otherwise valid Event page ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EventStopReason {
    Complete,
    PageLimit,
    ByteLimit,
}

impl EventStopReason {
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::Complete => "complete",
            Self::PageLimit => "page_limit",
            Self::ByteLimit => "byte_limit",
        }
    }
}

/// One stable page plus the source snapshot facts used to produce it.
#[derive(Debug)]
pub(crate) struct EventPage {
    pub(crate) events: Vec<Value>,
    pub(crate) next_cursor: Option<String>,
    pub(crate) stop_reason: EventStopReason,
    pub(crate) active_position: Option<u64>,
    pub(crate) warnings: Vec<Value>,
}

/// Typed admission and reader failures for an Event page.
#[derive(Debug)]
pub(crate) enum EventPageError {
    Api(ApiError),
    Cancelled,
    ScanLimit,
    SegmentLimit,
    WarningLimit,
    FixedMetadataTooLarge,
    FirstRowTooLarge,
    Semantics(ProductSemanticsError),
    InvalidSemantics(String),
}

impl std::fmt::Display for EventPageError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Api(error) => error.fmt(f),
            Self::Cancelled => f.write_str("Event read was cancelled"),
            Self::ScanLimit => f.write_str("Event scan exceeded its row or cell limit"),
            Self::SegmentLimit => f.write_str("Event scan intersects more than 64 segments"),
            Self::WarningLimit => f.write_str("Event scan encountered more than 64 store warnings"),
            Self::FixedMetadataTooLarge => {
                f.write_str("the fixed Event metadata exceeds the value-byte limit")
            }
            Self::FirstRowTooLarge => {
                f.write_str("the fixed Event metadata and first row exceed the value-byte limit")
            }
            Self::Semantics(error) => error.fmt(f),
            Self::InvalidSemantics(error) => f.write_str(error),
        }
    }
}

impl std::error::Error for EventPageError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Api(error) => Some(error),
            Self::Semantics(error) => Some(error),
            Self::Cancelled
            | Self::ScanLimit
            | Self::SegmentLimit
            | Self::WarningLimit
            | Self::FixedMetadataTooLarge
            | Self::FirstRowTooLarge
            | Self::InvalidSemantics(_) => None,
        }
    }
}

impl From<ApiError> for EventPageError {
    fn from(error: ApiError) -> Self {
        Self::Api(error)
    }
}

impl From<ReaderError> for EventPageError {
    fn from(error: ReaderError) -> Self {
        Self::Api(ApiError::from(error))
    }
}

impl From<ProductSemanticsError> for EventPageError {
    fn from(error: ProductSemanticsError) -> Self {
        Self::Semantics(error)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct EventKey {
    timestamp_us: i64,
    segment_id: i64,
    type_id: u32,
    row_ordinal: u64,
}

struct Candidate {
    key: EventKey,
    source_index: usize,
    tier: EventTier,
    row: Row,
    segment: Arc<Segment>,
}

impl PartialEq for Candidate {
    fn eq(&self, other: &Self) -> bool {
        self.key == other.key
    }
}

impl Eq for Candidate {}

impl PartialOrd for Candidate {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Candidate {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        self.key.cmp(&other.key)
    }
}

enum Retained {
    Asc(BinaryHeap<Candidate>),
    Desc(BinaryHeap<Reverse<Candidate>>),
}

impl Retained {
    const fn new(direction: Order) -> Self {
        match direction {
            Order::Asc => Self::Asc(BinaryHeap::new()),
            Order::Desc => Self::Desc(BinaryHeap::new()),
        }
    }

    fn push(&mut self, candidate: Candidate, capacity: usize) {
        match self {
            Self::Asc(heap) => {
                heap.push(candidate);
                if heap.len() > capacity {
                    let _discarded = heap.pop();
                }
            }
            Self::Desc(heap) => {
                heap.push(Reverse(candidate));
                if heap.len() > capacity {
                    let _discarded = heap.pop();
                }
            }
        }
    }

    fn ordered(self, direction: Order) -> Vec<Candidate> {
        let mut candidates = match self {
            Self::Asc(heap) => heap.into_vec(),
            Self::Desc(heap) => heap.into_iter().map(|item| item.0).collect(),
        };
        candidates.sort_by_key(|candidate| candidate.key);
        if direction == Order::Desc {
            candidates.reverse();
        }
        candidates
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Cursor {
    key: EventKey,
    active: Option<(i64, u64)>,
    binding: u64,
    sources: u64,
}

#[derive(Clone, Copy)]
struct CursorSource {
    active: Option<(i64, u64)>,
    binding: u64,
    sources: u64,
}

struct TierPolicy {
    discriminator: Option<String>,
    tiers: Vec<EventTier>,
    fallback: EventTier,
}

/// Read one exact globally ordered Event page from a stable recorded prefix.
pub(crate) fn read_event_page(
    root: &Path,
    request: &EventPageRequest,
    cancelled: &impl Fn() -> bool,
    page_fits: &impl Fn(usize, usize, Option<&str>, EventStopReason, Option<u64>, &[Value]) -> bool,
) -> Result<EventPage, EventPageError> {
    validate_request(request)?;
    let search = request
        .find
        .as_deref()
        .map(EventSearch::parse)
        .transpose()?;
    let binding = request_binding(request, search.as_ref());
    let cursor = request
        .cursor
        .as_deref()
        .map(Cursor::parse)
        .transpose()?
        .filter(|cursor| cursor.binding == binding);
    if request.cursor.is_some() && cursor.is_none() {
        return Err(ApiError::BadCursor.into());
    }

    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments((Included(request.from_us), Included(request.to_us)))?;
    log_warnings(&listing.warnings);
    if listing.segments.len() > MAX_SEGMENTS {
        return Err(EventPageError::SegmentLimit);
    }
    if listing.warnings.len() > MAX_WARNING_RECORDS {
        return Err(EventPageError::WarningLimit);
    }
    let warnings = listing
        .warnings
        .iter()
        .map(super::catalog::warning_value)
        .collect();
    let selected = request
        .sources
        .iter()
        .map(|source| source.logical_name.as_str())
        .collect::<HashSet<_>>();
    let relevant = listing
        .segments
        .into_iter()
        .filter(|segment| {
            segment.sections().iter().any(|section| {
                logical_section_name(section.type_id).is_some_and(|name| selected.contains(name))
            })
        })
        .collect::<Vec<_>>();
    let pinned = pin_sources(relevant, cursor)?;
    let source_binding = source_binding(&pinned, &selected);
    if cursor.is_some_and(|cursor| cursor.sources != source_binding) {
        return Err(source_changed().into());
    }
    let active = pinned.iter().find_map(|segment| {
        segment
            .active_position()
            .map(|position| (segment.id(), position))
    });

    let retained = scan_candidates(
        &reader,
        &pinned,
        request,
        search.as_ref(),
        cursor,
        cancelled,
    )?;
    render_page(
        retained,
        request,
        CursorSource {
            active,
            binding,
            sources: source_binding,
        },
        warnings,
        cancelled,
        page_fits,
    )
}

fn validate_request(request: &EventPageRequest) -> Result<(), EventPageError> {
    if request.from_us > request.to_us {
        return Err(ApiError::BadFilter("window".to_owned()).into());
    }
    if request.sources.is_empty() {
        return Err(ApiError::BadFilter("sources".to_owned()).into());
    }
    if request.page_size == 0 || request.page_size > MAX_PAGE_ROWS {
        return Err(ApiError::BadFilter("page_size".to_owned()).into());
    }
    Ok(())
}

fn pin_sources(
    mut sources: Vec<SegmentRef>,
    cursor: Option<Cursor>,
) -> Result<Vec<SegmentRef>, EventPageError> {
    let Some(cursor) = cursor else {
        return Ok(sources);
    };
    let current = sources
        .iter()
        .enumerate()
        .find_map(|(index, segment)| segment.active_position().map(|position| (index, position)));
    match (cursor.active, current) {
        (None, None) => Ok(sources),
        (Some((wanted_id, wanted_position)), Some((index, current_position)))
            if sources[index].id() == wanted_id && current_position >= wanted_position =>
        {
            sources[index] = sources[index]
                .at_active_position(wanted_position)
                .map_err(|_error| source_changed())?;
            Ok(sources)
        }
        (None | Some(_), Some(_)) | (Some(_), None) => Err(source_changed().into()),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "the bounded scan keeps its cancellation, row, cell, and segment admission checks in one linear loop"
)]
fn scan_candidates(
    reader: &Reader,
    references: &[SegmentRef],
    request: &EventPageRequest,
    search: Option<&EventSearch>,
    cursor: Option<Cursor>,
    cancelled: &impl Fn() -> bool,
) -> Result<Retained, EventPageError> {
    let mut retained = Retained::new(request.direction);
    let capacity = request.page_size.saturating_add(1);
    let mut visited = 0_u64;
    let mut decoded_cells = 0_u64;
    for reference in references {
        if cancelled() {
            return Err(EventPageError::Cancelled);
        }
        let segment = Arc::new(reader.open_segment(reference)?);
        for (source_index, source) in request.sources.iter().enumerate() {
            if segment.layouts(&source.logical_name).next().is_none() {
                continue;
            }
            let policy = tier_policy(&source.logical_name)?;
            let mut fields = source.fields.clone();
            if !fields.iter().any(|field| field == "ts") {
                fields.push("ts".to_owned());
            }
            let data = DataRequest {
                segment: SegmentRequest {
                    segment_id: segment.id(),
                    section: source.logical_name.clone(),
                },
                fields,
                filters: Vec::new(),
                type_id: None,
                after: None,
            };
            for mut plan in plans(&segment, &data, true)? {
                let timestamp = plan.timestamp.ok_or_else(|| {
                    ApiError::Unreadable(Box::new(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        format!("Event layout {} has no timestamp", plan.type_id),
                    )))
                })?;
                let mut hidden = vec![timestamp];
                if let Some(discriminator) = policy.discriminator.as_deref()
                    && let Some(column) = plan.contract.column(discriminator)
                {
                    hidden.push(column.name);
                }
                if let Some(category) = plan.contract.column("category") {
                    hidden.push(category.name);
                }
                if search.is_some() {
                    hidden.extend(
                        plan.contract
                            .columns
                            .iter()
                            .filter(|column| column.ty == kronika_registry::ColumnType::StrId)
                            .map(|column| column.name),
                    );
                }
                plan.add_projection_columns(&hidden);
                let cells = u64::try_from(plan.projection.len()).unwrap_or(u64::MAX);
                let mut offset = 0_u64;
                loop {
                    let mut interrupted = false;
                    let mut scan_limited = false;
                    let mut rows = Vec::with_capacity(ROW_CHUNK);
                    let chunk = segment.visit_rows(
                        plan.type_id,
                        &plan.projection,
                        offset,
                        ROW_CHUNK,
                        |row_ordinal, row| {
                            if cancelled() {
                                interrupted = true;
                                return false;
                            }
                            if visited >= MAX_ROW_VISITS
                                || decoded_cells.saturating_add(cells) > MAX_DECODED_CELLS
                            {
                                scan_limited = true;
                                return false;
                            }
                            visited = visited.saturating_add(1);
                            decoded_cells = decoded_cells.saturating_add(cells);
                            rows.push((row_ordinal, row));
                            true
                        },
                    )?;
                    if interrupted {
                        return Err(EventPageError::Cancelled);
                    }
                    if scan_limited {
                        return Err(EventPageError::ScanLimit);
                    }
                    let dictionary = if search.is_some() {
                        let ids = rows
                            .iter()
                            .flat_map(|(_ordinal, row)| row.iter())
                            .filter_map(|(_name, cell)| match cell {
                                Cell::StrId(id) => Some(*id),
                                _other => None,
                            })
                            .collect::<HashSet<_>>();
                        Some(resolved_dictionary(&segment, &ids)?)
                    } else {
                        None
                    };
                    for (row_ordinal, row) in rows {
                        let Some(timestamp_us) = row_timestamp(&row, timestamp) else {
                            continue;
                        };
                        if timestamp_us < request.from_us || timestamp_us > request.to_us {
                            continue;
                        }
                        let key = EventKey {
                            timestamp_us,
                            segment_id: segment.id(),
                            type_id: plan.type_id,
                            row_ordinal,
                        };
                        if cursor
                            .is_some_and(|cursor| !after_cursor(key, cursor.key, request.direction))
                        {
                            continue;
                        }
                        let tier = policy.tier(&row);
                        if let Some(search) = search {
                            let dictionary = dictionary.as_ref().ok_or_else(|| {
                                EventPageError::InvalidSemantics(
                                    "missing Event search dictionary".to_owned(),
                                )
                            })?;
                            if !search.matches(
                                &source.logical_name,
                                tier_code(tier),
                                &row,
                                dictionary,
                            ) {
                                continue;
                            }
                        }
                        retained.push(
                            Candidate {
                                key,
                                source_index,
                                tier,
                                row,
                                segment: Arc::clone(&segment),
                            },
                            capacity,
                        );
                    }
                    offset = offset.saturating_add(u64::try_from(chunk).unwrap_or(u64::MAX));
                    if chunk < ROW_CHUNK {
                        break;
                    }
                }
            }
        }
    }
    Ok(retained)
}

fn render_page(
    retained: Retained,
    request: &EventPageRequest,
    source: CursorSource,
    warnings: Vec<Value>,
    cancelled: &impl Fn() -> bool,
    page_fits: &impl Fn(usize, usize, Option<&str>, EventStopReason, Option<u64>, &[Value]) -> bool,
) -> Result<EventPage, EventPageError> {
    let candidates = retained.ordered(request.direction);
    let retained_count = candidates.len();
    let target = retained_count.min(request.page_size);
    if cancelled() {
        return Err(EventPageError::Cancelled);
    }
    let active_position = source.active.map(|(_id, position)| position);
    if target == 0
        && !page_fits(
            0,
            0,
            None,
            EventStopReason::Complete,
            active_position,
            &warnings,
        )
    {
        return Err(EventPageError::FixedMetadataTooLarge);
    }
    let dictionaries = dictionaries(&candidates[..target])?;
    let mut events = Vec::with_capacity(target);
    let mut encoded = 0_usize;
    let mut byte_limited = false;
    for candidate in candidates.iter().take(target) {
        if cancelled() {
            return Err(EventPageError::Cancelled);
        }
        let dictionary = dictionaries
            .get(&candidate.key.segment_id)
            .ok_or_else(|| missing_dictionary(candidate.key.segment_id))?;
        let value = render_event(candidate, request, dictionary)?;
        let bytes = serde_json::to_vec(&value).map_err(ApiError::from)?.len();
        let candidate_encoded = encoded
            .saturating_add(usize::from(!events.is_empty()))
            .saturating_add(bytes);
        let returned = events.len().saturating_add(1);
        let has_more = returned < retained_count;
        let stop_reason = if returned < target {
            EventStopReason::ByteLimit
        } else if retained_count > request.page_size {
            EventStopReason::PageLimit
        } else {
            EventStopReason::Complete
        };
        let next_cursor = has_more.then(|| {
            Cursor {
                key: candidate.key,
                active: source.active,
                binding: source.binding,
                sources: source.sources,
            }
            .encode()
        });
        if !page_fits(
            returned,
            candidate_encoded,
            next_cursor.as_deref(),
            stop_reason,
            active_position,
            &warnings,
        ) {
            if events.is_empty() {
                return Err(EventPageError::FirstRowTooLarge);
            }
            byte_limited = true;
            break;
        }
        encoded = candidate_encoded;
        events.push(value);
    }
    let has_more = events.len() < retained_count;
    let stop_reason = if byte_limited {
        EventStopReason::ByteLimit
    } else if retained_count > request.page_size {
        EventStopReason::PageLimit
    } else {
        EventStopReason::Complete
    };
    let next_cursor = has_more.then(|| {
        let key = candidates[events.len().saturating_sub(1)].key;
        Cursor {
            key,
            active: source.active,
            binding: source.binding,
            sources: source.sources,
        }
        .encode()
    });
    Ok(EventPage {
        events,
        next_cursor,
        stop_reason,
        active_position,
        warnings,
    })
}

fn dictionaries(candidates: &[Candidate]) -> Result<BTreeMap<i64, Dictionary>, EventPageError> {
    let mut selected = BTreeMap::<i64, (Arc<Segment>, HashSet<u64>)>::new();
    for candidate in candidates {
        let entry = selected
            .entry(candidate.key.segment_id)
            .or_insert_with(|| (Arc::clone(&candidate.segment), HashSet::new()));
        entry
            .1
            .extend(candidate.row.iter().filter_map(|(_name, cell)| {
                if let Cell::StrId(id) = cell {
                    Some(*id)
                } else {
                    None
                }
            }));
    }
    selected
        .into_iter()
        .map(|(segment_id, (segment, ids))| {
            resolved_dictionary(&segment, &ids).map(|dictionary| (segment_id, dictionary))
        })
        .collect::<Result<_, _>>()
        .map_err(EventPageError::from)
}

fn render_event(
    candidate: &Candidate,
    request: &EventPageRequest,
    dictionary: &Dictionary,
) -> Result<Value, EventPageError> {
    let source = &request.sources[candidate.source_index];
    let layout = contract(candidate.key.type_id).ok_or(ApiError::NoSuchSection)?;
    let fields = source
        .fields
        .iter()
        .map(|name| {
            let value = layout
                .column(name)
                .and_then(|column| candidate.row.get(column.name))
                .map_or(Ok(Value::Null), |stored| cell(stored, dictionary))?;
            Ok((name.clone(), value))
        })
        .collect::<Result<Map<_, _>, ApiError>>()?;
    Ok(json!({
        "section": source.logical_name,
        "tier": candidate.tier,
        "semantic_id": format!("event.{}.tier", source.logical_name),
        "segment_id": candidate.key.segment_id.to_string(),
        "type_id": candidate.key.type_id.to_string(),
        "row_ordinal": candidate.key.row_ordinal.to_string(),
        "timestamp_us": candidate.key.timestamp_us.to_string(),
        "fields": fields,
    }))
}

fn tier_policy(section: &str) -> Result<TierPolicy, EventPageError> {
    let id = format!("event.{section}.tier");
    let definition = crate::product_semantics::get(&id)?.ok_or_else(|| {
        EventPageError::InvalidSemantics(format!("missing accepted Event tier for {section}"))
    })?;
    let SemanticPolicy::EventTier {
        discriminator,
        tiers,
        fallback,
        ..
    } = &definition.policy
    else {
        return Err(EventPageError::InvalidSemantics(format!(
            "invalid accepted Event tier for {section}"
        )));
    };
    Ok(TierPolicy {
        discriminator: discriminator.clone(),
        tiers: tiers.clone(),
        fallback: fallback.to_owned(),
    })
}

impl TierPolicy {
    fn tier(&self, row: &Row) -> EventTier {
        self.discriminator
            .as_deref()
            .and_then(|field| row.get(field))
            .and_then(cell_index)
            .and_then(|index| self.tiers.get(index))
            .copied()
            .unwrap_or(self.fallback)
    }
}

const fn tier_code(tier: EventTier) -> &'static str {
    match tier {
        EventTier::Critical => "critical",
        EventTier::Notable => "notable",
        EventTier::Routine => "routine",
    }
}

fn cell_index(cell: &Cell) -> Option<usize> {
    match cell {
        Cell::I16(value) => usize::try_from(*value).ok(),
        Cell::I32(value) => usize::try_from(*value).ok(),
        Cell::I64(value) => usize::try_from(*value).ok(),
        Cell::U32(value) => usize::try_from(*value).ok(),
        Cell::U64(value) => usize::try_from(*value).ok(),
        _other => None,
    }
}

fn row_timestamp(row: &Row, column: &str) -> Option<i64> {
    match row.get(column) {
        Some(Cell::Ts(value)) => Some(*value),
        _other => None,
    }
}

fn after_cursor(key: EventKey, cursor: EventKey, direction: Order) -> bool {
    match direction {
        Order::Asc => key > cursor,
        Order::Desc => key < cursor,
    }
}

impl Cursor {
    fn parse(raw: &str) -> Result<Self, ApiError> {
        let fields = raw.split(',').collect::<Vec<_>>();
        if fields.len() != 9 || fields[0] != CURSOR_VERSION {
            return Err(ApiError::BadCursor);
        }
        let active_id = if fields[5] == "-" {
            None
        } else {
            Some(fields[5].parse().map_err(|_error| ApiError::BadCursor)?)
        };
        let active_position = fields[6].parse().map_err(|_error| ApiError::BadCursor)?;
        if active_id.is_none() && active_position != 0 {
            return Err(ApiError::BadCursor);
        }
        Ok(Self {
            key: EventKey {
                timestamp_us: fields[1].parse().map_err(|_error| ApiError::BadCursor)?,
                segment_id: fields[2].parse().map_err(|_error| ApiError::BadCursor)?,
                type_id: fields[3].parse().map_err(|_error| ApiError::BadCursor)?,
                row_ordinal: fields[4].parse().map_err(|_error| ApiError::BadCursor)?,
            },
            active: active_id.map(|id| (id, active_position)),
            binding: fields[7].parse().map_err(|_error| ApiError::BadCursor)?,
            sources: fields[8].parse().map_err(|_error| ApiError::BadCursor)?,
        })
    }

    fn encode(self) -> String {
        let (active_id, active_position) = self.active.map_or_else(
            || ("-".to_owned(), 0),
            |(id, position)| (id.to_string(), position),
        );
        format!(
            "{CURSOR_VERSION},{},{},{},{},{active_id},{active_position},{},{}",
            self.key.timestamp_us,
            self.key.segment_id,
            self.key.type_id,
            self.key.row_ordinal,
            self.binding,
            self.sources,
        )
    }
}

fn request_binding(request: &EventPageRequest, search: Option<&EventSearch>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    hash_part(&mut hash, b"from", &request.from_us.to_le_bytes());
    hash_part(&mut hash, b"to", &request.to_us.to_le_bytes());
    hash_part(
        &mut hash,
        b"direction",
        &[u8::from(request.direction == Order::Desc)],
    );
    for source in &request.sources {
        hash_part(&mut hash, b"source", source.logical_name.as_bytes());
        for field in &source.fields {
            hash_part(&mut hash, b"field", field.as_bytes());
        }
    }
    if let Some(search) = search {
        hash_part(&mut hash, b"find", search.canonical().as_bytes());
    }
    hash
}

fn source_binding(sources: &[SegmentRef], selected: &HashSet<&str>) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for source in sources {
        hash_part(&mut hash, b"segment", &source.id().to_le_bytes());
        hash_part(&mut hash, b"min", &source.min_ts().to_le_bytes());
        hash_part(&mut hash, b"max", &source.max_ts().to_le_bytes());
        let position = source.active_position().unwrap_or(0);
        hash_part(&mut hash, b"active", &position.to_le_bytes());
        for section in source.sections().iter().filter(|section| {
            logical_section_name(section.type_id).is_some_and(|name| selected.contains(name))
        }) {
            hash_part(&mut hash, b"type", &section.type_id.to_le_bytes());
            hash_part(&mut hash, b"rows", &section.rows.to_le_bytes());
            hash_part(&mut hash, b"bytes", &section.bytes.to_le_bytes());
        }
    }
    hash
}

fn hash_part(hash: &mut u64, tag: &[u8], bytes: &[u8]) {
    hash_bytes(hash, tag);
    hash_bytes(hash, bytes);
}

fn hash_bytes(hash: &mut u64, bytes: &[u8]) {
    for byte in bytes.len().to_le_bytes().iter().chain(bytes) {
        *hash ^= u64::from(*byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
}

fn missing_dictionary(segment_id: i64) -> EventPageError {
    ApiError::Unreadable(Box::new(std::io::Error::new(
        std::io::ErrorKind::InvalidData,
        format!("missing retained Event dictionary for segment {segment_id}"),
    )))
    .into()
}

fn source_changed() -> ApiError {
    ReaderError::from(std::io::Error::new(
        std::io::ErrorKind::Interrupted,
        "recorded Event source changed between pages",
    ))
    .into()
}
