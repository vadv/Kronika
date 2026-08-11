//! Strict parsing of the four resource families.

const DEFAULT_PAGE_SIZE: usize = 100;
const MAX_PAGE_SIZE: usize = 1_000;
const MAX_QUERY_BYTES: usize = 64 * 1024;
const MAX_SECTION_BYTES: usize = 128;
/// A screen draws a handful of tables; more than this is not a screen.
const MAX_SNAPSHOT_SECTIONS: usize = 16;
const MAX_FIELDS: usize = 256;
const MAX_FILTERS: usize = 64;

/// The requests the server answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// Actual finished/current segment catalog.
    Catalog(Window),
    /// One hour of the timeline: its segments, health line and marks.
    Hour(Window),
    /// One logical indexed series in one explicit segment.
    Index(SegmentRequest),
    /// Projected full-resolution history in one explicit segment.
    History(DataRequest),
    /// One stable page of physical rows in one explicit segment.
    Rows(RowsRequest),
    /// One moment of several sections, counters already turned into rates.
    Snapshot(SnapshotRequest),
}

/// One moment of one explicit segment, across the sections a screen draws.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SnapshotRequest {
    pub(crate) segment_id: i64,
    pub(crate) at: i64,
    pub(crate) sections: Vec<String>,
}

/// Optional timestamp bounds for catalog listing only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Window {
    pub(crate) from: Option<i64>,
    pub(crate) to: Option<i64>,
}

/// One logical section in one explicit segment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SegmentRequest {
    pub(crate) segment_id: i64,
    pub(crate) section: String,
}

/// One exact typed equality predicate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Filter {
    pub(crate) column: String,
    pub(crate) value: String,
}

/// Committed prefix of one active segment.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ActiveCursor {
    pub(crate) segment_id: i64,
    pub(crate) wal_position: u64,
}

/// Projection and predicates shared by history and rows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DataRequest {
    pub(crate) segment: SegmentRequest,
    pub(crate) fields: Vec<String>,
    pub(crate) filters: Vec<Filter>,
    pub(crate) after: Option<ActiveCursor>,
}

/// Physical row ordering.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Order {
    Asc,
    Desc,
}

/// Bounded page request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowsRequest {
    pub(crate) data: DataRequest,
    pub(crate) order: Order,
    pub(crate) page_size: usize,
    pub(crate) cursor: Option<String>,
}

/// Why a request was refused before touching disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteError {
    /// No resource has this path shape.
    NoSuchPath,
    /// One named path/query field is absent, duplicated, or invalid.
    BadParameter(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchPath => write!(f, "no such path"),
            Self::BadParameter(name) => write!(f, "invalid parameter {name}"),
        }
    }
}

