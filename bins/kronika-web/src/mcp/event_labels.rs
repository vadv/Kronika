//! Adds text labels for section-specific numeric event codes.
//! Numeric fields remain unchanged; labels are added as `<field>_label`
//! siblings. Severity codes are not ordered by severity.

use serde_json::{Map, Value, json};

/// Adds labels for known code fields. Unknown sections, missing fields,
/// non-integer codes, and out-of-range codes remain unchanged.
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

// Labels must match the corresponding tables in
// `bins/kronika-web/ui/src/events-format.ts`.
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
