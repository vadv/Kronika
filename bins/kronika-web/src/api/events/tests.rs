use std::collections::{BTreeMap, HashMap};

use serde_json::{Value, json};

use super::{
    EventDataRow, EventSource, EventStat, EventTier, EventsQuery, EventsQueryError,
    EventsRepresentation, EventsResult, StoredEventRow, event_collator, group_events,
    groups_result, label_event_fields, occurrences_result, slow_threshold_ms,
};
use crate::api::time::TimeRange;

const HOUR: i64 = 1_780_000_000_000_000;

fn row(_source: EventSource, ordinal: u64, minute: i64, values: Value) -> EventDataRow {
    let Value::Object(values) = values else {
        panic!("fixture values must be an object");
    };
    EventDataRow {
        segment_id: 7,
        type_id: 2_000_001,
        row_ordinal: ordinal,
        timestamp: HOUR + minute * 60_000_000,
        values,
    }
}

fn grouped(streams: HashMap<EventSource, Vec<EventDataRow>>) -> Vec<super::EventGroup> {
    group_events(streams, HOUR, Some(750.0)).expect("valid product groups")
}

fn assert_number(actual: f64, expected: f64) {
    assert_eq!(actual.to_bits(), expected.to_bits());
}

#[test]
fn errors_keep_weighted_counts_minutes_shared_values_and_one_locator() {
    let entries = grouped(HashMap::from([(
        EventSource::Errors,
        vec![
            row(
                EventSource::Errors,
                0,
                30,
                json!({ "severity": 0, "category": 1, "sqlstate": "23505", "pattern": "duplicate key", "count": 5, "database": "shop", "username": "app" }),
            ),
            row(
                EventSource::Errors,
                1,
                2,
                json!({ "severity": 0, "category": 1, "sqlstate": "23505", "pattern": "duplicate key", "count": 3, "database": "shop", "username": "other" }),
            ),
            row(
                EventSource::Errors,
                2,
                30,
                json!({ "severity": 1, "category": 6, "pattern": "terminating connection", "count": 1 }),
            ),
        ],
    )]));
    assert_eq!(entries.len(), 2);
    let fatal = &entries[0];
    assert_eq!(fatal.tier, EventTier::Critical);
    let duplicate = entries
        .iter()
        .find(|entry| entry.label.as_deref() == Some("duplicate key"))
        .expect("duplicate group");
    assert_number(duplicate.count, 8.0);
    assert_number(duplicate.minutes[2], 3.0);
    assert_number(duplicate.minutes[30], 5.0);
    assert_eq!(duplicate.detail_locator.section, "pg_log_errors");
    assert_eq!(duplicate.detail_locator.row_ordinal, json!("1"));
    assert_eq!(
        duplicate.detail_locator.row_key,
        Some(json!("duplicate key"))
    );
    let EventStat::Errors {
        database,
        username,
        sqlstate,
        ..
    } = &duplicate.stat
    else {
        panic!("error stat");
    };
    assert_eq!(database.as_deref(), Some("shop"));
    assert_eq!(username, &None);
    assert_eq!(sqlstate.as_deref(), Some("23505"));
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one regression covers the three compact-label grouping products"
)]
fn slow_autovacuum_and_pgbouncer_match_the_client_reducers() {
    let entries = grouped(HashMap::from([
        (
            EventSource::SlowQueries,
            vec![
                row(
                    EventSource::SlowQueries,
                    0,
                    3,
                    json!({ "pattern": "select ?", "sample": "select 1", "count": 2, "max_duration_ms": 100, "total_duration_ms": 150 }),
                ),
                row(
                    EventSource::SlowQueries,
                    1,
                    9,
                    json!({ "pattern": "select ?", "sample": "select 2", "count": 1, "max_duration_ms": 900, "total_duration_ms": 900 }),
                ),
                row(
                    EventSource::SlowQueries,
                    2,
                    10,
                    json!({ "pattern": "select ?", "sample": "tie loses", "count": 1, "max_duration_ms": 900, "total_duration_ms": null }),
                ),
            ],
        ),
        (
            EventSource::Autovacuum,
            vec![
                row(
                    EventSource::Autovacuum,
                    3,
                    4,
                    json!({ "kind": 1, "relation": "public.orders", "elapsed_ms": 10, "tuples_removed": 3, "tuples_dead_not_removable": 8 }),
                ),
                row(
                    EventSource::Autovacuum,
                    4,
                    8,
                    json!({ "kind": 1, "relation": "public.orders", "elapsed_ms": 15, "tuples_removed": 4, "tuples_dead_not_removable": 2 }),
                ),
            ],
        ),
        (
            EventSource::Pgbouncer,
            vec![
                row(
                    EventSource::Pgbouncer,
                    5,
                    1,
                    json!({ "level": 2, "text": "server login failed", "database": "app" }),
                ),
                row(
                    EventSource::Pgbouncer,
                    6,
                    2,
                    json!({ "level": 2, "text": "server login failed", "database": "app" }),
                ),
            ],
        ),
    ]));
    let slow = entries
        .iter()
        .find(|entry| entry.section == "pg_log_slow_queries")
        .expect("slow group");
    assert_number(slow.count, 4.0);
    assert_eq!(slow.label.as_deref(), Some("select ?"));
    assert_eq!(slow.detail_locator.row_ordinal, json!("1"));
    assert_eq!(
        slow.stat,
        EventStat::Slow {
            max_ms: 900.0,
            total_ms: 1_050.0,
            threshold_ms: Some(750.0)
        }
    );
    let vacuum = entries
        .iter()
        .find(|entry| entry.section == "pg_log_autovacuum")
        .expect("vacuum group");
    assert_eq!(
        vacuum.stat,
        EventStat::Autovacuum {
            analyze: true,
            runs: 2,
            total_ms: Some(25.0),
            tuples_removed: Some(7.0),
            tuples_dead: Some(2.0)
        }
    );
    let pool = entries
        .iter()
        .find(|entry| entry.section == "pgbouncer_events")
        .expect("pool group");
    assert_number(pool.count, 2.0);
    assert_eq!(pool.label, None);
    assert!(!pool.key.contains("server login failed"));
    assert!(pool.key.starts_with("pgbouncer:2:sha256:"));
    assert!(
        pool.detail_locator
            .row_key
            .as_ref()
            .and_then(Value::as_str)
            .is_some_and(|key| key.starts_with("sha256:"))
    );
    assert!(
        !serde_json::to_string(pool)
            .expect("pool wire")
            .contains("server login failed")
    );
    assert_eq!(
        pool.stat,
        EventStat::Pgbouncer {
            level: 2.0,
            database: Some("app".to_owned())
        }
    );
}

