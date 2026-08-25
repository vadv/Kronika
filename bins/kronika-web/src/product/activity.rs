//! Typed PostgreSQL Activity query, executor, and structured result.

use std::cmp::Ordering;
use std::collections::HashSet;
use std::fmt;
use std::path::Path;

use kronika_reader::{Cell, Dictionary, Reader, Resolved, Row, Segment, SegmentRef};
use kronika_registry::contract;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::execution::{Execution, ExecutionStop};
use super::page::{
    CursorBinding, DEFAULT_PAGE_SIZE, DecodedCursor, Direction, MAX_PAGE_SIZE, PageError, PageKey,
    PageRequest, PageSurface, QueryBinding, SourcePin, decode_cursor, encode_cursor, fit_page,
    reopen_sources,
};

const MICROS_PER_HOUR: i128 = 3_600_000_000;
const ACTIVITY_LAYOUTS: [u32; 3] = [1_001_001, 1_001_002, 1_001_004];
const ACTIVITY_COLUMNS: [&str; 20] = [
    "ts",
    "pid",
    "leader_pid",
    "datid",
    "datname",
    "usename",
    "application_name",
    "client_addr",
    "backend_type",
    "state",
    "wait_event_type",
    "wait_event",
    "query",
    "query_id",
    "backend_xid_age",
    "backend_xmin_age",
    "backend_start",
    "xact_start",
    "query_start",
    "state_change",
];
const ROW_CHUNK: usize = 1_024;
const MAX_FILTER_CLAUSES: usize = 18;
const MAX_CLAUSE_PROPERTIES: usize = 8;
const MAX_CLAUSE_VALUES: usize = 8;
const MAX_PATTERN_SCALARS: usize = 256;

/// Unvalidated transport arguments for `kronika_read_postgresql_activity`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityArgs {
    /// Required canonical signed-i64 unix microseconds.
    pub(crate) at: String,
    /// Optional OR array; omission and an empty array differ.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) filter: Option<Vec<ActivityClauseArgs>>,
    /// Optional semantic primary sort.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) sort: Option<ActivitySort>,
    /// Optional semantic direction.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) direction: Option<Direction>,
    /// Optional common page maximum.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) page_size: Option<u16>,
    /// Optional opaque continuation.
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) cursor: Option<String>,
}

fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

impl ActivityArgs {
    /// Parse the closed runtime shape published by the Tool descriptor.
    pub(crate) fn from_value(value: Value) -> Result<Self, ActivityError> {
        serde_json::from_value(value).map_err(|_error| {
            ActivityError::InvalidArguments("arguments do not match the Activity input schema")
        })
    }
}

/// Unvalidated flat Activity clause.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityClauseArgs {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) text: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) pid: Option<PidMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) query_id: Option<QueryIdMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) database: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) role: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) application: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) client: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) backend_type: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) state: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) wait_type: Option<TextMatchArgs>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) wait_event: Option<TextMatchArgs>,
}

/// Unvalidated any/all text matcher.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct TextMatchArgs {
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) any_of: Option<Vec<String>>,
    #[serde(
        default,
        deserialize_with = "present",
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) all_of: Option<Vec<String>>,
}

/// Unvalidated exact positive PID alternatives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct PidMatchArgs {
    pub(crate) any_of: Vec<i64>,
}

/// Unvalidated exact signed Query ID alternatives.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QueryIdMatchArgs {
    pub(crate) any_of: Vec<String>,
}

/// Public Activity sort tokens.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ActivitySort {
    Pid,
    Database,
    Role,
    QueryPreview,
    QueryDurationMs,
    TransactionDurationMs,
    Application,
    Client,
    State,
    WaitType,
    WaitEvent,
    BackendType,
}

impl ActivitySort {
    /// Exact public token.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Pid => "pid",
            Self::Database => "database",
            Self::Role => "role",
            Self::QueryPreview => "query_preview",
            Self::QueryDurationMs => "query_duration_ms",
            Self::TransactionDurationMs => "transaction_duration_ms",
            Self::Application => "application",
            Self::Client => "client",
            Self::State => "state",
            Self::WaitType => "wait_type",
            Self::WaitEvent => "wait_event",
            Self::BackendType => "backend_type",
        }
    }
}

/// Fully normalized Activity product query shared by HTTP and MCP.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ActivityQuery {
    /// Required requested timestamp.
    pub(crate) at: i64,
    hour_start_wide: i128,
    hour_end_exclusive_wide: i128,
    filter: ActivityFilter,
    /// Effective semantic sort.
    pub(crate) sort: ActivitySort,
    /// Effective semantic direction.
    pub(crate) direction: Direction,
    /// Effective shared page request.
    pub(crate) page: PageRequest,
    query_binding: String,
}

/// One structured Activity result.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ActivityResult {
    /// Requested `at` in canonical decimal text.
    pub(crate) requested_at: String,
    /// Selected observation in canonical decimal text.
    pub(crate) observed_at: Option<String>,
    /// Whole rows in deterministic product order.
    pub(crate) rows: Vec<ActivityRow>,
    /// First-unreturned continuation or `None`.
    pub(crate) next_cursor: Option<String>,
}

