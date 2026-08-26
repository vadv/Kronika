use std::sync::Arc;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use kronika_registry::Section as _;
use serde_json::json;

use super::response;
use crate::config::{Account, Config};
use crate::tests::artifacts::Fixture;

fn test_config(data_root: std::path::PathBuf) -> Arc<Config> {
    Arc::new(Config {
        data_root,
        listen: "127.0.0.1:0".parse().expect("listen address"),
        account: Account {
            user: "dba".to_owned(),
            password: "secret".to_owned(),
        },
        authentication_required: true,
        cookie_secure: false,
        sources: crate::config::SOURCE_OS | crate::config::SOURCE_POSTGRESQL,
        synthetic_demo: false,
    })
}

#[tokio::test]
async fn tools_list_returns_the_thirteen_tool_catalog() {
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/list",
        "params": {}
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(test_config(std::env::temp_dir()), request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    let names: Vec<&str> = decoded["result"]["tools"]
        .as_array()
        .expect("tools array")
        .iter()
        .map(|tool| tool["name"].as_str().expect("name"))
        .collect();
    assert_eq!(
        names,
        vec![
            "kronika_overview",
            "kronika_get_context",
            "kronika_find_postgresql_tables",
            "kronika_find_postgresql_indexes",
            "kronika_find_postgresql_activity",
            "kronika_find_postgresql_locks",
            "kronika_find_postgresql_vacuum",
            "kronika_find_postgresql_databases",
            "kronika_find_postgresql_statements",
            "kronika_find_postgresql_plans",
            "kronika_find_processes",
            "kronika_get_row_detail",
            "kronika_find_events",
        ]
    );
}

#[test]
fn get_context_lists_recorded_sections_with_row_counts() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture"), (300, 101, 10, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let sections = structured["sections"].as_array().expect("sections array");
    let os_process = sections
        .iter()
        .find(|section| section["logical_name"] == "os_process")
        .expect("os_process section present");
    assert_eq!(os_process["rows"], "2");
    assert_eq!(os_process["source_family"], "os");
}

#[test]
fn get_context_reports_no_sections_on_an_empty_data_root() {
    let empty_root = tempfile::tempdir().expect("empty data root");
    let config = test_config(empty_root.path().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["sections"].as_array(), Some(&Vec::new()));
}

// Two samples per PID make ranking maxima differ from last-value band totals.
fn ranked_process_gauge_rows() -> [(i64, i32, i64, &'static str); 10] {
    [
        (100, 101, 50, "fixture"),
        (300, 101, 10, "fixture"),
        (100, 102, 40, "fixture"),
        (300, 102, 45, "fixture"),
        (100, 103, 5, "fixture"),
        (300, 103, 30, "fixture"),
        (100, 104, 20, "fixture"),
        (300, 104, 8, "fixture"),
        (100, 105, 15, "fixture"),
        (300, 105, 25, "fixture"),
    ]
}

#[test]
fn overview_ranks_the_top_entities_and_reports_the_others_total() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "section": "os_process",
        "fields": ["rmem_kb"],
        "from": 100,
        "to": 400,
        "top": 2,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let entities = structured["entities"].as_array().expect("entities array");
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0]["total"], 50.0);
    assert_eq!(entities[0]["identity"]["pid"], 101);
    assert_eq!(entities[1]["total"], 45.0);
    // One convention for every number in a gauge ranking: entity totals
    // are window maxima, so the two aggregate fields are maxima too —
    // totals_total across all five entities, others_total across the
    // three beyond top=2.
    assert_eq!(structured["totals_total"], 50.0);
    assert_eq!(structured["others_total"], 30.0);
    assert_eq!(structured["entity_count"], "5");
}

#[test]
fn overview_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::overview::call(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn overview_rejects_a_top_above_the_heatmap_cap() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "section": "os_process",
        "fields": ["rmem_kb"],
        "from": 100,
        "to": 400,
        "top": 4_000_000_000_u64,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::overview::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("500") || message.contains("top"),
        "error must name the top cap: {message}"
    );
}

#[tokio::test]
async fn overview_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&ranked_process_gauge_rows());
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_overview",
            "arguments": {
                "section": "os_process",
                "fields": ["rmem_kb"],
                "from": 100,
                "to": 400,
                "top": 2,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    let entities = decoded["result"]["structuredContent"]["entities"]
        .as_array()
        .expect("entities array");
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0]["total"], 50.0);
    assert_eq!(entities[0]["identity"]["pid"], 101);
    assert_eq!(entities[1]["total"], 45.0);
    assert_eq!(decoded["result"]["structuredContent"]["totals_total"], 50.0);
    assert_eq!(decoded["result"]["structuredContent"]["others_total"], 30.0);
    assert_eq!(decoded["result"]["structuredContent"]["entity_count"], "5");
}