#[test]
#[expect(
    clippy::too_many_lines,
    reason = "one ported golden keeps the checkpoint, lock, and lifecycle interactions together"
)]
fn checkpoints_locks_and_lifecycle_keep_exact_episode_rules() {
    let entries = grouped(HashMap::from([
        (
            EventSource::Checkpoints,
            vec![
                row(
                    EventSource::Checkpoints,
                    0,
                    0,
                    json!({ "phase": 0, "reason": "time" }),
                ),
                row(
                    EventSource::Checkpoints,
                    1,
                    1,
                    json!({ "phase": 1, "buffers_written": 100, "sync_ms": 11 }),
                ),
                row(
                    EventSource::Checkpoints,
                    2,
                    20,
                    json!({ "phase": 0, "reason": "wal" }),
                ),
                row(
                    EventSource::Checkpoints,
                    3,
                    21,
                    json!({ "phase": 1, "buffers_written": 900, "sync_ms": 2100 }),
                ),
                row(
                    EventSource::Checkpoints,
                    4,
                    21,
                    json!({ "phase": 2, "seconds_apart": 18 }),
                ),
            ],
        ),
        (
            EventSource::LockWaits,
            vec![
                row(
                    EventSource::LockWaits,
                    5,
                    10,
                    json!({ "kind": 0, "pid": 2078, "lock_target": "transaction 987", "duration_ms": 1000, "holding_pids": "583" }),
                ),
                row(
                    EventSource::LockWaits,
                    6,
                    10,
                    json!({ "kind": 0, "pid": 456, "lock_target": "transaction 987", "duration_ms": 1001, "holding_pids": "583" }),
                ),
                row(
                    EventSource::LockWaits,
                    7,
                    11,
                    json!({ "kind": 1, "pid": 2078, "lock_target": "transaction 987", "duration_ms": 40000 }),
                ),
                row(
                    EventSource::LockWaits,
                    8,
                    12,
                    json!({ "kind": 1, "pid": 999, "lock_target": "transaction 44", "duration_ms": 1200 }),
                ),
            ],
        ),
        (
            EventSource::Lifecycle,
            vec![
                row(
                    EventSource::Lifecycle,
                    9,
                    5,
                    json!({ "kind": 0, "pid": 4242, "signal": 9, "message": "crash" }),
                ),
                row(
                    EventSource::Lifecycle,
                    10,
                    6,
                    json!({ "kind": 2, "message": "ready" }),
                ),
            ],
        ),
    ]));
    let checkpoint = entries
        .iter()
        .find(|entry| entry.key == "checkpoints")
        .expect("checkpoint group");
    assert_number(checkpoint.count, 2.0);
    assert_eq!(
        checkpoint.stat,
        EventStat::Checkpoints {
            completes: 2,
            timed: 1,
            requested: 1,
            max_sync_ms: Some(2100.0),
            buffers: Some(1000.0)
        }
    );
    let warning = entries
        .iter()
        .find(|entry| entry.key == "checkpoints:warning")
        .expect("warning group");
    assert_eq!(
        warning.stat,
        EventStat::CheckpointWarning {
            seconds_apart: Some(18.0)
        }
    );
    let waiting = entries
        .iter()
        .find(|entry| entry.key == "locks:583")
        .expect("waiting episode");
    assert_number(waiting.count, 2.0);
    assert_eq!(waiting.detail_locator.row_ordinal, json!("5"));
    assert_eq!(
        waiting.stat,
        EventStat::Locks {
            holders: Some("583".to_owned()),
            acquired: false,
            waiters: 2,
            max_ms: Some(40000.0),
            targets: vec!["transaction 987".to_owned()]
        }
    );
    let acquired = entries
        .iter()
        .find(|entry| entry.key == "locks:acquired")
        .expect("leftover acquisition");
    assert!(matches!(
        acquired.stat,
        EventStat::Locks { acquired: true, .. }
    ));
    assert_eq!(
        entries
            .iter()
            .filter(|entry| entry.section == "pg_log_lifecycle")
            .count(),
        2
    );
    assert_eq!(
        entries
            .iter()
            .find(|entry| {
                matches!(
                    entry.stat,
                    EventStat::Lifecycle { lifecycle, .. } if lifecycle == 0.0
                )
            })
            .expect("crash")
            .tier,
        EventTier::Critical
    );
}