/// One normalized PG10-18 Activity row.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ActivityRow {
    pub(crate) observed_at: String,
    pub(crate) pid: i32,
    pub(crate) leader_pid: Option<i32>,
    pub(crate) datid: Option<u32>,
    pub(crate) datname: Option<String>,
    pub(crate) usename: Option<String>,
    pub(crate) application_name: String,
    pub(crate) client_addr: String,
    pub(crate) backend_type: String,
    pub(crate) state: Option<String>,
    pub(crate) wait_event_type: Option<String>,
    pub(crate) wait_event: Option<String>,
    pub(crate) query_preview: Option<String>,
    pub(crate) query_id: Option<String>,
    pub(crate) backend_xid_age: Option<String>,
    pub(crate) backend_xmin_age: Option<String>,
    pub(crate) backend_start: String,
    pub(crate) xact_start: Option<String>,
    pub(crate) query_start: Option<String>,
    pub(crate) state_change: Option<String>,
    pub(crate) backend_age_ms: Option<f64>,
    pub(crate) query_duration_ms: Option<f64>,
    pub(crate) transaction_duration_ms: Option<f64>,
    pub(crate) state_duration_ms: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
enum ActivityFilter {
    All,
    Clauses(Vec<ActivityClause>),
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
struct ActivityClause {
    #[serde(skip_serializing_if = "Option::is_none")]
    text: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pid: Option<Vec<i32>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    query_id: Option<Vec<i64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    database: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    role: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    application: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    client: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    backend_type: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    state: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait_type: Option<TextMatch>,
    #[serde(skip_serializing_if = "Option::is_none")]
    wait_event: Option<TextMatch>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct TextMatch {
    #[serde(skip_serializing_if = "Vec::is_empty")]
    any_of: Vec<GlobPattern>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    all_of: Vec<GlobPattern>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct GlobPattern {
    source: String,
    #[serde(skip)]
    tokens: Vec<GlobToken>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GlobToken {
    Star,
    Any,
    Literal(char),
}

/// Stable Activity tool failure.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ActivityError {
    InvalidArguments(&'static str),
    ReadFailed(&'static str),
    ResultTooLarge,
    Cancelled,
    DeadlineExceeded,
}

impl ActivityError {
    /// Stable error code published in the tool-error envelope.
    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::InvalidArguments(_) => "invalid_arguments",
            Self::ReadFailed(_) => "activity_read_failed",
            Self::ResultTooLarge => "result_too_large",
            Self::Cancelled => "cancelled",
            Self::DeadlineExceeded => "deadline_exceeded",
        }
    }

    /// Sanitized caller-facing recovery detail.
    pub(crate) const fn message(self) -> &'static str {
        match self {
            Self::InvalidArguments(message) | Self::ReadFailed(message) => message,
            Self::ResultTooLarge => {
                "the shared page envelope cannot fit the next whole Activity row as a one-row page"
            }
            Self::Cancelled => "the Activity read was cancelled",
            Self::DeadlineExceeded => "the Activity read deadline elapsed",
        }
    }
}

impl fmt::Display for ActivityError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.message())
    }
}

impl std::error::Error for ActivityError {}

/// Normalize raw transport arguments into the one product query.
pub(crate) fn normalize_activity(args: ActivityArgs) -> Result<ActivityQuery, ActivityError> {
    let at = canonical_i64(&args.at, "at must be canonical signed i64 decimal text")?;
    let hour_index = i128::from(at).div_euclid(MICROS_PER_HOUR);
    let hour_start_wide = hour_index * MICROS_PER_HOUR;
    let hour_end_exclusive_wide = hour_start_wide + MICROS_PER_HOUR;
    let filter = normalize_filter(args.filter)?;
    let sort = args.sort.unwrap_or(ActivitySort::QueryDurationMs);
    let direction = args.direction.unwrap_or(Direction::Desc);
    let page_size = args.page_size.unwrap_or(DEFAULT_PAGE_SIZE);
    if !(1..=MAX_PAGE_SIZE).contains(&page_size) {
        return Err(ActivityError::InvalidArguments(
            "page_size must be between 1 and 5000",
        ));
    }
    if args
        .cursor
        .as_ref()
        .is_some_and(|cursor| cursor.is_empty() || cursor.len() > 4_096)
    {
        return Err(ActivityError::InvalidArguments(
            "cursor must contain between 1 and 4096 bytes",
        ));
    }
    let filter_bytes = serde_json::to_vec(&filter).map_err(|_error| {
        ActivityError::InvalidArguments("the normalized Activity filter could not be bound")
    })?;
    let mut binding = QueryBinding::new(PageSurface::Activity);
    binding.part("at", &at.to_le_bytes());
    binding.part("hour_start", hour_start_wide.to_string().as_bytes());
    binding.part(
        "hour_end_exclusive",
        hour_end_exclusive_wide.to_string().as_bytes(),
    );
    binding.part("filter", &filter_bytes);
    binding.part("sort", sort.as_str().as_bytes());
    binding.part(
        "direction",
        match direction {
            Direction::Asc => b"asc",
            Direction::Desc => b"desc",
        },
    );
    Ok(ActivityQuery {
        at,
        hour_start_wide,
        hour_end_exclusive_wide,
        filter,
        sort,
        direction,
        page: PageRequest {
            page_size,
            cursor: args.cursor,
        },
        query_binding: binding.finish(),
    })
}

