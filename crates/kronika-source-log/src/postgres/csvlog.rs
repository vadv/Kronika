//! `log_destination = csvlog`.
//!
//! The first 23 columns have been fixed since PG 9.0; releases from PG 13 on
//! append `backend_type`, `leader_pid` and `query_id` after them, and reading
//! by position ignores whatever a newer release appends next.

use super::{PgRecord, Severity};
use crate::text::bounded;
use crate::timestamp;

const LOG_TIME: usize = 0;
const USER_NAME: usize = 1;
const DATABASE_NAME: usize = 2;
const ERROR_SEVERITY: usize = 11;
const SQL_STATE_CODE: usize = 12;
const MESSAGE: usize = 13;
const DETAIL: usize = 14;
const HINT: usize = 15;
const CONTEXT: usize = 18;
const QUERY: usize = 19;
/// Columns every supported release writes.
const MIN_FIELDS: usize = 23;

/// A record continues while a quoted field is still open, which is how a
/// statement with a newline in it reaches the log.
pub(super) const fn continues(_open: &[String], _line: &str, raw_quotes_odd: bool) -> bool {
    raw_quotes_odd
}

pub(super) fn parse(record: &str, now: i64) -> Option<PgRecord> {
    let fields = split(record);
    if fields.len() < MIN_FIELDS {
        return None;
    }
    let severity = Severity::parse(field(&fields, ERROR_SEVERITY)?)?;
    let message = field(&fields, MESSAGE)?;
    let ts = timestamp::parse_local(field(&fields, LOG_TIME)?).map_or(now, |(ts, _rest)| ts);
    let mut parsed = PgRecord::new(ts, severity, message);
    parsed.sqlstate = field(&fields, SQL_STATE_CODE).and_then(bounded);
    parsed.detail = field(&fields, DETAIL).and_then(bounded);
    parsed.hint = field(&fields, HINT).and_then(bounded);
    parsed.context = field(&fields, CONTEXT).and_then(bounded);
    parsed.statement = field(&fields, QUERY).and_then(bounded);
    parsed.database = field(&fields, DATABASE_NAME).and_then(bounded);
    parsed.username = field(&fields, USER_NAME).and_then(bounded);
    Some(parsed)
}

fn field(fields: &[String], index: usize) -> Option<&str> {
    fields.get(index).map(String::as_str)
}

/// Split one CSV record: fields are comma-separated, a quoted field runs to the
/// next lone `"`, and `""` inside one is a literal quote.
fn split(record: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut current = String::new();
    let mut quoted = false;
    let mut chars = record.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' if quoted && chars.peek() == Some(&'"') => {
                chars.next();
                current.push('"');
            }
            '"' => quoted = !quoted,
            ',' if !quoted => fields.push(std::mem::take(&mut current)),
            _ => current.push(c),
        }
    }
    fields.push(current);
    fields
}

#[cfg(test)]
mod tests;
