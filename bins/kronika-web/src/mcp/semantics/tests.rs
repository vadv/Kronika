use kronika_index::{FINDING_SEMANTICS, HEALTH_SEMANTICS, LOCKS_BLOCKED_BY_SEMANTIC};
use serde_json::json;

use super::{accepted, health, indexed, recorded_layout, referenced};

#[test]
fn accepted_definition_keeps_the_product_registry_value_in_one_envelope() {
    let definition = accepted("value_tone.query_duration_ms").expect("accepted semantic");

    assert_eq!(
        definition,
        json!({
            "id": "value_tone.query_duration_ms",
            "origin": "accepted_presentation",
            "source": "kronika_product_registry",
            "unit": "milliseconds",
            "formula": "(sample_timestamp - query_start) / 1000",
            "operands": ["sample_timestamp", "query_start", "backend_type", "state"],
            "thresholds": [
                {"operator": "gte", "value": 5000.0, "tone": "critical"},
                {"operator": "gte", "value": 1000.0, "tone": "warning"}
            ],
            "expected_band": {"min_inclusive": null, "max_exclusive": 1000.0},
            "policy": {
                "kind": "numeric_value_tone",
                "field": "query_duration_ms",
                "active_client_only": true
            }
        })
    );
}

#[test]
fn indexed_health_definition_uses_the_evaluator_descriptor() {
    let definition = indexed(HEALTH_SEMANTICS[0]);

    assert_eq!(definition["id"], "health.os");
    assert_eq!(definition["origin"], "kronika_derived");
    assert_eq!(definition["source"], "kronika_index");
    assert_eq!(definition["unit"], "percent");
    assert_eq!(definition["boundary"], json!(null));
}

#[test]
fn indexed_lock_definition_uses_the_exact_index_boundary() {
    let definition = indexed(LOCKS_BLOCKED_BY_SEMANTIC);

    assert_eq!(
        definition,
        json!({
            "id": "finding.pg_locks.blocked_by_nonempty",
            "logical_name": "pg_locks",
            "field": "blocked_by",
            "origin": "kronika_derived",
            "source": "kronika_index",
            "unit": null,
            "formula": null,
            "operands": ["blocked_by"],
            "boundary": {"operator": "nonempty"}
        })
    );
}

#[test]
fn referenced_dictionary_is_unique_exact_and_uses_index_descriptors() {
    let cpu = FINDING_SEMANTICS
        .iter()
        .find(|definition| definition.id == "finding.os_cpu.cpu_busy")
        .expect("CPU descriptor");
    let records = [
        json!({"semantic_id": cpu.id}),
        json!({"semantic_id": LOCKS_BLOCKED_BY_SEMANTIC.id}),
        json!({"semantic_id": cpu.id}),
        json!({"record": "event"}),
    ];

    let definitions = referenced(&records).expect("referenced definitions");
    let ids = definitions
        .iter()
        .map(|definition| definition["id"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        [
            Some("finding.os_cpu.cpu_busy"),
            Some("finding.pg_locks.blocked_by_nonempty"),
        ]
    );
    assert_eq!(definitions[0]["boundary"]["operator"], "gte");
    assert_eq!(definitions[0]["boundary"]["numerator"], "80");
    assert_eq!(definitions[0]["boundary"]["denominator"], "1");
}

#[test]
fn recorded_layout_keeps_lossless_registry_identity() {
    let layout = json!({
        "logical_name": "pg_locks",
        "type_id": "1011002",
        "columns": [{"name": "blocked_by", "unit": "none"}],
    });

    let definition = recorded_layout(&layout).expect("recorded layout");

    assert_eq!(definition["id"], "layout.1011002");
    assert_eq!(definition["origin"], "recorded");
    assert_eq!(definition["source"], "kronika_registry");
    assert_eq!(definition["layout"], layout);
}

#[test]
fn health_dictionary_contains_every_evaluator_descriptor() {
    let definitions = health();
    let ids = definitions
        .iter()
        .map(|definition| definition["id"].as_str())
        .collect::<Vec<_>>();

    assert_eq!(
        ids,
        [
            Some("health.os"),
            Some("health.postgresql"),
            Some("health.overall"),
        ]
    );
}
