use hyper::StatusCode;
use kronika_index::{
    ActiveBackendPoint, Finding, FindingBlock, FindingKind, HealthPoint, ResourceIndex,
    SeriesBlock, TargetedIndex, TransactionPoint,
};
use kronika_reader::SegmentKind;

use super::{health_layout, resource_meta, section_layout, stream_findings, stream_series};
use crate::api::CachePolicy;
use crate::route::Window;

#[test]
fn a_finished_index_is_kept_by_the_browser_as_long_as_its_segment_lasts() {
    let meta = resource_meta(SegmentKind::Finished, Some(0x1234_abcd)).unwrap();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::Immutable);
    assert_eq!(meta.etag.as_deref(), Some("W/\"1234abcd\""));
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
            category: None,
            field_ordinal: 0,
            row_ordinal: 42,
            timestamp: 1_700_000_000_000_000,
        }],
    };
    let mut rows = Vec::new();
    let streamed = stream_findings(
        "pg_log_lifecycle",
        block,
        None,
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
            "logical_name": "pg_log_lifecycle",
            "type_id": "2006001",
            "total_hits": 1,
            "truncated": false,
        })
    );
    assert_eq!(
        rows[1],
        serde_json::json!({
            "record": "finding",
            "logical_name": "pg_log_lifecycle",
            "kind": "event",
            "type_id": "2006001",
            "field_ordinal": 0,
            "row_ordinal": 42,
            "ts": "1700000000000000",
        })
    );
    for copied in [
        "severity",
        "sqlstate",
        "pattern",
        "sample",
        "message",
        "query",
        "statement",
    ] {
        assert!(rows[1].get(copied).is_none());
    }
}

#[test]
fn error_event_stream_exposes_only_the_stored_category_and_locator() {
    let block = FindingBlock {
        type_id: 2_001_001,
        total_hits: 1,
        truncated: false,
        findings: vec![Finding {
            kind: FindingKind::Event,
            category: Some(5),
            field_ordinal: 0,
            row_ordinal: 42,
            timestamp: 1_700_000_000_000_000,
        }],
    };
    let mut rows = Vec::new();
    stream_findings(
        "pg_log_errors",
        block,
        None,
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("finding JSON"));
            true
        },
        &|| false,
    )
    .expect("stream findings");
    assert_eq!(
        rows[1],
        serde_json::json!({
            "record": "finding",
            "logical_name": "pg_log_errors",
            "kind": "event",
            "type_id": "2001001",
            "field_ordinal": 0,
            "row_ordinal": 42,
            "ts": "1700000000000000",
            "category": 5,
        })
    );
    for copied in [
        "severity",
        "sqlstate",
        "pattern",
        "sample",
        "message",
        "query",
        "statement",
    ] {
        assert!(rows[1].get(copied).is_none());
    }
}

#[test]
fn an_hour_keeps_only_inclusive_window_findings_and_recounts_them() {
    let block = FindingBlock {
        type_id: 2_006_001,
        total_hits: 5,
        truncated: false,
        findings: [99_i64, 100, 150, 200, 201]
            .into_iter()
            .enumerate()
            .map(|(row_ordinal, timestamp)| Finding {
                kind: FindingKind::Event,
                category: None,
                field_ordinal: 0,
                row_ordinal: u32::try_from(row_ordinal).expect("small test ordinal"),
                timestamp,
            })
            .collect(),
    };
    let mut rows = Vec::new();
    stream_findings(
        "pg_log_lifecycle",
        block,
        Some(Window {
            from: Some(100),
            to: Some(200),
        }),
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("finding JSON"));
            true
        },
        &|| false,
    )
    .expect("stream filtered findings");

    assert_eq!(rows[0]["total_hits"], 3);
    assert_eq!(rows[0]["truncated"], false);
    assert_eq!(
        rows[1..]
            .iter()
            .map(|row| row["ts"].as_str().expect("timestamp"))
            .collect::<Vec<_>>(),
        ["100", "150", "200"]
    );
}

#[test]
fn an_hour_filters_every_index_point_variant_to_its_inclusive_window() {
    let rows = bounded_index_rows();
    let points = rows
        .iter()
        .filter(|row| row["record"] == "point")
        .collect::<Vec<_>>();
    assert_eq!(points.len(), 10);
    assert!(
        points
            .iter()
            .all(|point| matches!(point["ts"].as_str(), Some("100" | "200")))
    );
    for series in ["os_health", "overall_health", "postgres_health"] {
        let values = points
            .iter()
            .filter(|point| point["series"] == series)
            .map(|point| point["value"].clone())
            .collect::<Vec<_>>();
        assert_eq!(values, [serde_json::Value::Null, serde_json::json!(0)]);
    }
    let transactions = points
        .iter()
        .filter(|point| point["series"] == "transactions_per_second")
        .map(|point| point["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        transactions,
        [serde_json::Value::Null, serde_json::json!(0.0)]
    );
    let activity = points
        .iter()
        .filter(|point| point["series"] == "active_backends")
        .map(|point| point["value"].clone())
        .collect::<Vec<_>>();
    assert_eq!(activity, [serde_json::json!(0), serde_json::json!(2)]);
    let findings = rows
        .iter()
        .filter(|row| row["record"] == "finding")
        .map(|row| row["ts"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        findings,
        [serde_json::json!("100"), serde_json::json!("200")]
    );
    assert_eq!(
        rows.iter()
            .find(|row| row["record"] == "findings")
            .expect("finding summary")["total_hits"],
        2
    );
}

fn bounded_index_rows() -> Vec<serde_json::Value> {
    let mut rows = Vec::new();
    let streamed = stream_series(
        "health",
        boundary_index_resource(),
        Some(Window {
            from: Some(100),
            to: Some(200),
        }),
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("index JSON"));
            true
        },
        &|| false,
    )
    .expect("stream bounded index");
    assert!(streamed);
    rows
}