#[test]
fn find_postgresql_tables_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 0, "db", "public", "alpha"),
        (200, 1, 11, 30, "db", "public", "alpha"),
        (100, 1, 12, 0, "db", "public", "beta"),
        (200, 1, 12, 15, "db", "public", "beta"),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "group": "object",
        "filters": [],
        "sort": {"field": "seq_scan", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_tables(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["relname"], "alpha");
    assert_eq!(rows[0]["datname"], "db");
    assert_eq!(rows[1]["relname"], "beta");
    assert_eq!(structured["has_more"], false);

    let filtered_arguments = serde_json::json!({
        "group": "object",
        "filters": [{"field": "table_name", "op": "eq", "value": "alpha"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_tables(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_structured = filtered.structured_content.expect("structured content");
    let filtered_rows = filtered_structured["rows"].as_array().expect("rows array");
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["relname"], "alpha");
}

#[test]
fn find_postgresql_tables_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_tables(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_postgresql_tables_rejects_a_limit_above_the_snapshot_page_size_cap() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "group": "object",
        "filters": [],
        "limit": 4_000_000_000_u64,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_tables(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("5000") || message.contains("limit"),
        "error must name the limit cap: {message}"
    );
}

#[test]
fn find_postgresql_indexes_returns_keyed_rows_with_indexrelname() {
    let mut fixture = Fixture::new();
    fixture.append_named_index_snapshots(&[
        (
            100,
            1,
            21,
            0,
            "db",
            "public",
            "alpha",
            "alpha_pkey",
            "CREATE UNIQUE INDEX alpha_pkey ON alpha USING btree (id)",
        ),
        (
            200,
            1,
            21,
            20,
            "db",
            "public",
            "alpha",
            "alpha_pkey",
            "CREATE UNIQUE INDEX alpha_pkey ON alpha USING btree (id)",
        ),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "group": "object",
        "filters": [],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_indexes(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["indexrelname"], "alpha_pkey");
    assert_eq!(rows[0]["relname"], "alpha");
    assert_eq!(structured["has_more"], false);
}

#[test]
fn find_postgresql_activity_ranks_and_filters() {
    // `append_postgres_health(2)` records active PIDs 0 and 1 and idle PID
    // 10,000.
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(2);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "pid", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_activity(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["pid"], 10_000);
    assert_eq!(rows[0]["state"], "idle");
    assert!(rows[0]["segment_id"].is_string());
    assert!(rows[0]["type_id"].is_string());
    assert!(rows[0]["row_ordinal"].is_string());
    assert!(rows[0]["at"].is_string());
    assert_eq!(structured["has_more"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "state", "op": "eq", "value": "idle"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_activity(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_rows = filtered.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 10_000);
}

#[test]
fn find_postgresql_activity_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_activity(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[tokio::test]
async fn find_postgresql_activity_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(2);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_find_postgresql_activity",
            "arguments": {
                "filters": [],
                "sort": {"field": "pid", "direction": "desc"},
                "limit": 10,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    let rows = decoded["result"]["structuredContent"]["rows"]
        .as_array()
        .expect("rows array");
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["pid"], 10_000);
    assert_eq!(rows[0]["state"], "idle");
    assert_eq!(decoded["result"]["structuredContent"]["has_more"], false);
}

#[test]
fn find_postgresql_locks_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_lock_rows(&[
        (100, 701, "active", "RowExclusiveLock"),
        (100, 702, "active", "AccessShareLock"),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "pid", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_locks(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 702);
    assert_eq!(rows[1]["pid"], 701);
    assert!(rows[0]["segment_id"].is_string());
    assert!(rows[0]["type_id"].is_string());
    assert!(rows[0]["row_ordinal"].is_string());
    assert!(rows[0]["at"].is_string());
    assert_eq!(structured["has_more"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "lock_mode", "op": "eq", "value": "AccessShareLock"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_locks(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_rows = filtered.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 702);
}

#[test]
fn find_postgresql_locks_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_locks(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_postgresql_vacuum_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_vacuum_rows(&[
        (100, 501, "scanning heap", 1_000, 500, 400),
        (100, 502, "vacuuming indexes", 2_000, 2_000, 1_800),
    ]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "heap_blks_scanned", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_vacuum(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 502);
    assert_eq!(rows[1]["pid"], 501);
    assert!(rows[0]["segment_id"].is_string());
    assert!(rows[0]["type_id"].is_string());
    assert!(rows[0]["row_ordinal"].is_string());
    assert!(rows[0]["at"].is_string());
    assert_eq!(structured["has_more"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "phase", "op": "eq", "value": "scanning heap"}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::postgresql::call_vacuum(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_rows = filtered.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 501);
}

#[test]
fn find_postgresql_vacuum_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_vacuum(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_postgresql_databases_ranks_and_filters() {
    // Two samples for one database produce a positive deadlock rate.
    let mut fixture = Fixture::new();
    fixture.append_postgres_database_snapshots(&[(100, 100, 10, 0), (200, 180, 30, 7)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "datid", "direction": "asc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_databases(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["datid"], 73);
    assert_eq!(rows[0]["datname"], "db");
    assert!(rows[0]["segment_id"].is_string());
    assert!(rows[0]["type_id"].is_string());
    assert!(rows[0]["row_ordinal"].is_string());
    assert!(rows[0]["at"].is_string());
    assert_eq!(structured["has_more"], false);

    let matching_arguments = serde_json::json!({
        "filters": [{"field": "deadlocks", "op": "gt", "value": 0}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let matching = crate::mcp::postgresql::call_databases(&config, matching_arguments, &|| false);
    assert_eq!(matching.is_error, Some(false));
    let matching_rows = matching.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(matching_rows.len(), 1);

    let below_threshold_arguments = serde_json::json!({
        "filters": [{"field": "deadlocks", "op": "gt", "value": 100}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let below_threshold =
        crate::mcp::postgresql::call_databases(&config, below_threshold_arguments, &|| false);
    assert_eq!(below_threshold.is_error, Some(false));
    let below_threshold_rows = below_threshold
        .structured_content
        .expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert!(below_threshold_rows.is_empty());
}

#[test]
fn find_postgresql_databases_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_databases(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

/// Checks a JSON number within `1e-9`; composed ratios divide previously
/// rounded rates, so exact `f64` equality is not stable.
fn assert_close(actual: &serde_json::Value, expected: f64) {
    let actual = actual
        .as_f64()
        .unwrap_or_else(|| panic!("expected a number, got {actual:?}"));
    assert!(
        (actual - expected).abs() < 1e-9,
        "expected {expected}, got {actual}"
    );
}

#[test]
fn find_postgresql_statements_computes_derived_ratio_fields() {
    // Expected fields use the `ts=200 - ts=100` deltas from
    // `append_ranked_statements`.
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array").clone();
    assert_eq!(rows.len(), 3);

    let row = |queryid: &str| {
        rows.iter()
            .find(|row| row["queryid"] == queryid)
            .unwrap_or_else(|| panic!("no row for queryid {queryid}"))
    };

    let one = row("1");
    assert_close(&one["derived_mean_exec_ms_per_call"], 10.0);
    assert_close(&one["derived_rows_per_call"], 10.0);
    assert_close(&one["derived_blocks_per_call"], 10.0);
    assert_close(&one["derived_hit_fraction"], 0.8);
    assert_close(&one["derived_wal_per_call"], 10.0);
    assert_close(&one["derived_plan_time_fraction"], 0.5);
    assert_close(&one["derived_cv"], 0.9);
    assert!(one["segment_id"].is_string());
    assert!(one["type_id"].is_string());
    assert!(one["row_ordinal"].is_string());
    assert!(one["at"].is_string());

    let two = row("2");
    assert_close(&two["derived_mean_exec_ms_per_call"], 15.0);
    assert_close(&two["derived_rows_per_call"], 30.0);
    assert_close(&two["derived_blocks_per_call"], 25.0);
    assert_close(&two["derived_hit_fraction"], 0.9);
    assert_close(&two["derived_wal_per_call"], 30.0);
    assert_close(&two["derived_plan_time_fraction"], 40.0 / 70.0);
    assert_close(&two["derived_cv"], 2.0);

    let three = row("3");
    assert_close(&three["derived_mean_exec_ms_per_call"], 5.0);
    assert_close(&three["derived_rows_per_call"], 1.0);
    assert_close(&three["derived_blocks_per_call"], 3.0);
    assert_close(&three["derived_hit_fraction"], 1.0 / 3.0);
    assert_close(&three["derived_wal_per_call"], 5.0);
    assert_close(&three["derived_plan_time_fraction"], 1.0 / 6.0);
    assert_close(&three["derived_cv"], 0.2);
}

#[test]
fn find_postgresql_statements_nulls_derived_fields_without_a_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 100.0)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    for field in [
        "derived_mean_exec_ms_per_call",
        "derived_rows_per_call",
        "derived_blocks_per_call",
        "derived_hit_fraction",
        "derived_wal_per_call",
        "derived_plan_time_fraction",
        "derived_cv",
    ] {
        assert_eq!(
            row[field],
            serde_json::Value::Null,
            "{field} must be null without a predecessor snapshot"
        );
    }
}

#[test]
fn find_postgresql_statements_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result =
        crate::mcp::postgresql::call_statements(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_postgresql_plans_computes_derived_ratio_fields() {
    // The OSSC `pg_store_plans` layout lacks `wal_bytes` and
    // `total_plan_time`, so both derived fields are null. `mean_time` and
    // `stddev_time` are gauges; `derived_cv` needs no predecessor.
    let mut fixture = Fixture::new();
    fixture.append_ranked_plans();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_plans(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array").clone();
    assert_eq!(rows.len(), 3);

    let row = |queryid: &str| {
        rows.iter()
            .find(|row| row["queryid"] == queryid)
            .unwrap_or_else(|| panic!("no row for queryid {queryid}"))
    };

    let one = row("1");
    assert_close(&one["derived_mean_exec_ms_per_call"], 10.0);
    assert_close(&one["derived_rows_per_call"], 10.0);
    assert_close(&one["derived_blocks_per_call"], 10.0);
    assert_close(&one["derived_hit_fraction"], 0.8);
    assert_eq!(one["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(one["derived_plan_time_fraction"], serde_json::Value::Null);
    assert_close(&one["derived_cv"], 2.2 / 24.9);

    let two = row("2");
    assert_close(&two["derived_mean_exec_ms_per_call"], 15.0);
    assert_close(&two["derived_rows_per_call"], 30.0);
    assert_close(&two["derived_blocks_per_call"], 25.0);
    assert_close(&two["derived_hit_fraction"], 0.9);
    assert_eq!(two["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(two["derived_plan_time_fraction"], serde_json::Value::Null);
    assert_close(&two["derived_cv"], 2.2 / 24.9);

    let three = row("3");
    assert_close(&three["derived_mean_exec_ms_per_call"], 5.0);
    assert_close(&three["derived_rows_per_call"], 1.0);
    assert_close(&three["derived_blocks_per_call"], 3.0);
    assert_close(&three["derived_hit_fraction"], 1.0 / 3.0);
    assert_eq!(three["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(three["derived_plan_time_fraction"], serde_json::Value::Null);
    assert_close(&three["derived_cv"], 2.2 / 24.9);
}

#[test]
fn find_postgresql_plans_nulls_rate_derived_fields_without_a_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_plan_snapshots(&[(100, 1, 10, 100.0)]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_plans(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    // Rate-dependent fields need a predecessor.
    for field in [
        "derived_mean_exec_ms_per_call",
        "derived_rows_per_call",
        "derived_blocks_per_call",
        "derived_hit_fraction",
    ] {
        assert_eq!(
            row[field],
            serde_json::Value::Null,
            "{field} must be null without a predecessor snapshot"
        );
    }
    // Absent on this layout regardless of predecessor.
    assert_eq!(row["derived_wal_per_call"], serde_json::Value::Null);
    assert_eq!(row["derived_plan_time_fraction"], serde_json::Value::Null);
    // Gauge-only: computable even without a predecessor.
    assert_close(&row["derived_cv"], 2.2 / 24.9);
}

#[test]
fn find_postgresql_plans_computes_plan_time_fraction_on_the_vadv_layout() {
    // Only the vadv layout (`1_004_001`) records `total_plan_time`; this
    // fixture makes `derived_plan_time_fraction` non-null.
    let mut fixture = Fixture::new();
    fixture.append_ranked_vadv_plans();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "filters": [], "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_plans(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    // total_plan_time=25.0, total_time=100.0 => 25 / (25 + 100) = 0.2.
    assert_close(&row["derived_plan_time_fraction"], 0.2);
}

#[test]
fn find_postgresql_plans_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_plans(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_processes_ranks_and_filters() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "filters": [],
        "sort": {"field": "rmem_kb", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::processes::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 101);
    assert_eq!(rows[1]["pid"], 102);
    assert_eq!(structured["has_more"], false);

    let filtered_arguments = serde_json::json!({
        "filters": [{"field": "pid", "op": "eq", "value": 102}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let filtered = crate::mcp::processes::call(&config, filtered_arguments, &|| false);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_structured = filtered.structured_content.expect("structured content");
    let filtered_rows = filtered_structured["rows"].as_array().expect("rows array");
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 102);
}

#[test]
fn find_processes_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::processes::call(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_processes_rejects_a_limit_above_the_snapshot_page_size_cap() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "filters": [],
        "limit": 4_000_000_000_u64,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::processes::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("5000") || message.contains("limit"),
        "error must name the limit cap: {message}"
    );
}

#[tokio::test]
async fn find_processes_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_find_processes",
            "arguments": {
                "filters": [],
                "sort": {"field": "rmem_kb", "direction": "desc"},
                "limit": 10,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    let rows = decoded["result"]["structuredContent"]["rows"]
        .as_array()
        .expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["pid"], 101);
    assert_eq!(rows[1]["pid"], 102);
    assert_eq!(decoded["result"]["structuredContent"]["has_more"], false);
}

#[test]
fn get_row_detail_agrees_with_find_processes_for_the_same_physical_row() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());

    let listing_arguments = serde_json::json!({
        "filters": [{"field": "pid", "op": "eq", "value": 101}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::processes::call(&config, listing_arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();

    // `append_process_gauge_rows` stores PID 101 at row ordinal 0.
    let (segment_id, at) = crate::mcp::postgresql::current_segment(&config.data_root, "os_process")
        .expect("current segment")
        .expect("os_process recorded");
    let type_id = kronika_registry::os_process::OsProcess::CONTRACT
        .type_id
        .get();
    let detail_arguments = serde_json::json!({
        "section": "os_process",
        "segment_id": segment_id,
        "at": at,
        "type_id": type_id,
        "row_ordinal": 0,
    })
    .as_object()
    .expect("object")
    .clone();

    let detail = crate::mcp::row_detail::call(&config, detail_arguments, &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["pid"], listing_row["pid"]);
    assert_eq!(detail_row["rmem_kb"], listing_row["rmem_kb"]);
    assert_eq!(detail_row["segment_id"], segment_id.to_string());
    assert_eq!(detail_row["type_id"], type_id.to_string());
    assert_eq!(detail_row["row_ordinal"], "0");
    assert_eq!(detail_row["at"], at.to_string());
}

#[test]
fn get_row_detail_chains_directly_from_a_find_processes_locator() {
    // The `find_processes` locator fields are valid `get_row_detail` arguments
    // without conversion.
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha"), (100, 102, 40, "beta")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());

    let listing_arguments = serde_json::json!({
        "filters": [{"field": "pid", "op": "eq", "value": 102}],
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::processes::call(&config, listing_arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();

    let detail_arguments = serde_json::json!({
        "section": "os_process",
        "segment_id": listing_row["segment_id"],
        "at": listing_row["at"],
        "type_id": listing_row["type_id"],
        "row_ordinal": listing_row["row_ordinal"],
    })
    .as_object()
    .expect("object")
    .clone();
    let detail = crate::mcp::row_detail::call(&config, detail_arguments, &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["pid"], listing_row["pid"]);
    assert_eq!(detail_row["rmem_kb"], listing_row["rmem_kb"]);
    assert_eq!(detail_row["segment_id"], listing_row["segment_id"]);
    assert_eq!(detail_row["type_id"], listing_row["type_id"]);
    assert_eq!(detail_row["row_ordinal"], listing_row["row_ordinal"]);
    assert_eq!(detail_row["at"], listing_row["at"]);
}

#[test]
fn get_row_detail_rejects_a_relation_grouped_section() {
    // Grouped `pg_stat_user_tables` results have no single physical row ordinal.
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let (segment_id, at) =
        crate::mcp::postgresql::current_segment(&config.data_root, "pg_stat_user_tables")
            .expect("current segment")
            .expect("pg_stat_user_tables recorded");
    let type_id = kronika_registry::pg_stat_user_tables::PgStatUserTablesV1::CONTRACT
        .type_id
        .get();
    let arguments = serde_json::json!({
        "section": "pg_stat_user_tables",
        "segment_id": segment_id,
        "at": at,
        "type_id": type_id,
        "row_ordinal": 0,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::row_detail::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn get_row_detail_accepts_segment_id_and_row_ordinal_as_decimal_strings() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let (segment_id, at) = crate::mcp::postgresql::current_segment(&config.data_root, "os_process")
        .expect("current segment")
        .expect("os_process recorded");
    let type_id = kronika_registry::os_process::OsProcess::CONTRACT
        .type_id
        .get();
    let arguments = serde_json::json!({
        "section": "os_process",
        "segment_id": segment_id.to_string(),
        "at": at,
        "type_id": type_id,
        "row_ordinal": "0",
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::row_detail::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["pid"], 101);
}

#[test]
fn get_row_detail_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::row_detail::call(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_events_returns_rows_with_source_and_locator_fields() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    assert_eq!(structured["has_more"], false);
    for row in rows {
        assert_eq!(row["source"], "pg_log_errors");
        assert!(
            row["segment_id"].is_string(),
            "segment_id is a decimal string"
        );
        assert!(row["type_id"].is_string(), "type_id is a decimal string");
        assert!(
            row["row_ordinal"].is_string(),
            "row_ordinal is a decimal string"
        );
        assert!(row["at"].is_string(), "at is a decimal string");
        assert_eq!(row["category"], 8);
    }
}

#[test]
fn find_events_labels_the_numeric_severity_and_category_codes() {
    // `severity` and `category` are unordered Kronika codes.
    // `append_log_error` records 0 (`error`) and 8 (`auth`).
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert_eq!(row["severity"], 0, "raw numeric code stays untouched");
    assert_eq!(row["severity_label"], "error");
    assert_eq!(row["category"], 8);
    assert_eq!(row["category_label"], "auth");
}

#[test]
fn find_events_sorts_returned_candidates_by_timestamp() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_pgbouncer_event(150);
    fixture.append_log_error(300);
    fixture.append_pgbouncer_event(250);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors", "pgbouncer_events"],
        "from": 0,
        "to": 1_000,
        "limit": 3,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 3, "4 rows exist in the window but limit is 3");
    assert_eq!(structured["has_more"], true);
    let ordered_ats: Vec<i64> = rows
        .iter()
        .map(|row| {
            row["at"]
                .as_str()
                .expect("at is a decimal string")
                .parse()
                .expect("at parses")
        })
        .collect();
    assert_eq!(
        ordered_ats,
        vec![100, 150, 250],
        "retained fixture rows are sorted by timestamp"
    );
    let sources: Vec<&str> = rows
        .iter()
        .map(|row| row["source"].as_str().expect("source"))
        .collect();
    assert_eq!(
        sources,
        vec!["pg_log_errors", "pgbouncer_events", "pgbouncer_events"]
    );
}

#[test]
fn find_events_rejects_a_3_600_000_000_microsecond_endpoint_difference() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 3_600_000_001_i64,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(true));
    let message = result.content[0]
        .as_text()
        .expect("text content")
        .text
        .clone();
    assert!(
        message.contains("3600000000") || message.contains("one hour"),
        "error must name the window limit: {message}"
    );
}

#[test]
fn find_events_accepts_a_window_of_exactly_one_hour() {
    // "At most one hour" includes one hour itself: the schema promises the
    // span, so the boundary must not be off by one microsecond.
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 3_600_000_000_i64,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
}

#[test]
fn find_events_rejects_an_unknown_source_name() {
    let config = test_config(std::env::temp_dir());
    let arguments = serde_json::json!({
        "sources": ["pg_log_made_up"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn find_events_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::events::call(&config, serde_json::Map::new(), &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn get_row_detail_chains_directly_from_a_find_events_locator() {
    // The `find_events` locator fields are valid `get_row_detail` arguments
    // without conversion.
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let listing_arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::events::call(&config, listing_arguments, &|| false);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();

    let detail_arguments = serde_json::json!({
        "section": "pg_log_errors",
        "segment_id": listing_row["segment_id"],
        "at": listing_row["at"],
        "type_id": listing_row["type_id"],
        "row_ordinal": listing_row["row_ordinal"],
    })
    .as_object()
    .expect("object")
    .clone();
    let detail = crate::mcp::row_detail::call(&config, detail_arguments, &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["category"], listing_row["category"]);
    assert_eq!(detail_row["segment_id"], listing_row["segment_id"]);
    assert_eq!(detail_row["type_id"], listing_row["type_id"]);
    assert_eq!(detail_row["row_ordinal"], listing_row["row_ordinal"]);
    assert_eq!(detail_row["at"], listing_row["at"]);
}

#[test]
fn get_row_detail_labels_the_same_numeric_codes_find_events_does() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let listing_arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let listing = crate::mcp::events::call(&config, listing_arguments, &|| false);
    let listing_row = listing.structured_content.expect("structured content")["rows"][0].clone();

    let detail_arguments = serde_json::json!({
        "section": "pg_log_errors",
        "segment_id": listing_row["segment_id"],
        "at": listing_row["at"],
        "type_id": listing_row["type_id"],
        "row_ordinal": listing_row["row_ordinal"],
    })
    .as_object()
    .expect("object")
    .clone();
    let detail = crate::mcp::row_detail::call(&config, detail_arguments, &|| false);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    assert_eq!(detail_row["severity_label"], "error");
    assert_eq!(detail_row["category_label"], "auth");
    assert_eq!(detail_row["severity_label"], listing_row["severity_label"]);
    assert_eq!(detail_row["category_label"], listing_row["category_label"]);
}

#[tokio::test]
async fn find_events_end_to_end_through_the_real_transport() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let body = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "kronika_find_events",
            "arguments": {
                "sources": ["pg_log_errors"],
                "from": 0,
                "to": 1_000,
                "limit": 10,
            }
        }
    });
    let request = Request::builder()
        .method(Method::POST)
        .uri("http://kronika.test/mcp")
        .header(HOST, "kronika.test")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(
            serde_json::to_vec(&body).expect("json"),
        )))
        .expect("request");

    let response = response(config, request).await;
    assert_eq!(response.status(), hyper::StatusCode::OK);
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("body")
        .to_bytes();
    let decoded: serde_json::Value = serde_json::from_slice(&bytes).expect("json-rpc response");
    let rows = decoded["result"]["structuredContent"]["rows"]
        .as_array()
        .expect("rows array");
    assert_eq!(rows.len(), 2);
    for row in rows {
        assert_eq!(row["source"], "pg_log_errors");
        assert_eq!(row["category"], 8);
        assert!(
            row["segment_id"].is_string(),
            "segment_id is a decimal string"
        );
    }
    assert_eq!(decoded["result"]["structuredContent"]["has_more"], false);
}

#[test]
fn find_tables_falls_back_to_the_newest_segment_carrying_the_section() {
    // Relations ride a slower cadence than the rest, so the newest segment
    // regularly has none — the tool must answer from the newest segment
    // that does, not fail with a paging error.
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish_and_continue(1_709_164_800_000_000 + 1_000);
    fixture.append_process_gauge_rows(&[(200, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "group": "object",
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_tables(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["relname"], "alpha");
    assert!(structured["as_of"].is_string());
}

#[test]
fn find_vacuum_reports_no_recorded_rows_instead_of_an_error() {
    // On a healthy host this is the tool's normal state: the section is
    // recorded only while a vacuum runs.
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({ "limit": 10 })
        .as_object()
        .expect("object")
        .clone();

    let result = crate::mcp::postgresql::call_vacuum(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["rows"].as_array(), Some(&Vec::new()));
    assert_eq!(structured["has_more"], false);
    assert!(structured["as_of"].is_null());
}

#[test]
fn unknown_sort_fields_are_rejected_not_ignored() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(1);
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let plain = serde_json::json!({
        "sort": {"field": "query_duration", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::postgresql::call_activity(&config, plain, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("no such sort field"),
        "unexpected message: {message}"
    );

    let relation = serde_json::json!({
        "group": "object",
        "sort": {"field": "not_a_field", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::postgresql::call_tables(&config, relation, &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn filter_eq_matches_the_whole_value_and_contains_a_substring() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(1);
    fixture.finish();
    let config = test_config(fixture.root().to_path_buf());

    let count_for = |op: &str, value: &str| {
        let arguments = serde_json::json!({
            "filters": [{"field": "state", "op": op, "value": value}],
            "limit": 10,
        })
        .as_object()
        .expect("object")
        .clone();
        let result = crate::mcp::postgresql::call_activity(&config, arguments, &|| false);
        assert_eq!(result.is_error, Some(false));
        result.structured_content.expect("structured content")["rows"]
            .as_array()
            .expect("rows array")
            .len()
    };

    assert_eq!(count_for("eq", "idle"), 1);
    assert_eq!(count_for("eq", "idl"), 0);
    assert_eq!(count_for("contains", "idl"), 1);
}

#[test]
fn more_than_eight_filters_are_rejected() {
    let config = test_config(std::env::temp_dir());
    let filters: Vec<serde_json::Value> = (0..9)
        .map(|_| serde_json::json!({"field": "state", "op": "eq", "value": "idle"}))
        .collect();
    let arguments = serde_json::json!({ "filters": filters, "limit": 10 })
        .as_object()
        .expect("object")
        .clone();
    let result = crate::mcp::postgresql::call_activity(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(true));
    let message = result.content[0].as_text().expect("text").text.clone();
    assert!(
        message.contains("too many filters"),
        "unexpected message: {message}"
    );
}

#[test]
fn overview_rejects_duplicate_fields_and_a_reversed_window() {
    let config = test_config(std::env::temp_dir());

    let duplicated = serde_json::json!({
        "section": "os_process",
        "fields": ["rmem_kb", "rmem_kb"],
        "from": 0,
        "to": 100,
        "top": 5,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::overview::call(&config, duplicated, &|| false);
    assert_eq!(result.is_error, Some(true));

    let reversed = serde_json::json!({
        "section": "os_process",
        "fields": ["rmem_kb"],
        "from": 100,
        "to": 0,
        "top": 5,
    })
    .as_object()
    .expect("object")
    .clone();
    let result = crate::mcp::overview::call(&config, reversed, &|| false);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn get_context_reports_the_recorded_range_and_field_catalog() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture"), (300, 101, 10, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new(), &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert!(structured["recorded_from"].is_string());
    assert!(structured["recorded_to"].is_string());
    let sections = structured["sections"].as_array().expect("sections array");
    assert!(
        sections.iter().all(|section| !section["logical_name"]
            .as_str()
            .expect("logical name")
            .starts_with("dict.")),
        "store-internal dict.* sections must stay hidden"
    );
    let os_process = sections
        .iter()
        .find(|section| section["logical_name"] == "os_process")
        .expect("os_process section present");
    assert!(
        os_process["identity"]
            .as_array()
            .expect("identity array")
            .contains(&serde_json::json!("pid"))
    );
    let fields = os_process["fields"].as_array().expect("fields array");
    let rmem = fields
        .iter()
        .find(|field| field["name"] == "rmem_kb")
        .expect("rmem_kb field present");
    assert_eq!(rmem["class"], "gauge");
    assert_eq!(rmem["unit"], "kibibytes");
}

#[test]
fn find_events_accepts_the_temp_files_source() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_temp_files"],
        "from": 0,
        "to": 1_000,
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);
    assert_eq!(result.is_error, Some(false));
}

#[test]
fn statements_sort_by_a_derived_field_ranks_by_it() {
    // append_ranked_statements' derived_mean_exec_ms_per_call per queryid:
    // 2 -> 15.0, 1 -> 10.0, 3 -> 5.0 (execution rate over call rate, the
    // shared interval cancels).
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sort": {"field": "derived_mean_exec_ms_per_call", "direction": "desc"},
        "limit": 10,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::postgresql::call_statements(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    let ranked: Vec<f64> = rows
        .iter()
        .map(|row| {
            row["derived_mean_exec_ms_per_call"]
                .as_f64()
                .expect("derived value")
        })
        .collect();
    assert_eq!(ranked, vec![15.0, 10.0, 5.0]);
}

#[test]
fn find_events_reports_more_rows_behind_a_skipped_segment() {
    // Two matching rows fill limit=2 from the first segment; the second
    // segment starts later than everything held, so it is skipped without
    // being opened — its row must still surface as has_more.
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.append_log_error(200);
    fixture.finish_and_continue(1_709_164_800_000_000 + 1_000);
    fixture.append_log_error(300);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let arguments = serde_json::json!({
        "sources": ["pg_log_errors"],
        "from": 0,
        "to": 1_000,
        "limit": 2,
    })
    .as_object()
    .expect("object")
    .clone();

    let result = crate::mcp::events::call(&config, arguments, &|| false);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(
        rows.iter()
            .map(|row| row["at"].as_str().expect("at"))
            .collect::<Vec<_>>(),
        vec!["100", "200"]
    );
    assert_eq!(structured["has_more"], true);
}
