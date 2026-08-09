//! What a request asks for: the path, and the window it names.

/// The requests the server answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Route {
    /// The health line over a window.
    Health,
    /// The objects of one section, ordered by one column.
    Top(TopRequest),
    /// One column of one object over time.
    Series(SeriesRequest),
    /// The rows of one section, as they were recorded.
    Rows(RowsRequest),
}

/// What a request for one object's history names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SeriesRequest {
    /// Which section to read.
    pub(crate) section: u32,
    /// Which of its numbers to follow.
    pub(crate) column: String,
    /// Label columns the rows have to match, as the request wrote them.
    pub(crate) filters: Vec<(String, String)>,
}

/// What a request for raw rows names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RowsRequest {
    /// Which section to read.
    pub(crate) section: u32,
    /// How many rows to answer with.
    pub(crate) limit: usize,
    /// Label columns the rows have to match.
    pub(crate) filters: Vec<(String, String)>,
}

/// What a request for the objects of a section names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct TopRequest {
    /// Which section to read.
    pub(crate) section: u32,
    /// Which of its numbers to order by.
    pub(crate) column: String,
    /// How many objects to answer with.
    pub(crate) limit: usize,
    /// How many columns to split the window into. One is a plain ordering.
    pub(crate) buckets: usize,
}

/// Objects answered when the request does not say.
const DEFAULT_LIMIT: usize = 20;

/// Most objects one answer carries. A dashboard draws rows; a request for a
/// hundred thousand of them is a mistake, and a slow one.
const MAX_LIMIT: usize = 1_000;

/// Most columns a window is split into.
const MAX_BUCKETS: usize = 1_000;

/// The window a request names, in unix microseconds.
///
/// An absent bound means everything on that side. The server does not invent
/// one from the clock: a request that wants the last hour says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Window {
    /// Oldest timestamp to include.
    pub(crate) from: Option<i64>,
    /// Newest timestamp to include.
    pub(crate) to: Option<i64>,
}

/// Why a request was refused before it reached the data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum RouteError {
    /// No handler answers that path.
    NoSuchPath,
    /// A parameter is present and unusable, or required and absent.
    BadParameter(String),
}

impl std::fmt::Display for RouteError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NoSuchPath => write!(f, "no such path"),
            Self::BadParameter(name) => write!(f, "{name} is not a number"),
        }
    }
}

/// Read the route and its window out of a request target.
///
/// # Errors
///
/// Returns [`RouteError::NoSuchPath`] for a path nothing answers, and
/// [`RouteError::BadParameter`] for a bound that is not a number.
pub(crate) fn parse(target: &str) -> Result<(Route, Window), RouteError> {
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let mut window = Window::default();
    let mut section: Option<u32> = None;
    let mut column: Option<String> = None;
    let mut limit = DEFAULT_LIMIT;
    let mut buckets = 1;
    let mut filters: Vec<(String, String)> = Vec::new();
    for (name, value) in pairs(query) {
        match name {
            "from" => window.from = Some(number("from", value)?),
            "to" => window.to = Some(number("to", value)?),
            "section" => section = Some(type_id(value)?),
            "column" => column = Some(value.to_owned()),
            "limit" => limit = count("limit", value, MAX_LIMIT)?,
            "buckets" => buckets = count("buckets", value, MAX_BUCKETS)?.max(1),
            // Anything else names a label column and the value it must hold.
            // The server does not decide which label is the right one to filter
            // on; the request says, and the rows either match or do not.
            label => filters.push((label.to_owned(), decoded(value))),
        }
    }
    let route = match path {
        "/api/health" => Route::Health,
        "/api/top" => Route::Top(TopRequest {
            section: section.ok_or_else(|| RouteError::BadParameter("section".to_owned()))?,
            column: column.ok_or_else(|| RouteError::BadParameter("column".to_owned()))?,
            limit,
            buckets,
        }),
        "/api/series" => Route::Series(SeriesRequest {
            section: section.ok_or_else(|| RouteError::BadParameter("section".to_owned()))?,
            column: column.ok_or_else(|| RouteError::BadParameter("column".to_owned()))?,
            filters,
        }),
        "/api/rows" => Route::Rows(RowsRequest {
            section: section.ok_or_else(|| RouteError::BadParameter("section".to_owned()))?,
            limit,
            filters,
        }),
        _other => return Err(RouteError::NoSuchPath),
    };
    Ok((route, window))
}

/// The `name=value` pairs of a query string, in the order they were written.
fn pairs(query: &str) -> impl Iterator<Item = (&str, &str)> {
    query
        .split('&')
        .filter(|part| !part.is_empty())
        .map(|part| part.split_once('=').unwrap_or((part, "")))
}

fn number(name: &str, value: &str) -> Result<i64, RouteError> {
    value
        .parse()
        .map_err(|_reason| RouteError::BadParameter(name.to_owned()))
}

/// A query value with its percent escapes and pluses undone.
///
/// A mount point and a command line both carry characters a query string has
/// to escape, and a filter has to compare against what the segment recorded.
fn decoded(value: &str) -> String {
    let bytes = value.replace('+', " ").into_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%'
            && let Some(hex) = bytes.get(at + 1..at + 3)
            && let Ok(text) = std::str::from_utf8(hex)
            && let Ok(byte) = u8::from_str_radix(text, 16)
        {
            out.push(byte);
            at += 3;
            continue;
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// A section id, which is a `type_id` of the registry.
fn type_id(value: &str) -> Result<u32, RouteError> {
    value
        .parse()
        .map_err(|_reason| RouteError::BadParameter("section".to_owned()))
}

/// A count, refused rather than clamped when it runs past what is answerable.
fn count(name: &str, value: &str, most: usize) -> Result<usize, RouteError> {
    let parsed: usize = value
        .parse()
        .map_err(|_reason| RouteError::BadParameter(name.to_owned()))?;
    if parsed > most {
        return Err(RouteError::BadParameter(name.to_owned()));
    }
    Ok(parsed)
}

#[cfg(test)]
mod tests;