/// Execute one normalized Activity query against recorded storage.
pub(crate) fn execute_activity(
    root: &Path,
    query: &ActivityQuery,
    key: &PageKey,
    execution: &Execution,
) -> Result<ActivityResult, ActivityError> {
    checkpoint(execution)?;
    let reader = Reader::open(root).map_err(|_error| read_failed())?;
    let (sources, pins, observed_at, first_unreturned) =
        if let Some(raw_cursor) = query.page.cursor.as_deref() {
            let cursor = decode_cursor(raw_cursor, key).map_err(map_page_error)?;
            validate_cursor_query(query, &cursor)?;
            let sources = reopen_sources(&reader, &cursor.binding.source_pins)
                .map_err(|_error| invalid_cursor())?;
            let observed_at = cursor.binding.selected_at.ok_or_else(invalid_cursor)?;
            let selected = select_observation(&reader, &sources, query, execution, true)?;
            if selected != Some(observed_at) {
                return Err(invalid_cursor());
            }
            (
                sources,
                cursor.binding.source_pins,
                Some(observed_at),
                cursor.first_unreturned,
            )
        } else {
            let lower = i64::try_from(query.hour_start_wide.max(i128::from(i64::MIN)))
                .map_err(|_overflow| read_failed())?;
            let listing = reader
                .segments(lower..=query.at)
                .map_err(|_error| read_failed())?;
            let sources = listing.segments;
            let pins = sources.iter().map(SourcePin::capture).collect::<Vec<_>>();
            let observed_at = select_observation(&reader, &sources, query, execution, false)?;
            (sources, pins, observed_at, 0)
        };
    let Some(observed_at) = observed_at else {
        return Ok(ActivityResult {
            requested_at: query.at.to_string(),
            observed_at: None,
            rows: Vec::new(),
            next_cursor: None,
        });
    };
    let mut rows = read_observation_rows(
        &reader,
        &sources,
        observed_at,
        &query.filter,
        execution,
        query.page.cursor.is_some(),
    )?;
    checkpoint(execution)?;
    sort_rows(&mut rows, query.sort, query.direction, execution)?;
    let rows = rows.into_iter().map(|row| row.row).collect::<Vec<_>>();
    let binding = CursorBinding {
        surface: PageSurface::Activity,
        query_binding: query.query_binding.clone(),
        selected_at: Some(observed_at),
        source_pins: pins,
        page_size: query.page.page_size,
    };
    let requested_text = query.at.to_string();
    let observed_text = observed_at.to_string();
    let page = fit_page(
        &rows,
        first_unreturned,
        query.page.page_size,
        |offset| encode_cursor(&binding, offset, key),
        |selected, cursor| {
            serde_json::to_vec(&ActivityResultRef {
                requested_at: &requested_text,
                observed_at: &observed_text,
                rows: selected,
                next_cursor: cursor,
            })
            .map(|encoded| encoded.len())
            .map_err(|_error| PageError::ResultEncoding)
        },
    )
    .map_err(map_page_error)?;
    checkpoint(execution)?;
    Ok(ActivityResult {
        requested_at: requested_text,
        observed_at: Some(observed_text),
        rows: page.rows,
        next_cursor: page.next_cursor,
    })
}

#[derive(Serialize)]
struct ActivityResultRef<'a> {
    requested_at: &'a str,
    observed_at: &'a str,
    rows: &'a [ActivityRow],
    next_cursor: Option<&'a str>,
}