/// Parse a path and optional query into a typed resource request.
///
/// # Errors
///
/// Returns a path or named-parameter refusal; malformed percent escapes and
/// non-UTF-8 data are always refused.
pub(crate) fn parse(path: &str, query: Option<&str>) -> Result<Route, RouteError> {
    let query = query.unwrap_or("");
    if query.len() > MAX_QUERY_BYTES {
        return Err(RouteError::BadParameter("query".to_owned()));
    }
    if path == "/api/catalog" {
        return parse_catalog(query).map(Route::Catalog);
    }
    if path == "/api/hour" {
        return parse_catalog(query).map(Route::Hour);
    }
    let tail = path
        .strip_prefix("/api/segments/")
        .ok_or(RouteError::NoSuchPath)?;
    let pieces: Vec<&str> = tail.split('/').collect();
    if pieces.len() == 2 && pieces[1] == "snapshot" && !pieces[0].is_empty() {
        return parse_snapshot(number("segment_id", pieces[0])?, query).map(Route::Snapshot);
    }
    if pieces.len() != 4 || pieces[1] != "sections" || pieces.iter().any(|piece| piece.is_empty()) {
        return Err(RouteError::NoSuchPath);
    }
    let segment_id = number("segment_id", pieces[0])?;
    let section = decoded("section", pieces[2], false)?;
    if section.is_empty() || section.len() > MAX_SECTION_BYTES {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    let segment = SegmentRequest {
        segment_id,
        section,
    };
    match pieces[3] {
        "index" if query.is_empty() => Ok(Route::Index(segment)),
        "index" => Err(RouteError::BadParameter("query".to_owned())),
        "history" => parse_data(segment, query).map(Route::History),
        "rows" => parse_rows(segment, query).map(Route::Rows),
        _ => Err(RouteError::NoSuchPath),
    }
}

fn parse_snapshot(segment_id: i64, query: &str) -> Result<SnapshotRequest, RouteError> {
    let mut at = None;
    let mut sections = Vec::new();
    for (raw_name, raw_value) in pairs(query)? {
        match raw_name {
            "at" if at.is_none() => at = Some(number("at", raw_value)?),
            "section" => {
                let section = decoded("section", raw_value, false)?;
                if section.is_empty() || section.len() > MAX_SECTION_BYTES {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                if sections.contains(&section) || sections.len() >= MAX_SNAPSHOT_SECTIONS {
                    return Err(RouteError::BadParameter("section".to_owned()));
                }
                sections.push(section);
            }
            _other => return Err(RouteError::BadParameter(raw_name.to_owned())),
        }
    }
    if sections.is_empty() {
        return Err(RouteError::BadParameter("section".to_owned()));
    }
    Ok(SnapshotRequest {
        segment_id,
        at: at.ok_or_else(|| RouteError::BadParameter("at".to_owned()))?,
        sections,
    })
}

fn parse_catalog(query: &str) -> Result<Window, RouteError> {
    let mut window = Window::default();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        match name.as_str() {
            "from" if window.from.is_none() => window.from = Some(number("from", &value)?),
            "to" if window.to.is_none() => window.to = Some(number("to", &value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    if window
        .from
        .zip(window.to)
        .is_some_and(|(from, to)| from > to)
    {
        return Err(RouteError::BadParameter("from".to_owned()));
    }
    Ok(window)
}

fn parse_data(segment: SegmentRequest, query: &str) -> Result<DataRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut after = None;
    for (name, value) in extras {
        match name.as_str() {
            "after" if after.is_none() => after = Some(active_cursor(&value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(DataRequest {
        segment,
        fields,
        filters,
        after,
    })
}

fn parse_rows(segment: SegmentRequest, query: &str) -> Result<RowsRequest, RouteError> {
    let (fields, filters, extras) = data_parameters(query)?;
    let mut order = Order::Asc;
    let mut page_size = DEFAULT_PAGE_SIZE;
    let mut cursor = None;
    let mut after = None;
    let mut saw_order = false;
    let mut saw_page_size = false;
    for (name, value) in extras {
        match name.as_str() {
            "order" if !saw_order => {
                order = match value.as_str() {
                    "asc" => Order::Asc,
                    "desc" => Order::Desc,
                    _ => return Err(RouteError::BadParameter(name)),
                };
                saw_order = true;
            }
            "page_size" if !saw_page_size => {
                page_size = value
                    .parse::<usize>()
                    .ok()
                    .filter(|size| (1..=MAX_PAGE_SIZE).contains(size))
                    .ok_or_else(|| RouteError::BadParameter(name.clone()))?;
                saw_page_size = true;
            }
            "cursor" if cursor.is_none() && !value.is_empty() => cursor = Some(value),
            "after" if after.is_none() => after = Some(active_cursor(&value)?),
            _ => return Err(RouteError::BadParameter(name)),
        }
    }
    validate_after(&segment, after)?;
    Ok(RowsRequest {
        data: DataRequest {
            segment,
            fields,
            filters,
            after,
        },
        order,
        page_size,
        cursor,
    })
}

fn active_cursor(value: &str) -> Result<ActiveCursor, RouteError> {
    let (segment_id, wal_position) = value
        .split_once(',')
        .filter(|(segment_id, wal_position)| {
            !segment_id.is_empty() && !wal_position.is_empty() && !wal_position.contains(',')
        })
        .ok_or_else(|| RouteError::BadParameter("after".to_owned()))?;
    Ok(ActiveCursor {
        segment_id: number("after", segment_id)?,
        wal_position: wal_position
            .parse()
            .map_err(|_error| RouteError::BadParameter("after".to_owned()))?,
    })
}

fn validate_after(segment: &SegmentRequest, after: Option<ActiveCursor>) -> Result<(), RouteError> {
    if after.is_some_and(|cursor| cursor.segment_id != segment.segment_id) {
        return Err(RouteError::BadParameter("after".to_owned()));
    }
    Ok(())
}

type Extra = (String, String);
type DataParameters = (Vec<String>, Vec<Filter>, Vec<Extra>);

fn data_parameters(query: &str) -> Result<DataParameters, RouteError> {
    let mut fields = Vec::new();
    let mut filters = Vec::new();
    let mut extras = Vec::new();
    for (raw_name, raw_value) in pairs(query)? {
        let name = decoded("parameter", raw_name, true)?;
        let value = decoded(&name, raw_value, true)?;
        if name == "field" {
            if value.is_empty() || fields.len() >= MAX_FIELDS || fields.contains(&value) {
                return Err(RouteError::BadParameter("field".to_owned()));
            }
            fields.push(value);
        } else if let Some(column) = name.strip_prefix("where.") {
            if column.is_empty()
                || filters.len() >= MAX_FILTERS
                || filters
                    .iter()
                    .any(|filter: &Filter| filter.column == column)
            {
                return Err(RouteError::BadParameter("where".to_owned()));
            }
            filters.push(Filter {
                column: column.to_owned(),
                value,
            });
        } else {
            extras.push((name, value));
        }
    }
    Ok((fields, filters, extras))
}

fn pairs(query: &str) -> Result<Vec<(&str, &str)>, RouteError> {
    if query.is_empty() {
        return Ok(Vec::new());
    }
    query
        .split('&')
        .map(|part| {
            if part.is_empty() {
                return Err(RouteError::BadParameter("query".to_owned()));
            }
            Ok(part.split_once('=').unwrap_or((part, "")))
        })
        .collect()
}

fn decoded(name: &str, value: &str, plus_as_space: bool) -> Result<String, RouteError> {
    let bytes = value.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0_usize;
    while at < bytes.len() {
        match bytes[at] {
            b'+' if plus_as_space => {
                out.push(b' ');
                at += 1;
            }
            b'%' => {
                let raw = bytes
                    .get(at + 1..at + 3)
                    .ok_or_else(|| RouteError::BadParameter(name.to_owned()))?;
                let text = std::str::from_utf8(raw)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                let byte = u8::from_str_radix(text, 16)
                    .map_err(|_error| RouteError::BadParameter(name.to_owned()))?;
                out.push(byte);
                at += 3;
            }
            byte => {
                out.push(byte);
                at += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

fn number(name: &str, value: &str) -> Result<i64, RouteError> {
    value
        .parse()
        .map_err(|_error| RouteError::BadParameter(name.to_owned()))
}

#[cfg(test)]
mod tests;
