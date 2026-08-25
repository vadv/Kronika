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

// No existing test in this crate builds a `Config`: `bins/kronika-web/src/tests.rs`
// only ever constructs an `Account` (its `account()` helper), because routing
// there is tested through `route_request`/`route_request_at`, which take
// `&Account` rather than the full `Config`. This is the smallest `Config` that
// satisfies the transport, mirroring that file's `account()` helper. Callers
// that don't read the data root (`tools/list`, malformed-argument rejection)
// pass an arbitrary path; callers that do pass a real `Fixture::root()`.
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
async fn tools_list_returns_the_six_tool_catalog() {
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
            "kronika_find_processes",
            "kronika_get_row_detail",
        ]
    );
}

#[test]
fn get_context_lists_recorded_sections_with_row_counts() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "fixture"), (300, 101, 10, "fixture")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let result = crate::mcp::context::call(&config, serde_json::Map::new());

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
    let result = crate::mcp::context::call(&config, serde_json::Map::new());

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["sections"].as_array(), Some(&Vec::new()));
}

// Same values as `rank_only`'s `ranked_process_gauge_rows` fixture
// (`bins/kronika-web/src/tests/artifacts.rs`): five pids, each sampled
// twice, so ranking with top=2 leaves a non-empty "others" band.
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

    let result = crate::mcp::overview::call(&config, arguments);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let entities = structured["entities"].as_array().expect("entities array");
    assert_eq!(entities.len(), 2);
    assert_eq!(entities[0]["total"], 50.0);
    assert_eq!(entities[1]["total"], 45.0);
    assert_eq!(structured["totals_total"], 118.0);
    assert_eq!(structured["others_total"], 63.0);
    assert_eq!(structured["entity_count"], "5");
}

#[test]
fn overview_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::overview::call(&config, serde_json::Map::new());
    assert_eq!(result.is_error, Some(true));
}

#[tokio::test]
async fn overview_end_to_end_through_the_real_transport() {
    // Same fixture as `overview_ranks_the_top_entities_and_reports_the_others_total`,
    // driven through `mcp::response` instead of calling `overview::call` directly.
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
    assert_eq!(entities[1]["total"], 45.0);
    assert_eq!(
        decoded["result"]["structuredContent"]["totals_total"],
        118.0
    );
    assert_eq!(decoded["result"]["structuredContent"]["others_total"], 63.0);
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

    let result = crate::mcp::postgresql::call_tables(&config, arguments);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    // Rows are keyed, not positional — a real field name is directly
    // addressable, and the higher seq_scan rate (alpha: 30 over 100 units)
    // ranks first.
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
    let filtered = crate::mcp::postgresql::call_tables(&config, filtered_arguments);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_structured = filtered.structured_content.expect("structured content");
    let filtered_rows = filtered_structured["rows"].as_array().expect("rows array");
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["relname"], "alpha");
}

#[test]
fn find_postgresql_tables_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::postgresql::call_tables(&config, serde_json::Map::new());
    assert_eq!(result.is_error, Some(true));
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

    let result = crate::mcp::postgresql::call_indexes(&config, arguments);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["indexrelname"], "alpha_pkey");
    assert_eq!(rows[0]["relname"], "alpha");
    assert_eq!(structured["has_more"], false);
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

    let result = crate::mcp::processes::call(&config, arguments);

    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    let rows = structured["rows"].as_array().expect("rows array");
    assert_eq!(rows.len(), 2);
    // Rows are keyed, not positional — pid is directly addressable, and
    // the process with the higher rmem_kb (alpha: 50) ranks first.
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
    let filtered = crate::mcp::processes::call(&config, filtered_arguments);
    assert_eq!(filtered.is_error, Some(false));
    let filtered_structured = filtered.structured_content.expect("structured content");
    let filtered_rows = filtered_structured["rows"].as_array().expect("rows array");
    assert_eq!(filtered_rows.len(), 1);
    assert_eq!(filtered_rows[0]["pid"], 102);
}

#[test]
fn find_processes_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::processes::call(&config, serde_json::Map::new());
    assert_eq!(result.is_error, Some(true));
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
    let listing = crate::mcp::processes::call(&config, listing_arguments);
    assert_eq!(listing.is_error, Some(false));
    let listing_rows = listing.structured_content.expect("structured content")["rows"]
        .as_array()
        .expect("rows array")
        .clone();
    assert_eq!(listing_rows.len(), 1);
    let listing_row = listing_rows[0].clone();

    // Row 0 in the fixture's insertion order is pid 101, the row the
    // listing above narrowed down to.
    let (segment_id, at) =
        crate::mcp::postgresql::current_segment(&config.data_root).expect("current segment");
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

    let detail = crate::mcp::row_detail::call(&config, detail_arguments);

    assert_eq!(detail.is_error, Some(false));
    let detail_row = detail.structured_content.expect("structured content");
    // The exact-locator path and the listing path must agree on content
    // for the same physical row.
    assert_eq!(detail_row["pid"], listing_row["pid"]);
    assert_eq!(detail_row["rmem_kb"], listing_row["rmem_kb"]);
    assert_eq!(detail_row["segment_id"], segment_id.to_string());
    assert_eq!(detail_row["type_id"], type_id.to_string());
    assert_eq!(detail_row["row_ordinal"], "0");
    assert_eq!(detail_row["at"], at.to_string());
}

#[test]
fn get_row_detail_rejects_a_relation_grouped_section() {
    // pg_stat_user_tables is grouped by (datid, relid) at the find_* layer,
    // not addressed by a single physical row ordinal — out of scope for
    // this tool, per its own description.
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100, 1, 11, 0, "db", "public", "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let (segment_id, at) =
        crate::mcp::postgresql::current_segment(&config.data_root).expect("current segment");
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

    let result = crate::mcp::row_detail::call(&config, arguments);
    assert_eq!(result.is_error, Some(true));
}

#[test]
fn get_row_detail_accepts_segment_id_and_row_ordinal_as_decimal_strings() {
    let mut fixture = Fixture::new();
    fixture.append_process_gauge_rows(&[(100, 101, 50, "alpha")]);
    fixture.finish();

    let config = test_config(fixture.root().to_path_buf());
    let (segment_id, at) =
        crate::mcp::postgresql::current_segment(&config.data_root).expect("current segment");
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

    let result = crate::mcp::row_detail::call(&config, arguments);
    assert_eq!(result.is_error, Some(false));
    let structured = result.structured_content.expect("structured content");
    assert_eq!(structured["pid"], 101);
}

#[test]
fn get_row_detail_rejects_malformed_arguments_without_panicking() {
    let config = test_config(std::env::temp_dir());
    let result = crate::mcp::row_detail::call(&config, serde_json::Map::new());
    assert_eq!(result.is_error, Some(true));
}
