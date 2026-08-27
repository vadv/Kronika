//! Row identity carried alongside `kronika_get_row_detail` locators.

use serde_json::{Map, Value};

/// The column whose value pins a section's row to one object; binding
/// columns (`dbid`, `userid`, `toplevel` and alike) are reported in the
/// row but not checked. Segment finalization reorders a section's rows,
/// so an ordinal alone can drift onto a neighbouring row with the same
/// timestamp. Locator-emitting sections absent here keep one row per
/// timestamp, and `at` pins them alone.
pub(super) fn discriminator(section: &str) -> Option<&'static str> {
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

/// Copies the discriminator value into a `row_key` field. A missing
/// column or a null value attaches nothing: such a row has no key, and
/// `verify` accepts it without one.
pub(super) fn attach(section: &str, object: &mut Map<String, Value>) {
    let value = discriminator(section)
        .and_then(|column| object.get(column))
        .filter(|value| !value.is_null())
        .cloned();
    if let Some(value) = value {
        object.insert("row_key".to_owned(), value);
    }
}

/// Checks a requested `row_key` against the fetched row's discriminator
/// value; the error text is the tool answer.
pub(super) fn verify(
    section: &str,
    column: &str,
    requested: Option<&Value>,
    actual: &Value,
) -> Result<(), String> {
    match requested {
        None if actual.is_null() => Ok(()),
        None => Err(format!(
            "row_key is required for {section}: copy the row_key value from the find row, \
             or re-run the find tool if that row carried none"
        )),
        Some(expected) if matches(expected, actual) => Ok(()),
        Some(expected) => Err(format!(
            "stale locator: the row at this ordinal has {column}={}, not the requested \
             row_key {}; segment finalization reorders rows — re-run the find tool for \
             fresh locators",
            shown(actual),
            shown(expected),
        )),
    }
}

/// Equality across the number and decimal-string renderings of one value.
pub(super) fn matches(expected: &Value, actual: &Value) -> bool {
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