#[derive(Debug)]
struct RankedActivityRow {
    row: ActivityRow,
    coordinate: RowCoordinate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct RowCoordinate {
    source: usize,
    layout: u32,
    ordinal: u64,
}

fn validate_cursor_query(
    query: &ActivityQuery,
    cursor: &DecodedCursor,
) -> Result<(), ActivityError> {
    let binding = &cursor.binding;
    if binding.surface != PageSurface::Activity
        || binding.query_binding != query.query_binding
        || binding.page_size != query.page.page_size
        || binding.selected_at.is_none()
        || binding.source_pins.is_empty()
    {
        return Err(invalid_cursor());
    }
    Ok(())
}

fn select_observation(
    reader: &Reader,
    sources: &[SegmentRef],
    query: &ActivityQuery,
    execution: &Execution,
    continuation: bool,
) -> Result<Option<i64>, ActivityError> {
    let mut selected = None;
    for source in sources {
        checkpoint(execution)?;
        let segment = reader.open_segment(source).map_err(|_error| {
            if continuation {
                invalid_cursor()
            } else {
                read_failed()
            }
        })?;
        for type_id in ACTIVITY_LAYOUTS {
            if segment.rows_of(type_id).is_none() {
                continue;
            }
            let mut stop = None;
            let mut malformed = false;
            segment
                .visit_rows(type_id, &["ts"], 0, usize::MAX, |_ordinal, row| {
                    if let Err(error) = execution.checkpoint() {
                        stop = Some(error);
                        return false;
                    }
                    let Some(Cell::Ts(timestamp)) = row.get("ts") else {
                        malformed = true;
                        return false;
                    };
                    let wide = i128::from(*timestamp);
                    if wide >= query.hour_start_wide
                        && wide < query.hour_end_exclusive_wide
                        && *timestamp <= query.at
                    {
                        selected =
                            Some(selected.map_or(*timestamp, |before: i64| before.max(*timestamp)));
                    }
                    true
                })
                .map_err(|_error| {
                    if continuation {
                        invalid_cursor()
                    } else {
                        read_failed()
                    }
                })?;
            if let Some(stop) = stop {
                return Err(map_execution_stop(stop));
            }
            if malformed {
                return Err(if continuation {
                    invalid_cursor()
                } else {
                    read_failed()
                });
            }
        }
    }
    Ok(selected)
}

fn read_observation_rows(
    reader: &Reader,
    sources: &[SegmentRef],
    observed_at: i64,
    filter: &ActivityFilter,
    execution: &Execution,
    continuation: bool,
) -> Result<Vec<RankedActivityRow>, ActivityError> {
    let mut output = Vec::new();
    for (source_index, source) in sources.iter().enumerate() {
        checkpoint(execution)?;
        let segment = reader.open_segment(source).map_err(|_error| {
            if continuation {
                invalid_cursor()
            } else {
                read_failed()
            }
        })?;
        for type_id in ACTIVITY_LAYOUTS {
            let Some(row_count) = segment.rows_of(type_id) else {
                continue;
            };
            let Some(layout) = contract(type_id) else {
                return Err(read_failed());
            };
            let projection = ACTIVITY_COLUMNS
                .iter()
                .copied()
                .filter(|name| layout.column(name).is_some())
                .collect::<Vec<_>>();
            let mut offset = 0_u64;
            while offset < row_count {
                checkpoint(execution)?;
                let mut stop = None;
                let mut selected = Vec::new();
                let visited = segment
                    .visit_rows(type_id, &projection, offset, ROW_CHUNK, |ordinal, row| {
                        if let Err(error) = execution.checkpoint() {
                            stop = Some(error);
                            return false;
                        }
                        if row.get("ts") == Some(&Cell::Ts(observed_at)) {
                            selected.push((ordinal, row));
                        }
                        true
                    })
                    .map_err(|_error| {
                        if continuation {
                            invalid_cursor()
                        } else {
                            read_failed()
                        }
                    })?;
                if let Some(stop) = stop {
                    return Err(map_execution_stop(stop));
                }
                if visited == 0 {
                    return Err(if continuation {
                        invalid_cursor()
                    } else {
                        read_failed()
                    });
                }
                normalize_chunk(
                    &segment,
                    selected,
                    source_index,
                    type_id,
                    observed_at,
                    filter,
                    execution,
                    &mut output,
                )?;
                offset = offset
                    .checked_add(u64::try_from(visited).map_err(|_overflow| read_failed())?)
                    .ok_or_else(read_failed)?;
            }
        }
    }
    Ok(output)
}

#[expect(
    clippy::too_many_arguments,
    reason = "the bounded decode stage keeps source identity and product selection explicit"
)]
fn normalize_chunk(
    segment: &Segment,
    selected: Vec<(u64, Row)>,
    source_index: usize,
    type_id: u32,
    observed_at: i64,
    filter: &ActivityFilter,
    execution: &Execution,
    output: &mut Vec<RankedActivityRow>,
) -> Result<(), ActivityError> {
    let ids = selected
        .iter()
        .flat_map(|(_ordinal, row)| row.iter())
        .filter_map(|(_name, cell)| match cell {
            Cell::StrId(id) => Some(*id),
            Cell::I16(_)
            | Cell::I32(_)
            | Cell::I64(_)
            | Cell::U32(_)
            | Cell::U64(_)
            | Cell::F64(_)
            | Cell::Bool(_)
            | Cell::Ts(_)
            | Cell::ListI32(_)
            | Cell::Null => None,
        })
        .collect::<HashSet<_>>();
    let dictionary = segment
        .dictionary_for(&ids)
        .map_err(|_error| read_failed())?;
    for (ordinal, stored) in selected {
        checkpoint(execution)?;
        let row = normalize_row(&stored, &dictionary, observed_at)?;
        if filter.matches(&row) {
            output.push(RankedActivityRow {
                row,
                coordinate: RowCoordinate {
                    source: source_index,
                    layout: type_id,
                    ordinal,
                },
            });
        }
    }
    Ok(())
}

