//! `log_destination = stderr`.
//!
//! A record is the line carrying `SEVERITY:  `, followed by the `DETAIL:`,
//! `HINT:`, `CONTEXT:`, `STATEMENT:` and `QUERY:` lines the server writes after
//! it, each of which repeats the `log_line_prefix`. Inside one of those, a line
//! the server wrapped starts with a tab.

use super::prefix::LinePrefix;
use super::{PgRecord, Severity};
use crate::tail::Record;
use crate::text::{append, append_str};

/// Severity markers, longest first so `WARNING` is not read as the tail of a
/// message that happens to contain `LOG:  `.
const SEVERITIES: &[(&str, Severity)] = &[
    ("PANIC:  ", Severity::Panic),
    ("FATAL:  ", Severity::Fatal),
    ("ERROR:  ", Severity::Error),
    ("WARNING:  ", Severity::Warning),
    ("LOG:  ", Severity::Log),
];

/// What follows a record, as its own line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Part {
    Detail,
    Hint,
    Context,
    Statement,
    Query,
}

const PARTS: &[(&str, Part)] = &[
    ("DETAIL:  ", Part::Detail),
    ("HINT:  ", Part::Hint),
    ("CONTEXT:  ", Part::Context),
    ("STATEMENT:  ", Part::Statement),
    ("QUERY:  ", Part::Query),
];

/// A wrapped line starts with a tab; a `DETAIL:` and its kin carry the prefix
/// again and are found by their marker.
pub(super) fn continues(_open: &[String], line: &str, _raw_quotes_odd: bool) -> bool {
    line.starts_with('\t') || PARTS.iter().any(|(marker, _part)| line.contains(marker))
}

pub(super) fn parse(record: &Record, prefix: Option<&LinePrefix>, now: i64) -> Option<PgRecord> {
    let first = record.first();
    let (at, marker, severity) = find_marker(first, SEVERITIES)?;
    let head = first.get(..at)?;
    let (sqlstate, message) = strip_sqlstate(first.get(at + marker.len()..)?.trim());
    let fields = prefix.map(|prefix| prefix.read(head)).unwrap_or_default();

    let mut parsed = PgRecord::new(
        fields
            .ts
            .or_else(|| crate::timestamp::parse_local(head).map(|(ts, _rest)| ts))
            .unwrap_or(now),
        severity,
        message,
    );
    parsed.sqlstate = sqlstate.map(str::to_owned);
    parsed.database = fields.database;
    parsed.username = fields.username;

    let mut open: Option<Part> = None;
    for line in record.rest() {
        if let Some(text) = line.strip_prefix('\t') {
            extend(&mut parsed, open, text);
            continue;
        }
        let Some((at, marker, part)) = find_marker(line, PARTS) else {
            continue;
        };
        open = Some(part);
        if let Some(text) = line.get(at + marker.len()..) {
            extend(&mut parsed, open, text);
        }
    }
    Some(parsed)
}

/// Add a continuation's text to the field it belongs to; a wrapped line with no
/// marker before it extends the message.
fn extend(parsed: &mut PgRecord, part: Option<Part>, text: &str) {
    match part {
        Some(Part::Detail) => append(&mut parsed.detail, text),
        Some(Part::Hint) => append(&mut parsed.hint, text),
        Some(Part::Context) => append(&mut parsed.context, text),
        // `QUERY:` is the internal form of the statement; both name the SQL the
        // record was raised under and neither has a column of its own.
        Some(Part::Statement | Part::Query) => append(&mut parsed.statement, text),
        None => append_str(&mut parsed.message, text),
    }
}

/// The earliest marker in `line`, so text quoting a marker cannot pose as one.
fn find_marker<T: Copy>(
    line: &str,
    markers: &[(&'static str, T)],
) -> Option<(usize, &'static str, T)> {
    markers
        .iter()
        .filter_map(|(marker, value)| line.find(marker).map(|at| (at, *marker, *value)))
        .min_by_key(|(at, _marker, _value)| *at)
}

/// Strip the `42P01: ` a `log_error_verbosity = verbose` record starts with.
fn strip_sqlstate(message: &str) -> (Option<&str>, &str) {
    let bytes = message.as_bytes();
    let is_code = bytes.len() >= 7
        && bytes.get(..5).is_some_and(|code| {
            code.iter()
                .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
        })
        && bytes.get(5) == Some(&b':')
        && bytes.get(6) == Some(&b' ');
    if is_code {
        return (
            message.get(..5),
            message.get(6..).unwrap_or_default().trim_start(),
        );
    }
    (None, message)
}