fn boundary_index_resource() -> ResourceIndex {
    ResourceIndex {
        index: TargetedIndex {
            checksum: None,
            blocks: vec![
                SeriesBlock::OsHealth(boundary_health()),
                SeriesBlock::OverallHealth(boundary_health()),
                SeriesBlock::PostgresHealth(boundary_health()),
                SeriesBlock::PgTransactions {
                    type_id: 1_005_001,
                    points: [
                        (99, Some(9.0)),
                        (100, None),
                        (200, Some(0.0)),
                        (201, Some(1.0)),
                    ]
                    .map(|(timestamp, value)| TransactionPoint {
                        timestamp,
                        datid: 7,
                        value,
                    })
                    .to_vec(),
                },
                SeriesBlock::PgActiveBackends {
                    type_id: 1_001_001,
                    points: [(99, 9), (100, 0), (200, 2), (201, 1)]
                        .map(|(timestamp, count)| ActiveBackendPoint { timestamp, count })
                        .to_vec(),
                },
                SeriesBlock::Findings(boundary_findings()),
            ],
        },
        persisted: false,
    }
}

fn boundary_health() -> Vec<HealthPoint> {
    [(99, Some(99)), (100, None), (200, Some(0)), (201, Some(1))]
        .map(|(timestamp, value)| HealthPoint { timestamp, value })
        .to_vec()
}

fn boundary_findings() -> FindingBlock {
    FindingBlock {
        type_id: 2_006_001,
        total_hits: 4,
        truncated: false,
        findings: [99_i64, 100, 200, 201]
            .into_iter()
            .enumerate()
            .map(|(row_ordinal, timestamp)| Finding {
                kind: FindingKind::Event,
                category: None,
                field_ordinal: 0,
                row_ordinal: u32::try_from(row_ordinal).expect("small test ordinal"),
                timestamp,
            })
            .collect(),
    }
}

#[test]
fn a_filtered_truncated_block_never_counts_out_of_window_locators() {
    let block = FindingBlock {
        type_id: 2_006_001,
        total_hits: 5,
        truncated: true,
        findings: [90_i64, 100, 150]
            .into_iter()
            .enumerate()
            .map(|(row_ordinal, timestamp)| Finding {
                kind: FindingKind::Event,
                category: None,
                field_ordinal: 0,
                row_ordinal: u32::try_from(row_ordinal).expect("small test ordinal"),
                timestamp,
            })
            .collect(),
    };
    let mut rows = Vec::new();
    stream_findings(
        "pg_log_lifecycle",
        block.clone(),
        Some(Window {
            from: Some(100),
            to: Some(200),
        }),
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("finding JSON"));
            true
        },
        &|| false,
    )
    .expect("stream truncated findings");

    assert_eq!(rows[0]["total_hits"], 2);
    assert_eq!(rows[0]["truncated"], true);
    assert_eq!(rows.len(), 3);

    rows.clear();
    stream_findings(
        "pg_log_lifecycle",
        block,
        Some(Window {
            from: Some(90),
            to: Some(100),
        }),
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("finding JSON"));
            true
        },
        &|| false,
    )
    .expect("stream window before omitted tail");

    assert_eq!(rows[0]["total_hits"], 2);
    assert_eq!(rows[0]["truncated"], false);
    assert_eq!(rows.len(), 3);
}

#[test]
fn a_filtered_empty_truncated_block_keeps_its_unknown_tail_visible() {
    let block = FindingBlock {
        type_id: 2_006_001,
        total_hits: 1,
        truncated: true,
        findings: Vec::new(),
    };
    let mut rows = Vec::new();
    stream_findings(
        "pg_log_lifecycle",
        block,
        Some(Window {
            from: Some(100),
            to: Some(200),
        }),
        &mut |line| {
            rows.push(serde_json::from_slice::<serde_json::Value>(&line).expect("finding JSON"));
            true
        },
        &|| false,
    )
    .expect("stream empty truncated findings");

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["total_hits"], 0);
    assert_eq!(rows[0]["truncated"], true);
}