fn normalize_row(
    stored: &Row,
    dictionary: &Dictionary,
    observed_at: i64,
) -> Result<ActivityRow, ActivityError> {
    if required_ts(stored, "ts")? != observed_at {
        return Err(read_failed());
    }
    let pid = required_i32(stored, "pid")?;
    if pid <= 0 {
        return Err(read_failed());
    }
    let leader_pid = optional_i32(stored, "leader_pid")?;
    if leader_pid.is_some_and(|value| value <= 0) {
        return Err(read_failed());
    }
    let datid = optional_u32(stored, "datid")?;
    let datname = optional_text(stored, "datname", dictionary)?;
    let usename = optional_text(stored, "usename", dictionary)?;
    let application_name = required_text(stored, "application_name", dictionary)?;
    let client_addr = required_text(stored, "client_addr", dictionary)?;
    let backend_type = required_text(stored, "backend_type", dictionary)?;
    let state = optional_text(stored, "state", dictionary)?;
    let wait_event_type = optional_text(stored, "wait_event_type", dictionary)?;
    let wait_event = optional_text(stored, "wait_event", dictionary)?;
    let query_preview = optional_text(stored, "query", dictionary)?.map(shorten_query);
    let query_id = optional_i64(stored, "query_id")?;
    let backend_xid_age = optional_i64(stored, "backend_xid_age")?;
    let backend_xmin_age = optional_i64(stored, "backend_xmin_age")?;
    let backend_start = required_ts(stored, "backend_start")?;
    let xact_start = optional_ts(stored, "xact_start")?;
    let query_start = optional_ts(stored, "query_start")?;
    let state_change = optional_ts(stored, "state_change")?;
    let backend_age_ms = duration_ms(observed_at, Some(backend_start));
    let query_duration_ms = (state.as_deref() == Some("active"))
        .then(|| duration_ms(observed_at, query_start))
        .flatten();
    let transaction_duration_ms = duration_ms(observed_at, xact_start);
    let state_duration_ms = state
        .as_deref()
        .filter(|value| *value != "idle")
        .and_then(|_state| duration_ms(observed_at, state_change));
    Ok(ActivityRow {
        observed_at: observed_at.to_string(),
        pid,
        leader_pid,
        datid,
        datname,
        usename,
        application_name,
        client_addr,
        backend_type,
        state,
        wait_event_type,
        wait_event,
        query_preview,
        query_id: query_id.map(|value| value.to_string()),
        backend_xid_age: backend_xid_age.map(|value| value.to_string()),
        backend_xmin_age: backend_xmin_age.map(|value| value.to_string()),
        backend_start: backend_start.to_string(),
        xact_start: xact_start.map(|value| value.to_string()),
        query_start: query_start.map(|value| value.to_string()),
        state_change: state_change.map(|value| value.to_string()),
        backend_age_ms,
        query_duration_ms,
        transaction_duration_ms,
        state_duration_ms,
    })
}

fn required_i32(row: &Row, name: &str) -> Result<i32, ActivityError> {
    match row.get(name) {
        Some(Cell::I32(value)) => Ok(*value),
        Some(
            Cell::I16(_)
            | Cell::I64(_)
            | Cell::U32(_)
            | Cell::U64(_)
            | Cell::F64(_)
            | Cell::Bool(_)
            | Cell::Ts(_)
            | Cell::StrId(_)
            | Cell::ListI32(_)
            | Cell::Null,
        )
        | None => Err(read_failed()),
    }
}

fn optional_i32(row: &Row, name: &str) -> Result<Option<i32>, ActivityError> {
    match row.get(name) {
        None | Some(Cell::Null) => Ok(None),
        Some(Cell::I32(value)) => Ok(Some(*value)),
        Some(_) => Err(read_failed()),
    }
}

fn optional_u32(row: &Row, name: &str) -> Result<Option<u32>, ActivityError> {
    match row.get(name) {
        None | Some(Cell::Null) => Ok(None),
        Some(Cell::U32(value)) => Ok(Some(*value)),
        Some(_) => Err(read_failed()),
    }
}

fn optional_i64(row: &Row, name: &str) -> Result<Option<i64>, ActivityError> {
    match row.get(name) {
        None | Some(Cell::Null) => Ok(None),
        Some(Cell::I64(value)) => Ok(Some(*value)),
        Some(_) => Err(read_failed()),
    }
}

fn required_ts(row: &Row, name: &str) -> Result<i64, ActivityError> {
    match row.get(name) {
        Some(Cell::Ts(value)) => Ok(*value),
        Some(_) | None => Err(read_failed()),
    }
}

fn optional_ts(row: &Row, name: &str) -> Result<Option<i64>, ActivityError> {
    match row.get(name) {
        None | Some(Cell::Null) => Ok(None),
        Some(Cell::Ts(value)) => Ok(Some(*value)),
        Some(_) => Err(read_failed()),
    }
}

fn required_text(row: &Row, name: &str, dictionary: &Dictionary) -> Result<String, ActivityError> {
    match row.get(name) {
        Some(Cell::StrId(id)) => resolved_text(dictionary, *id),
        Some(_) | None => Err(read_failed()),
    }
}

fn optional_text(
    row: &Row,
    name: &str,
    dictionary: &Dictionary,
) -> Result<Option<String>, ActivityError> {
    match row.get(name) {
        None | Some(Cell::Null) => Ok(None),
        Some(Cell::StrId(id)) => resolved_text(dictionary, *id).map(Some),
        Some(_) => Err(read_failed()),
    }
}

fn resolved_text(dictionary: &Dictionary, id: u64) -> Result<String, ActivityError> {
    let bytes = match dictionary.resolve(id) {
        Some(Resolved::Str(bytes)) => bytes,
        Some(Resolved::Blob(blob)) => blob.stored_bytes,
        None => return Err(read_failed()),
    };
    std::str::from_utf8(bytes)
        .map(str::to_owned)
        .map_err(|_error| read_failed())
}

fn shorten_query(query: String) -> String {
    let mut characters = query.chars();
    let mut preview = characters.by_ref().take(160).collect::<String>();
    if characters.next().is_some() {
        preview.push('…');
    }
    preview
}

#[expect(
    clippy::cast_precision_loss,
    reason = "checked microsecond differences are intentionally published as fractional f64 milliseconds"
)]
fn duration_ms(observed_at: i64, start: Option<i64>) -> Option<f64> {
    let start = start.filter(|value| *value > 0)?;
    let delta = i128::from(observed_at).checked_sub(i128::from(start))?;
    (delta >= 0).then_some(delta as f64 / 1_000.0)
}

