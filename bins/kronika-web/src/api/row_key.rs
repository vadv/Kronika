//! Row identity carried alongside exact physical row locators.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest as _, Sha256};

/// Ready-to-use input for `kronika_get_row_detail`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetailLocator {
    /// Recorded logical section.
    pub(crate) section: String,
    /// Signed 64-bit segment ID as a JSON integer or decimal string.
    pub(crate) segment_id: Value,
    /// Signed 64-bit row timestamp as a JSON integer or decimal string.
    pub(crate) at: Value,
    /// Unsigned 32-bit physical layout ID as a JSON integer or decimal string.
    pub(crate) type_id: Value,
    /// Unsigned 64-bit row position as a JSON integer or decimal string.
    pub(crate) row_ordinal: Value,
    /// Row identity copied with the physical coordinates when one is needed.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub(crate) row_key: Option<Value>,
}

impl DetailLocator {
    pub(crate) fn new(
        section: &str,
        segment_id: i64,
        at: i64,
        type_id: u32,
        row_ordinal: u64,
        row_key: Option<Value>,
    ) -> Self {
        Self {
            section: section.to_owned(),
            segment_id: Value::String(segment_id.to_string()),
            at: Value::String(at.to_string()),
            type_id: Value::String(type_id.to_string()),
            row_ordinal: Value::String(row_ordinal.to_string()),
            row_key,
        }
    }
}

/// Builds the exact physical locator for one rendered row.
pub(crate) fn detail_locator(
    section: &str,
    segment_id: i64,
    at: i64,
    type_id: u32,
    row_ordinal: u64,
    fields: &Map<String, Value>,
) -> DetailLocator {
    DetailLocator::new(
        section,
        segment_id,
        at,
        type_id,
        row_ordinal,
        discriminator(section)
            .and_then(|column| fields.get(column))
            .and_then(|value| key_value(section, value)),
    )
}

/// Stored text kept out of mass results and returned only by row detail.
pub(crate) fn is_detail_text(section: &str, field: &str) -> bool {
    matches!(
        (section, field),
        ("os_process", "cmdline")
            | (
                "pg_stat_activity" | "pg_locks" | "pg_stat_statements",
                "query"
            )
            | ("pg_store_plans", "plan")
            | (
                "pg_log_errors",
                "sample" | "detail" | "hint" | "context" | "statement"
            )
            | ("pg_log_slow_queries", "sample")
            | ("pg_log_checkpoints", "reason")
            | ("pg_log_lock_waits", "detail" | "context" | "statement")
            | ("pg_log_temp_files", "statement")
            | ("pg_log_lifecycle", "message" | "query_detail")
            | ("pgbouncer_events", "text")
    )
}

/// The column whose value pins a section's row to one object; binding
/// columns (`dbid`, `userid`, `toplevel` and alike) are reported in the
/// row but not checked. Segment finalization reorders a section's rows,
/// so an ordinal alone can drift onto a neighbouring row with the same
/// timestamp. Locator-emitting sections absent here keep one row per
/// timestamp, and `at` pins them alone.
pub(crate) fn discriminator(section: &str) -> Option<&'static str> {
    match section {
        "pg_stat_statements" => Some("queryid"),
        "pg_store_plans" => Some("planid"),
        "pg_stat_activity" | "pg_stat_progress_vacuum" | "pg_locks" | "pg_log_lock_waits" => {
            Some("pid")
        }
        "pg_stat_database" => Some("datid"),
        "pg_settings" => Some("name"),
        "os_process" => Some("pid"),
        "pg_log_errors" | "pg_log_slow_queries" => Some("pattern"),
        "pg_log_checkpoints" => Some("phase"),
        "pg_log_autovacuum" => Some("relation"),
        "pg_log_temp_files" => Some("size_bytes"),
        "pg_log_lifecycle" => Some("kind"),
        "pgbouncer_events" => Some("text"),
        _ => None,
    }
}

/// Attaches a safe discriminator value as `row_key`; raw-text keys use their
/// digest. A missing column or null value attaches nothing: such a row has no
/// key, and `verify` accepts it without one.
pub(crate) fn attach(section: &str, object: &mut Map<String, Value>) {
    let value = discriminator(section)
        .and_then(|column| object.get(column))
        .and_then(|value| key_value(section, value));
    if let Some(value) = value {
        object.insert("row_key".to_owned(), value);
    }
}

/// Checks a requested `row_key` against the fetched row's discriminator
/// value; the error text is the tool answer.
pub(crate) fn verify(
    section: &str,
    column: &str,
    requested: Option<&Value>,
    actual: &Value,
) -> Result<(), String> {
    let actual_key = key_value(section, actual);
    match requested {
        None if actual_key.is_none() => Ok(()),
        None => Err(format!(
            "row_key is required for {section}: copy the row_key value from the find row, \
             or re-run the find tool if that row carried none"
        )),
        Some(expected)
            if actual_key
                .as_ref()
                .is_some_and(|actual| matches(expected, actual)) =>
        {
            Ok(())
        }
        Some(expected) => Err(format!(
            "stale locator: the row at this ordinal has {column}={}, not the requested \
             row_key {}; segment finalization reorders rows — re-run the find tool for \
             fresh locators",
            actual_key.as_ref().map_or_else(|| "null".to_owned(), shown),
            shown(expected),
        )),
    }
}

fn key_value(section: &str, value: &Value) -> Option<Value> {
    if value.is_null() {
        return None;
    }
    if discriminator(section).is_some_and(|column| is_detail_text(section, column)) {
        return Some(Value::String(format!("sha256:{}", text_sha256(value))));
    }
    Some(value.clone())
}

fn text_sha256(value: &Value) -> String {
    if let Value::Object(object) = value {
        if let Some(hash) = object.get("sha256").and_then(Value::as_str) {
            return hash.to_owned();
        }
        if let Some(text) = object.get("stored_text").and_then(Value::as_str) {
            return format!("{:x}", Sha256::digest(text.as_bytes()));
        }
    }
    let text = value
        .as_str()
        .map_or_else(|| comparable(value), str::to_owned);
    format!("{:x}", Sha256::digest(text.as_bytes()))
}

/// Equality across the number and decimal-string renderings of one value.
pub(crate) fn matches(expected: &Value, actual: &Value) -> bool {
    comparable(expected) == comparable(actual)
}

fn comparable(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        other => other.to_string(),
    }
}

/// At most 120 characters of a value in an error text: `pattern` and
/// `pgbouncer_events.text` keys reach kilobytes.
fn shown(value: &Value) -> String {
    let text = comparable(value);
    if text.chars().count() <= 120 {
        return text;
    }
    let cut: String = text.chars().take(120).collect();
    format!("{cut}…")
}

#[cfg(test)]
mod tests;
