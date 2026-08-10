use hyper::StatusCode;
use kronika_index::{Finding, FindingBlock, FindingKind};
use kronika_reader::SegmentKind;

use super::{etag_matches, health_layout, resource_meta, section_layout, stream_findings};
use crate::api::CachePolicy;

#[test]
fn entity_tag_matching_accepts_lists_wildcards_and_weak_validators() {
    assert!(etag_matches("\"other\", W/\"1234abcd\"", "\"1234abcd\""));
    assert!(etag_matches("*", "\"1234abcd\""));
    assert!(!etag_matches("\"1234abce\"", "\"1234abcd\""));
}

#[test]
fn every_finished_index_revalidates_even_when_a_cold_build_was_not_published() {
    let meta = resource_meta(SegmentKind::Finished, Some(0x1234_abcd)).unwrap();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::Revalidate);
    assert_eq!(meta.etag.as_deref(), Some("\"1234abcd\""));
    assert!(resource_meta(SegmentKind::Finished, None).is_err());
}

#[test]
fn active_index_has_no_validator_and_is_never_stored() {
    let meta = resource_meta(SegmentKind::Active, None).unwrap();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::NoStore);
    assert_eq!(meta.etag, None);
}

#[test]
fn health_has_three_explicit_allowlisted_series() {
    for series in ["os_health", "overall_health", "postgres_health"] {
        let value = health_layout(series);
        assert_eq!(value["logical_name"], "health");
        assert_eq!(value["type_id"], "0");
        assert_eq!(value["identity"].as_array().unwrap().len(), 0);
        assert_eq!(value["columns"][0]["name"], series);
        assert_eq!(value["columns"][0]["class"], "gauge");
        assert_eq!(value["columns"][0]["type"], "u8");
    }
    assert_eq!(
        section_layout("health", 0).unwrap()["columns"][0]["name"],
        "os_health"
    );
}

#[test]
fn a_logical_section_retains_its_exact_physical_layout_provenance() {
    let value = section_layout("pg_stat_database", 1_005_004).expect("known PG18 layout");
    assert_eq!(value["logical_name"], "pg_stat_database");
    assert_eq!(value["physical_name"], "pg_stat_database");
    assert_eq!(value["type_id"], "1005004");
    assert_eq!(value["identity"][0], "datid");
    assert_eq!(value["columns"][0]["name"], "transactions_per_second");
}

#[test]
fn statement_and_plan_layouts_have_no_summary_layout() {
    assert!(section_layout("pg_stat_statements", 1_002_006).is_err());
    assert!(section_layout("pg_store_plans", 1_004_001).is_err());
}

#[test]
fn event_stream_contains_only_sparse_locator_facts() {
    let block = FindingBlock {
        type_id: 2_006_001,
        total_hits: 1,
        truncated: false,
        findings: vec![Finding {
            kind: FindingKind::Event,
            field_ordinal: 0,
            row_ordinal: 42,
            timestamp: 1_700_000_000_000_000,
        }],
    };
    let mut rows = Vec::new();
    let streamed = stream_findings(
        block,
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("finding JSON"));
            true
        },
        &|| false,
    )
    .expect("stream findings");
    assert!(streamed);
    assert_eq!(rows.len(), 2);
    assert_eq!(
        rows[0],
        serde_json::json!({
            "record": "findings",
            "type_id": "2006001",
            "total_hits": 1,
            "truncated": false,
        })
    );
    assert_eq!(
        rows[1],
        serde_json::json!({
            "record": "finding",
            "kind": "event",
            "type_id": "2006001",
            "field_ordinal": 0,
            "row_ordinal": 42,
            "ts": "1700000000000000",
        })
    );
    for copied in ["message", "query", "statement"] {
        assert!(rows[1].get(copied).is_none());
    }
}