#[test]
fn en_us_key_order_matches_the_javascript_locale_compare_golden() {
    let collator = event_collator().expect("compiled en-US collation data");
    let mut values = vec![
        "Ж", "a2", "a10", "A", "a", "á", "a\u{301}", "Å", "ä", "a b", "a_b", "a-b", "æ", "e", "E",
        "é", "z", "Е", "я", "😀",
    ];
    values.sort_by(|left, right| collator.compare(left, right));
    assert_eq!(
        values,
        [
            "😀", "a", "A", "á", "a\u{301}", "Å", "ä", "a b", "a_b", "a-b", "a10", "a2", "æ", "e",
            "E", "é", "z", "Е", "Ж", "я"
        ]
    );
    assert_eq!(collator.compare("á", "a\u{301}"), std::cmp::Ordering::Equal);
}

#[test]
fn normalization_defaults_deduplicates_and_rejects_temp_files_for_groups() {
    let range = TimeRange {
        from: HOUR,
        to_exclusive: HOUR + 1,
    };
    let default = EventsQuery::normalize(range, None, EventsRepresentation::Groups, 500)
        .expect("default groups");
    assert_eq!(default.sources, EventSource::GROUPS);
    let explicit = EventsQuery::normalize(
        range,
        Some(vec![
            "pg_log_errors".to_owned(),
            "pg_log_slow_queries".to_owned(),
            "pg_log_errors".to_owned(),
        ]),
        EventsRepresentation::Occurrences,
        500,
    )
    .expect("occurrences");
    assert_eq!(
        explicit.sources,
        [EventSource::Errors, EventSource::SlowQueries]
    );
    let error = EventsQuery::normalize(
        range,
        Some(vec!["pg_log_temp_files".to_owned()]),
        EventsRepresentation::Groups,
        500,
    )
    .expect_err("temp files have no group representation");
    let EventsQueryError::Source { valid, .. } = error else {
        panic!("source error");
    };
    assert_eq!(
        valid,
        EventSource::GROUPS
            .iter()
            .map(|source| source.as_str().to_owned())
            .collect::<Vec<_>>()
    );
}