fn sort_rows(
    rows: &mut [RankedActivityRow],
    sort: ActivitySort,
    direction: Direction,
    execution: &Execution,
) -> Result<(), ActivityError> {
    checkpoint(execution)?;
    rows.sort_by(|left, right| {
        let primary = compare_primary(&left.row, &right.row, sort, direction);
        if primary != Ordering::Equal {
            return primary;
        }
        if sort == ActivitySort::QueryDurationMs && direction == Direction::Desc {
            let transaction = compare_nullable(
                left.row.transaction_duration_ms,
                right.row.transaction_duration_ms,
                Direction::Desc,
                f64::total_cmp,
            );
            if transaction != Ordering::Equal {
                return transaction;
            }
        }
        left.row
            .pid
            .cmp(&right.row.pid)
            .then_with(|| left.coordinate.cmp(&right.coordinate))
    });
    checkpoint(execution)
}

fn compare_primary(
    left: &ActivityRow,
    right: &ActivityRow,
    sort: ActivitySort,
    direction: Direction,
) -> Ordering {
    match sort {
        ActivitySort::Pid => directed(left.pid.cmp(&right.pid), direction),
        ActivitySort::Database => compare_nullable(
            left.datname.as_deref(),
            right.datname.as_deref(),
            direction,
            |left, right| left.cmp(right),
        ),
        ActivitySort::Role => compare_nullable(
            left.usename.as_deref(),
            right.usename.as_deref(),
            direction,
            |left, right| left.cmp(right),
        ),
        ActivitySort::QueryPreview => compare_nullable(
            left.query_preview.as_deref(),
            right.query_preview.as_deref(),
            direction,
            |left, right| left.cmp(right),
        ),
        ActivitySort::QueryDurationMs => compare_nullable(
            left.query_duration_ms,
            right.query_duration_ms,
            direction,
            f64::total_cmp,
        ),
        ActivitySort::TransactionDurationMs => compare_nullable(
            left.transaction_duration_ms,
            right.transaction_duration_ms,
            direction,
            f64::total_cmp,
        ),
        ActivitySort::Application => directed(
            left.application_name.cmp(&right.application_name),
            direction,
        ),
        ActivitySort::Client => directed(left.client_addr.cmp(&right.client_addr), direction),
        ActivitySort::State => compare_nullable(
            left.state.as_deref(),
            right.state.as_deref(),
            direction,
            |left, right| left.cmp(right),
        ),
        ActivitySort::WaitType => compare_nullable(
            left.wait_event_type.as_deref(),
            right.wait_event_type.as_deref(),
            direction,
            |left, right| left.cmp(right),
        ),
        ActivitySort::WaitEvent => compare_nullable(
            left.wait_event.as_deref(),
            right.wait_event.as_deref(),
            direction,
            |left, right| left.cmp(right),
        ),
        ActivitySort::BackendType => {
            directed(left.backend_type.cmp(&right.backend_type), direction)
        }
    }
}

fn compare_nullable<T: Copy>(
    left: Option<T>,
    right: Option<T>,
    direction: Direction,
    compare: impl FnOnce(&T, &T) -> Ordering,
) -> Ordering {
    match (left, right) {
        (Some(left), Some(right)) => directed(compare(&left, &right), direction),
        (Some(_), None) => Ordering::Less,
        (None, Some(_)) => Ordering::Greater,
        (None, None) => Ordering::Equal,
    }
}

const fn directed(ordering: Ordering, direction: Direction) -> Ordering {
    match direction {
        Direction::Asc => ordering,
        Direction::Desc => ordering.reverse(),
    }
}

fn normalize_filter(raw: Option<Vec<ActivityClauseArgs>>) -> Result<ActivityFilter, ActivityError> {
    let Some(raw) = raw else {
        return Ok(ActivityFilter::All);
    };
    if raw.len() > MAX_FILTER_CLAUSES {
        return Err(ActivityError::InvalidArguments(
            "filter accepts at most 18 clauses",
        ));
    }
    raw.into_iter()
        .map(normalize_clause)
        .collect::<Result<Vec<_>, _>>()
        .map(ActivityFilter::Clauses)
}

