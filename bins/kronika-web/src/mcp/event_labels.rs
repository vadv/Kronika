//! Human-readable labels for the Kronika-invented numeric codes on the
//! seven event-shaped log sections `kronika_find_events` and
//! `kronika_get_row_detail` both render. The codes are not raw `PostgreSQL`
//! or `PgBouncer` values and are not even a monotonic severity ordering (a
//! `pg_log_errors` WARNING is numerically 3, higher than ERROR's 0), so a
//! tool-calling model has no safe way to guess them. Before this module,
//! the only decoding lived in `ui/src/events-format.ts`'s
//! `enumValueKey`/`categoryLabel`, reachable only by the browser UI.
//!
//! Label strings are copied verbatim from that file's own tables so the
//! MCP surface and the browser UI never disagree. The numeric field stays
//! untouched — a caller doing numeric comparison keeps working — and a
//! `<field>_label` sibling is added next to it.

use serde_json::{Map, Value, json};

/// Adds a `<field>_label` sibling for every numeric code field `section`
/// carries. A no-op for any other section (including `pg_log_slow_queries`,
/// one of the seven event sections but with no enum-coded field of its
/// own), for a field missing from `fields`, and for a code outside its
/// label table — the same "stays missing, does not guess" treatment the
/// rest of this codebase gives an absent or unrecognized value.
pub(crate) fn label_event_fields(section: &str, fields: &mut Map<String, Value>) {
    match section {
        "pg_log_errors" => {
            add_label(fields, "severity", SEVERITY_LABELS);
            add_label(fields, "category", CATEGORY_LABELS);
        }
        "pg_log_checkpoints" => add_label(fields, "phase", CHECKPOINT_PHASE_LABELS),
        "pg_log_autovacuum" => add_label(fields, "kind", AUTOVACUUM_KIND_LABELS),
        "pg_log_lock_waits" => add_label(fields, "kind", LOCK_WAIT_KIND_LABELS),
        "pg_log_lifecycle" => add_label(fields, "kind", LIFECYCLE_KIND_LABELS),
        "pgbouncer_events" => add_label(fields, "level", PGBOUNCER_LEVEL_LABELS),
        _ => {}
    }
}

// Verbatim from `ui/src/events-format.ts`'s `enumValueKey`/`ERROR_CATEGORIES`.
const SEVERITY_LABELS: &[&str] = &["error", "fatal", "panic", "warning", "log"];
const CATEGORY_LABELS: &[&str] = &[
    "lock",
    "constraint",
    "serialization",
    "timeout",
    "resource",
    "data_corruption",
    "system",
    "connection",
    "auth",
    "syntax",
    "other",
];
const CHECKPOINT_PHASE_LABELS: &[&str] = &["started", "completed", "too_frequent"];
const AUTOVACUUM_KIND_LABELS: &[&str] = &["vacuum", "analyze"];
const LOCK_WAIT_KIND_LABELS: &[&str] = &["waiting", "acquired"];
const LIFECYCLE_KIND_LABELS: &[&str] = &["crash", "shutdown", "ready"];
const PGBOUNCER_LEVEL_LABELS: &[&str] = &["fatal", "error", "warning", "log", "debug", "noise"];

fn add_label(fields: &mut Map<String, Value>, field: &str, labels: &[&str]) {
    let Some(code) = fields.get(field).and_then(Value::as_u64) else {
        return;
    };
    let Some(index) = usize::try_from(code).ok() else {
        return;
    };
    let Some(label) = labels.get(index) else {
        return;
    };
    fields.insert(format!("{field}_label"), json!(label));
}

#[cfg(test)]
mod tests;
