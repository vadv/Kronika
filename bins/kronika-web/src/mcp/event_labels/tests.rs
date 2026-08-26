use serde_json::json;

use super::label_event_fields;

#[test]
fn labels_pg_log_errors_severity_and_category() {
    let mut fields = json!({ "severity": 0, "category": 8 })
        .as_object()
        .expect("object")
        .clone();
    label_event_fields("pg_log_errors", &mut fields);
    assert_eq!(fields["severity"], 0, "raw code stays untouched");
    assert_eq!(fields["severity_label"], "error");
    assert_eq!(fields["category"], 8);
    assert_eq!(fields["category_label"], "auth");
}

#[test]
fn labels_pg_log_checkpoints_phase() {
    let mut fields = json!({ "phase": 2 }).as_object().expect("object").clone();
    label_event_fields("pg_log_checkpoints", &mut fields);
    assert_eq!(fields["phase_label"], "too_frequent");
}

#[test]
fn labels_pg_log_autovacuum_kind() {
    let mut fields = json!({ "kind": 1 }).as_object().expect("object").clone();
    label_event_fields("pg_log_autovacuum", &mut fields);
    assert_eq!(fields["kind_label"], "analyze");
}

#[test]
fn labels_pg_log_lock_waits_kind_without_inverting_it() {
    // The reviewer's whole complaint: 0 and 1 read backwards if guessed.
    let mut still_waiting = json!({ "kind": 0 }).as_object().expect("object").clone();
    label_event_fields("pg_log_lock_waits", &mut still_waiting);
    assert_eq!(still_waiting["kind_label"], "waiting");

    let mut acquired = json!({ "kind": 1 }).as_object().expect("object").clone();
    label_event_fields("pg_log_lock_waits", &mut acquired);
    assert_eq!(acquired["kind_label"], "acquired");
}

#[test]
fn labels_pg_log_lifecycle_kind() {
    let mut fields = json!({ "kind": 2 }).as_object().expect("object").clone();
    label_event_fields("pg_log_lifecycle", &mut fields);
    assert_eq!(fields["kind_label"], "ready");
}

#[test]
fn labels_pgbouncer_events_level() {
    let mut fields = json!({ "level": 2 }).as_object().expect("object").clone();
    label_event_fields("pgbouncer_events", &mut fields);
    assert_eq!(fields["level_label"], "warning");
}

#[test]
fn leaves_pg_log_slow_queries_untouched() {
    // One of the seven sections, but it carries no enum-coded field.
    let mut fields = json!({ "pattern": "select 1" })
        .as_object()
        .expect("object")
        .clone();
    label_event_fields("pg_log_slow_queries", &mut fields);
    assert_eq!(
        fields.len(),
        1,
        "no label field invented for a section with none"
    );
}

#[test]
fn leaves_a_missing_field_missing_rather_than_guessing() {
    let mut fields = json!({}).as_object().expect("object").clone();
    label_event_fields("pg_log_errors", &mut fields);
    assert!(!fields.contains_key("severity_label"));
}

#[test]
fn leaves_an_out_of_range_code_unlabeled() {
    let mut fields = json!({ "kind": 99 }).as_object().expect("object").clone();
    label_event_fields("pg_log_lifecycle", &mut fields);
    assert!(!fields.contains_key("kind_label"));
}