fn normalize_clause(raw: ActivityClauseArgs) -> Result<ActivityClause, ActivityError> {
    let property_count = [
        raw.text.is_some(),
        raw.pid.is_some(),
        raw.query_id.is_some(),
        raw.database.is_some(),
        raw.role.is_some(),
        raw.application.is_some(),
        raw.client.is_some(),
        raw.backend_type.is_some(),
        raw.state.is_some(),
        raw.wait_type.is_some(),
        raw.wait_event.is_some(),
    ]
    .into_iter()
    .filter(|present| *present)
    .count();
    if property_count == 0 || property_count > MAX_CLAUSE_PROPERTIES {
        return Err(ActivityError::InvalidArguments(
            "each filter clause must contain between 1 and 8 named fields",
        ));
    }
    let value_count = raw
        .text
        .as_ref()
        .map_or(0, TextMatchArgs::value_count)
        .saturating_add(raw.pid.as_ref().map_or(0, |value| value.any_of.len()))
        .saturating_add(raw.query_id.as_ref().map_or(0, |value| value.any_of.len()))
        .saturating_add(raw.database.as_ref().map_or(0, TextMatchArgs::value_count))
        .saturating_add(raw.role.as_ref().map_or(0, TextMatchArgs::value_count))
        .saturating_add(
            raw.application
                .as_ref()
                .map_or(0, TextMatchArgs::value_count),
        )
        .saturating_add(raw.client.as_ref().map_or(0, TextMatchArgs::value_count))
        .saturating_add(
            raw.backend_type
                .as_ref()
                .map_or(0, TextMatchArgs::value_count),
        )
        .saturating_add(raw.state.as_ref().map_or(0, TextMatchArgs::value_count))
        .saturating_add(raw.wait_type.as_ref().map_or(0, TextMatchArgs::value_count))
        .saturating_add(
            raw.wait_event
                .as_ref()
                .map_or(0, TextMatchArgs::value_count),
        );
    if value_count > MAX_CLAUSE_VALUES {
        return Err(ActivityError::InvalidArguments(
            "each filter clause accepts at most 8 listed values",
        ));
    }
    Ok(ActivityClause {
        text: raw.text.map(normalize_text_match).transpose()?,
        pid: raw.pid.map(normalize_pids).transpose()?,
        query_id: raw.query_id.map(normalize_query_ids).transpose()?,
        database: raw.database.map(normalize_text_match).transpose()?,
        role: raw.role.map(normalize_text_match).transpose()?,
        application: raw.application.map(normalize_text_match).transpose()?,
        client: raw.client.map(normalize_text_match).transpose()?,
        backend_type: raw.backend_type.map(normalize_text_match).transpose()?,
        state: raw.state.map(normalize_text_match).transpose()?,
        wait_type: raw.wait_type.map(normalize_text_match).transpose()?,
        wait_event: raw.wait_event.map(normalize_text_match).transpose()?,
    })
}

impl TextMatchArgs {
    fn value_count(&self) -> usize {
        self.any_of.as_ref().map_or(0, Vec::len) + self.all_of.as_ref().map_or(0, Vec::len)
    }
}

fn normalize_text_match(raw: TextMatchArgs) -> Result<TextMatch, ActivityError> {
    let any_of = normalize_patterns(raw.any_of)?;
    let all_of = normalize_patterns(raw.all_of)?;
    if any_of.is_empty() && all_of.is_empty() {
        return Err(ActivityError::InvalidArguments(
            "a text predicate requires any_of or all_of",
        ));
    }
    Ok(TextMatch { any_of, all_of })
}

fn normalize_patterns(raw: Option<Vec<String>>) -> Result<Vec<GlobPattern>, ActivityError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    if raw.is_empty() || raw.len() > MAX_CLAUSE_VALUES {
        return Err(ActivityError::InvalidArguments(
            "each pattern list must contain between 1 and 8 values",
        ));
    }
    raw.into_iter().map(GlobPattern::new).collect()
}