#[test]
fn occurrences_keep_structural_fields_and_nested_locators_then_limit() {
    let query = EventsQuery {
        range: TimeRange {
            from: HOUR,
            to_exclusive: HOUR + 100,
        },
        sources: vec![EventSource::TempFiles, EventSource::Errors],
        representation: EventsRepresentation::Occurrences,
        limit: 3,
    };
    let stored = |ordinal, at, fields: Value| {
        let Value::Object(fields) = fields else {
            panic!("fields");
        };
        StoredEventRow {
            segment_id: 7,
            type_id: 2_007_001,
            row_ordinal: ordinal,
            at,
            fields: fields.into_iter().collect(),
        }
    };
    let result = occurrences_result(
        &query,
        vec![
            vec![
                stored(
                    0,
                    HOUR + 1,
                    json!({ "size_bytes": "9", "statement": "raw temp statement" }),
                ),
                stored(1, HOUR + 1, json!({ "size_bytes": "10" })),
            ],
            vec![
                stored(
                    2,
                    HOUR + 1,
                    json!({ "severity": 1, "pattern": "boom", "sample": "raw error sample" }),
                ),
                stored(3, HOUR + 2, json!({ "severity": 1, "pattern": "later" })),
            ],
        ],
    );
    let EventsResult::Occurrences {
        occurrences,
        truncated,
    } = result
    else {
        panic!("occurrences");
    };
    assert!(truncated);
    assert_eq!(
        occurrences
            .iter()
            .map(|row| row
                .detail_locator
                .row_ordinal
                .as_str()
                .expect("string ordinal"))
            .collect::<Vec<_>>(),
        ["0", "1", "2"]
    );
    assert!(!occurrences[0].fields.contains_key("statement"));
    assert!(!occurrences[2].fields.contains_key("sample"));
    assert_eq!(occurrences[2].fields.get("pattern"), Some(&json!("boom")));
    assert_eq!(occurrences[0].detail_locator.row_key, Some(json!("9")));
    let wire = serde_json::to_value(EventsResult::Occurrences {
        occurrences,
        truncated,
    })
    .expect("wire result");
    let first = &wire["occurrences"][0];
    assert!(first.get("segment_id").is_none());
    assert!(first.get("row_key").is_none());
    assert_eq!(first["detail_locator"]["segment_id"], "7");
    assert!(wire.get("has_more").is_none());
    assert!(wire.get("next_from").is_none());
}