fn normalize_pids(raw: PidMatchArgs) -> Result<Vec<i32>, ActivityError> {
    if raw.any_of.is_empty() || raw.any_of.len() > MAX_CLAUSE_VALUES {
        return Err(ActivityError::InvalidArguments(
            "pid.any_of must contain between 1 and 8 values",
        ));
    }
    let values = raw
        .any_of
        .into_iter()
        .map(|value| {
            i32::try_from(value).ok().filter(|value| *value > 0).ok_or(
                ActivityError::InvalidArguments("PID values must be positive i32 integers"),
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    if has_duplicates(&values) {
        return Err(ActivityError::InvalidArguments(
            "pid.any_of values must be unique",
        ));
    }
    Ok(values)
}

fn normalize_query_ids(raw: QueryIdMatchArgs) -> Result<Vec<i64>, ActivityError> {
    if raw.any_of.is_empty() || raw.any_of.len() > MAX_CLAUSE_VALUES {
        return Err(ActivityError::InvalidArguments(
            "query_id.any_of must contain between 1 and 8 values",
        ));
    }
    let values = raw
        .any_of
        .iter()
        .map(|value| canonical_i64(value, "Query IDs must be canonical signed i64 decimal text"))
        .collect::<Result<Vec<_>, _>>()?;
    if has_duplicates(&values) {
        return Err(ActivityError::InvalidArguments(
            "query_id.any_of values must be unique",
        ));
    }
    Ok(values)
}

fn has_duplicates<T: Eq + std::hash::Hash>(values: &[T]) -> bool {
    let mut unique = HashSet::with_capacity(values.len());
    values.iter().any(|value| !unique.insert(value))
}

fn canonical_i64(raw: &str, message: &'static str) -> Result<i64, ActivityError> {
    if raw.is_empty() || raw.len() > 20 {
        return Err(ActivityError::InvalidArguments(message));
    }
    let value = raw
        .parse::<i64>()
        .map_err(|_error| ActivityError::InvalidArguments(message))?;
    if value.to_string() != raw {
        return Err(ActivityError::InvalidArguments(message));
    }
    Ok(value)
}

impl GlobPattern {
    fn new(raw: String) -> Result<Self, ActivityError> {
        if raw.chars().count() > MAX_PATTERN_SCALARS {
            return Err(ActivityError::InvalidArguments(
                "patterns accept at most 256 Unicode scalar values",
            ));
        }
        let source = raw.trim().to_owned();
        if source.is_empty() {
            return Err(ActivityError::InvalidArguments(
                "patterns must contain non-whitespace text",
            ));
        }
        let mut tokens = vec![GlobToken::Star];
        for character in source.chars() {
            let token = match character {
                '*' => GlobToken::Star,
                '?' => GlobToken::Any,
                literal => GlobToken::Literal(literal),
            };
            if token != GlobToken::Star || tokens.last() != Some(&GlobToken::Star) {
                tokens.push(token);
            }
        }
        if tokens.last() != Some(&GlobToken::Star) {
            tokens.push(GlobToken::Star);
        }
        Ok(Self { source, tokens })
    }

    fn matches(&self, candidate: &str) -> bool {
        let mut pattern_index = 0;
        let mut candidate_index = 0;
        let mut star = None;
        let mut retry = 0;
        while candidate_index < candidate.len() {
            let Some(character) = candidate
                .get(candidate_index..)
                .and_then(|remaining| remaining.chars().next())
            else {
                return false;
            };
            match self.tokens.get(pattern_index) {
                Some(GlobToken::Literal(wanted)) if unicode_char_equal(*wanted, character) => {
                    pattern_index += 1;
                    candidate_index += character.len_utf8();
                }
                Some(GlobToken::Any) => {
                    pattern_index += 1;
                    candidate_index += character.len_utf8();
                }
                Some(GlobToken::Star) => {
                    star = Some(pattern_index);
                    pattern_index += 1;
                    retry = candidate_index;
                }
                _ if let Some(star_index) = star => {
                    let Some(retry_character) = candidate
                        .get(retry..)
                        .and_then(|remaining| remaining.chars().next())
                    else {
                        return false;
                    };
                    retry += retry_character.len_utf8();
                    candidate_index = retry;
                    pattern_index = star_index + 1;
                }
                _ => return false,
            }
        }
        while self.tokens.get(pattern_index) == Some(&GlobToken::Star) {
            pattern_index += 1;
        }
        pattern_index == self.tokens.len()
    }
}

fn unicode_char_equal(left: char, right: char) -> bool {
    left.to_lowercase().eq(right.to_lowercase())
}

impl TextMatch {
    fn matches(&self, target: &str) -> bool {
        (self.any_of.is_empty() || self.any_of.iter().any(|pattern| pattern.matches(target)))
            && self.all_of.iter().all(|pattern| pattern.matches(target))
    }

    fn matches_fields(&self, fields: &[Option<&str>]) -> bool {
        (self.any_of.is_empty()
            || self
                .any_of
                .iter()
                .any(|pattern| fields.iter().flatten().any(|field| pattern.matches(field))))
            && self
                .all_of
                .iter()
                .all(|pattern| fields.iter().flatten().any(|field| pattern.matches(field)))
    }
}

impl ActivityFilter {
    fn matches(&self, row: &ActivityRow) -> bool {
        match self {
            Self::All => true,
            Self::Clauses(clauses) => clauses.iter().any(|clause| clause.matches(row)),
        }
    }
}

impl ActivityClause {
    fn matches(&self, row: &ActivityRow) -> bool {
        let text_fields = [
            row.query_preview.as_deref(),
            row.datname.as_deref(),
            row.usename.as_deref(),
            Some(row.application_name.as_str()),
            Some(row.client_addr.as_str()),
            row.state.as_deref(),
            row.wait_event_type.as_deref(),
            row.wait_event.as_deref(),
        ];
        self.text
            .as_ref()
            .is_none_or(|predicate| predicate.matches_fields(&text_fields))
            && self
                .pid
                .as_ref()
                .is_none_or(|values| values.contains(&row.pid))
            && self.query_id.as_ref().is_none_or(|values| {
                row.query_id
                    .as_deref()
                    .and_then(|value| value.parse::<i64>().ok())
                    .is_some_and(|value| values.contains(&value))
            })
            && named_text_matches(self.database.as_ref(), row.datname.as_deref())
            && named_text_matches(self.role.as_ref(), row.usename.as_deref())
            && named_text_matches(self.application.as_ref(), Some(&row.application_name))
            && named_text_matches(self.client.as_ref(), Some(&row.client_addr))
            && named_text_matches(self.backend_type.as_ref(), Some(&row.backend_type))
            && named_text_matches(self.state.as_ref(), row.state.as_deref())
            && named_text_matches(self.wait_type.as_ref(), row.wait_event_type.as_deref())
            && named_text_matches(self.wait_event.as_ref(), row.wait_event.as_deref())
    }
}

fn named_text_matches(predicate: Option<&TextMatch>, target: Option<&str>) -> bool {
    predicate.is_none_or(|predicate| target.is_some_and(|target| predicate.matches(target)))
}

fn checkpoint(execution: &Execution) -> Result<(), ActivityError> {
    execution.checkpoint().map_err(map_execution_stop)
}

const fn map_execution_stop(stop: ExecutionStop) -> ActivityError {
    match stop {
        ExecutionStop::Cancelled => ActivityError::Cancelled,
        ExecutionStop::DeadlineExceeded => ActivityError::DeadlineExceeded,
    }
}

const fn map_page_error(error: PageError) -> ActivityError {
    match error {
        PageError::InvalidCursor => invalid_cursor(),
        PageError::ResultTooLarge => ActivityError::ResultTooLarge,
        PageError::ResultEncoding => read_failed(),
    }
}

const fn invalid_cursor() -> ActivityError {
    ActivityError::InvalidArguments(
        "cursor is invalid, changed, expired, or bound to different arguments",
    )
}

const fn read_failed() -> ActivityError {
    ActivityError::ReadFailed("the recorded Activity data could not be read or normalized")
}

#[cfg(test)]
mod tests;