#[test]
fn group_limit_is_global_after_full_grouping_and_has_no_continuation() {
    let query = EventsQuery {
        range: TimeRange {
            from: HOUR,
            to_exclusive: HOUR + 100,
        },
        sources: vec![EventSource::Errors],
        representation: EventsRepresentation::Groups,
        limit: 1,
    };
    let stored = |ordinal, at, pattern: &str, count| StoredEventRow {
        segment_id: 7,
        type_id: 2_001_001,
        row_ordinal: ordinal,
        at,
        fields: BTreeMap::from([
            ("severity".to_owned(), json!(0)),
            ("pattern".to_owned(), json!(pattern)),
            ("count".to_owned(), json!(count)),
        ]),
    };
    let result = groups_result(
        &query,
        vec![vec![
            stored(0, HOUR, "small", 1),
            stored(1, HOUR + 1, "large", 2),
            stored(2, HOUR + 2, "large", 3),
        ]],
        None,
    )
    .expect("groups");
    let EventsResult::Groups { groups, truncated } = &result else {
        panic!("groups");
    };
    assert!(truncated);
    assert_eq!(groups.len(), 1);
    assert_eq!(groups[0].label.as_deref(), Some("large"));
    assert_number(groups[0].count, 5.0);
    assert_eq!(groups[0].detail_locator.row_ordinal, json!("1"));
    let serialized = serde_json::to_value(&groups[0]).expect("group wire");
    assert!(serialized.get("text").is_none());
    assert!(serialized.get("rows").is_none());
    let wire = serde_json::to_value(result).expect("wire result");
    assert!(wire.get("has_more").is_none());
    assert!(wire.get("next_from").is_none());
    assert!(wire.get("next_cursor").is_none());
}

#[test]
fn occurrence_labels_cover_every_known_code_and_leave_unknown_codes_untouched() {
    for (section, field, code, label) in [
        ("pg_log_errors", "severity", 0, "error"),
        ("pg_log_errors", "category", 8, "auth"),
        ("pg_log_checkpoints", "phase", 2, "too_frequent"),
        ("pg_log_autovacuum", "kind", 1, "analyze"),
        ("pg_log_lock_waits", "kind", 1, "acquired"),
        ("pg_log_lifecycle", "kind", 2, "ready"),
        ("pgbouncer_events", "level", 2, "warning"),
    ] {
        let mut fields = serde_json::Map::from_iter([(field.to_owned(), json!(code))]);
        label_event_fields(section, &mut fields);
        assert_eq!(
            fields[&format!("{field}_label")],
            label,
            "{section}.{field}"
        );
        assert_eq!(fields[field], code, "the stored code remains unchanged");
    }

    let mut unknown = json!({ "kind": 99 }).as_object().expect("object").clone();
    label_event_fields("pg_log_lifecycle", &mut unknown);
    assert!(!unknown.contains_key("kind_label"));
    let mut unlabeled = json!({ "pattern": "select ?" })
        .as_object()
        .expect("object")
        .clone();
    label_event_fields("pg_log_slow_queries", &mut unlabeled);
    assert_eq!(
        unlabeled,
        json!({ "pattern": "select ?" })
            .as_object()
            .expect("object")
            .clone()
    );
}

#[test]
fn slow_threshold_uses_latest_strict_timestamp_and_exact_units() {
    let setting = |at, value: &str, unit: &str| StoredEventRow {
        segment_id: 1,
        type_id: 1,
        row_ordinal: 0,
        at,
        fields: BTreeMap::from([
            ("name".to_owned(), json!("log_min_duration_statement")),
            ("setting".to_owned(), json!(value)),
            ("unit".to_owned(), json!(unit)),
        ]),
    };
    assert_eq!(
        slow_threshold_ms(&[setting(1, "2", "s"), setting(2, "3", "min")]),
        Some(180_000.0)
    );
    assert_eq!(
        slow_threshold_ms(&[setting(2, "4", "ms"), setting(2, "9", "s")]),
        Some(4.0)
    );
    assert_eq!(slow_threshold_ms(&[setting(1, "-1", "ms")]), None);
    assert_eq!(slow_threshold_ms(&[setting(1, "not-a-number", "ms")]), None);
}
