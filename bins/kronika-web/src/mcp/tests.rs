use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    ACCEPT, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, COOKIE, HOST, HeaderValue, ORIGIN, VARY,
};
use hyper::{Method, Request, StatusCode};
use rmcp::model::{CallToolRequestParams, ErrorCode};
use serde_json::{Value, json};

use super::{
    REQUEST_BODY_BYTES, RESPONSE_BODY_BYTES, STRUCTURED_CONTENT_BYTES, TEXT_SUMMARY_BYTES,
};
use crate::config::{Account, Config};
use crate::{RequestTarget, route_request_at, route_request_without_authentication};

const AUTHORIZATION_VALUE: &str = "Basic dGVzdDpzZWNyZXQ=";
const NOW: u64 = 1_800_000_000;
const EXPECTED_NAMES: [&str; 20] = [
    "kronika_get_context",
    "kronika_list_hours",
    "kronika_rank_heatmap",
    "kronika_list_findings",
    "kronika_get_timeline",
    "kronika_get_host_context",
    "kronika_find_processes",
    "kronika_get_postgresql_overview",
    "kronika_find_postgresql_activity",
    "kronika_find_postgresql_locks",
    "kronika_find_postgresql_vacuum",
    "kronika_find_postgresql_statements",
    "kronika_find_postgresql_plans",
    "kronika_find_postgresql_databases",
    "kronika_find_postgresql_tables",
    "kronika_find_postgresql_indexes",
    "kronika_find_events",
    "kronika_get_metric_history",
    "kronika_get_snapshot",
    "kronika_get_row_detail",
];

fn account() -> Account {
    Account {
        user: "test".to_owned(),
        password: "secret".to_owned(),
    }
}

fn config() -> Config {
    Config {
        data_root: PathBuf::from("unused-mcp-test-root"),
        listen: SocketAddr::from(([127, 0, 0, 1], 0)),
        account: account(),
        authentication_required: true,
        cookie_secure: false,
        sources: 3,
        synthetic_demo: false,
    }
}

fn protocol_request(body: Vec<u8>) -> Request<Full<Bytes>> {
    Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(HOST, "localhost")
        .header(CONTENT_TYPE, "application/json")
        .header(ACCEPT, "application/json, text/event-stream")
        .body(Full::new(Bytes::from(body)))
        .expect("protocol request")
}

async fn protocol_json(request: Request<Full<Bytes>>) -> (StatusCode, hyper::HeaderMap, Value) {
    let directory = tempfile::tempdir().expect("temporary MCP protocol data root");
    let mut web = config();
    web.data_root = directory.path().to_owned();
    let response = super::response(&super::service(&web), request).await;
    let status = response.status();
    let headers = response.headers().clone();
    let body = response
        .into_body()
        .collect()
        .await
        .expect("MCP response body")
        .to_bytes();
    let value = serde_json::from_slice(&body).unwrap_or_else(|error| {
        panic!(
            "JSON response: {error}; status={status}; headers={headers:?}; body={:?}",
            String::from_utf8_lossy(&body)
        )
    });
    (status, headers, value)
}

#[test]
fn catalog_has_exact_stable_surface_names() {
    let names: Vec<_> = super::catalog::all()
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect();
    assert_eq!(names, EXPECTED_NAMES);
}

#[test]
fn catalog_schemas_are_bounded_surface_specific_and_read_only() {
    for tool in super::catalog::all() {
        assert_eq!(
            tool.input_schema.get("additionalProperties"),
            Some(&Value::Bool(false)),
            "{} input schema",
            tool.name
        );
        assert!(
            tool.output_schema.is_none(),
            "{} has no concrete output-schema consumer",
            tool.name
        );
        let annotations = tool.annotations.as_ref().expect("tool annotations");
        assert_eq!(annotations.read_only_hint, Some(true));
        assert_eq!(annotations.destructive_hint, Some(false));
        assert_eq!(annotations.idempotent_hint, Some(true));
        assert_eq!(annotations.open_world_hint, Some(false));
        let schema = serde_json::to_string(&tool.input_schema).expect("input schema JSON");
        assert!(!schema.contains("oneOf"), "{} contains oneOf", tool.name);
        assert!(!schema.contains("\"view\""), "{} contains view", tool.name);
    }
}

#[test]
fn catalog_does_not_advertise_inputs_that_handlers_refuse() {
    for (name, absent) in [
        ("kronika_rank_heatmap", &["fields"][..]),
        ("kronika_find_postgresql_activity", &["find"][..]),
        ("kronika_find_postgresql_locks", &["find", "cursor"][..]),
        ("kronika_find_postgresql_vacuum", &["find", "cursor"][..]),
        ("kronika_find_postgresql_databases", &["find"][..]),
        ("kronika_get_metric_history", &["cursor"][..]),
    ] {
        let tool = super::catalog::find(name).expect("catalog tool");
        let properties = tool
            .input_schema
            .get("properties")
            .and_then(Value::as_object)
            .expect("input properties");
        for parameter in absent {
            assert!(
                !properties.contains_key(*parameter),
                "{name} advertises refused input {parameter}"
            );
        }
    }
}

#[test]
fn mcp_tool_catalog_cost() {
    let catalog = super::catalog::all();
    let descriptor_bytes = serde_json::to_vec(catalog).expect("catalog JSON").len();
    let input_schema_bytes: usize = catalog
        .iter()
        .map(|tool| {
            serde_json::to_vec(&tool.input_schema)
                .expect("input schema JSON")
                .len()
        })
        .sum();
    let output_schema_bytes: usize = catalog
        .iter()
        .filter_map(|tool| tool.output_schema.as_ref())
        .map(|schema| {
            serde_json::to_vec(schema)
                .expect("output schema JSON")
                .len()
        })
        .sum();
    let estimated_tokens = descriptor_bytes.div_ceil(4);
    println!(
        "mcp_tool_catalog_cost tools={} descriptor_bytes={} input_schema_bytes={} output_schema_bytes={} estimated_tokens={}",
        catalog.len(),
        descriptor_bytes,
        input_schema_bytes,
        output_schema_bytes,
        estimated_tokens
    );
    assert_eq!(catalog.len(), 20);
    assert_eq!(descriptor_bytes, 23_671);
    assert_eq!(input_schema_bytes, 16_939);
    assert_eq!(output_schema_bytes, 0);
    assert_eq!(estimated_tokens, 5_918);
}

#[tokio::test]
async fn dispatch_allowlists_known_tools_without_fabricating_data() {
    let directory = tempfile::tempdir().expect("temporary MCP dispatch data root");
    let state = super::State {
        data_root: directory.path().to_owned(),
        sources: 3,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    let known = super::tools::dispatch(
        state.clone(),
        CallToolRequestParams::new("kronika_get_context"),
        || false,
    )
    .await
    .expect("known tool");
    assert_eq!(known.is_error, Some(false));
    assert_eq!(
        known
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/data/context/historical_only")),
        Some(&Value::Bool(true))
    );

    let unknown = super::tools::dispatch(state, CallToolRequestParams::new("run_sql"), || false)
        .await
        .expect_err("unknown tool");
    assert_eq!(unknown.code, ErrorCode::INVALID_PARAMS);
    assert_eq!(unknown.message, "tool not found");
}

#[tokio::test]
async fn dispatch_rejects_arguments_outside_the_advertised_schema() {
    let state = super::State {
        data_root: PathBuf::from("unused-mcp-test-root"),
        sources: 3,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    let mut request = CallToolRequestParams::new("kronika_get_context");
    request.arguments = Some(serde_json::Map::from_iter([(
        "sql".to_owned(),
        json!("select 1"),
    )]));
    let result = super::tools::dispatch(state, request, || false)
        .await
        .expect("known tool");
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/code")),
        Some(&json!("invalid_input"))
    );
    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/parameter")),
        Some(&json!("sql"))
    );
}

#[tokio::test]
async fn dispatch_observes_cancellation_while_waiting_for_scan_admission() {
    let directory = tempfile::tempdir().expect("temporary MCP cancellation data root");
    let checks = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::clone(&checks);
    let state = super::State {
        data_root: directory.path().to_owned(),
        sources: 3,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(0)),
    };

    let result = super::tools::dispatch(
        state,
        CallToolRequestParams::new("kronika_get_context"),
        move || cancelled.fetch_add(1, Ordering::Relaxed) >= 1,
    )
    .await
    .expect("cancelled queued tool");

    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/code")),
        Some(&json!("cancelled"))
    );
    assert!(checks.load(Ordering::Relaxed) >= 2);
}

#[tokio::test]
async fn dispatch_cancels_an_active_scan_and_releases_its_permit() {
    let directory = tempfile::tempdir().expect("temporary MCP cancellation data root");
    let checks = Arc::new(AtomicUsize::new(0));
    let cancelled = Arc::clone(&checks);
    let gate = Arc::new(tokio::sync::Semaphore::new(2));
    let state = super::State {
        data_root: directory.path().to_owned(),
        sources: 3,
        synthetic_demo: false,
        heavy_scans: Arc::clone(&gate),
    };

    let result = super::tools::dispatch(
        state,
        CallToolRequestParams::new("kronika_get_context"),
        move || cancelled.fetch_add(1, Ordering::Relaxed) >= 3,
    )
    .await
    .expect("cancelled active tool");

    assert_eq!(
        result
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/code")),
        Some(&json!("cancelled"))
    );
    assert_eq!(gate.available_permits(), 2);
}

#[tokio::test]
async fn dispatch_enforces_the_requested_structured_data_budget() {
    let directory = tempfile::tempdir().expect("temporary MCP budget data root");
    let state = super::State {
        data_root: directory.path().to_owned(),
        sources: 3,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    let mut below_minimum = CallToolRequestParams::new("kronika_get_context");
    below_minimum.arguments = Some(serde_json::Map::from_iter([(
        "data_budget_bytes".to_owned(),
        json!(1_023),
    )]));
    let invalid = super::tools::dispatch(state.clone(), below_minimum, || false)
        .await
        .expect("invalid data budget");
    assert_eq!(
        invalid
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/parameter")),
        Some(&json!("data_budget_bytes"))
    );

    let mut bounded = CallToolRequestParams::new("kronika_get_context");
    bounded.arguments = Some(serde_json::Map::from_iter([(
        "data_budget_bytes".to_owned(),
        json!(1_024),
    )]));
    let oversized = super::tools::dispatch(state, bounded, || false)
        .await
        .expect("bounded Context tool");
    assert_eq!(
        oversized
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/code")),
        Some(&json!("output_budget_exceeded"))
    );
}

#[test]
fn service_is_stateless_and_uses_exact_transport_caps() {
    let service = super::service(&config());
    assert!(!service.config.legacy_session_mode);
    assert!(service.config.json_response);
    assert_eq!(service.config.max_request_body_bytes, REQUEST_BODY_BYTES);
    assert!(!service.config.stateless_protocol_metadata_required);
    assert!(service.config.allowed_hosts.is_empty());
    assert!(service.config.allowed_origins.is_empty());
    assert_eq!(RESPONSE_BODY_BYTES, 131_072);
    assert_eq!(STRUCTURED_CONTENT_BYTES, 98_304);
    assert_eq!(TEXT_SUMMARY_BYTES, 2_048);
}

#[test]
fn mcp_authentication_and_origin_are_decided_without_a_body_trait() {
    struct NotAnHttpBody;

    let unauthorized = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .body(NotAnHttpBody)
        .expect("request");
    assert_eq!(
        route_request_at(&account(), &unauthorized, NOW)
            .expect_err("missing authentication")
            .response()
            .status(),
        StatusCode::UNAUTHORIZED
    );

    let mut basic = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(AUTHORIZATION, AUTHORIZATION_VALUE)
        .body(NotAnHttpBody)
        .expect("request");
    assert!(matches!(
        route_request_at(&account(), &basic, NOW),
        Ok(RequestTarget::Mcp)
    ));
    basic
        .headers_mut()
        .insert(ORIGIN, HeaderValue::from_static("http://localhost"));
    assert_eq!(
        route_request_at(&account(), &basic, NOW)
            .expect_err("browser Origin")
            .response()
            .status(),
        StatusCode::FORBIDDEN
    );

    let open = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .body(NotAnHttpBody)
        .expect("request");
    assert!(matches!(
        route_request_without_authentication(&open),
        Ok(RequestTarget::Mcp)
    ));
}

#[test]
fn mcp_accepts_the_existing_session_cookie_fallback() {
    struct NotAnHttpBody;

    let issued = crate::auth::issue_cookie(&account(), NOW, false);
    let cookie = issued.split(';').next().expect("cookie pair");
    let request = Request::builder()
        .method(Method::POST)
        .uri("/mcp")
        .header(COOKIE, cookie)
        .body(NotAnHttpBody)
        .expect("request");
    assert!(matches!(
        route_request_at(&account(), &request, NOW),
        Ok(RequestTarget::Mcp)
    ));
}

#[tokio::test]
async fn legacy_initialize_and_tools_list_are_stateless() {
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-03-26",
            "capabilities": {},
            "clientInfo": {"name": "kronika-test", "version": "1"}
        }
    });
    let (status, headers, body) = protocol_json(protocol_request(
        serde_json::to_vec(&initialize).expect("initialize JSON"),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get("mcp-session-id").is_none());
    assert_eq!(
        headers.get(CACHE_CONTROL),
        Some(&HeaderValue::from_static("no-store"))
    );
    assert_eq!(
        headers.get(VARY),
        Some(&HeaderValue::from_static("Authorization, Cookie"))
    );
    assert_eq!(
        body.pointer("/result/capabilities"),
        Some(&json!({"tools": {}}))
    );
    assert!(body.pointer("/result/instructions").is_none());

    let list = json!({"jsonrpc": "2.0", "id": 2, "method": "tools/list"});
    let (status, headers, body) = protocol_json(protocol_request(
        serde_json::to_vec(&list).expect("tools list JSON"),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get("mcp-session-id").is_none());
    assert_eq!(
        body.pointer("/result/tools")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(20)
    );
    assert!(
        body.pointer("/result/tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().all(|tool| tool.get("outputSchema").is_none()))
    );

    let call = json!({
        "jsonrpc": "2.0",
        "id": 5,
        "method": "tools/call",
        "params": {"name": "kronika_get_context", "arguments": {}}
    });
    let (status, headers, body) = protocol_json(protocol_request(
        serde_json::to_vec(&call).expect("tool call JSON"),
    ))
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get("mcp-session-id").is_none());
    assert_eq!(
        body.pointer("/result/structuredContent/data/context/historical_only"),
        Some(&json!(true))
    );
}

#[tokio::test]
async fn modern_discovery_uses_the_official_stateless_sdk_path() {
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientCapabilities": {},
        "io.modelcontextprotocol/clientInfo": {"name": "kronika-test", "version": "1"}
    });
    let discover = json!({
        "jsonrpc": "2.0",
        "id": 3,
        "method": "server/discover",
        "params": {"_meta": meta}
    });
    let mut request = protocol_request(serde_json::to_vec(&discover).expect("discover JSON"));
    request.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2026-07-28"),
    );
    request
        .headers_mut()
        .insert("mcp-method", HeaderValue::from_static("server/discover"));
    let (status, headers, body) = protocol_json(request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get("mcp-session-id").is_none());
    assert_eq!(
        body.pointer("/result/supportedVersions"),
        Some(&json!([
            "2025-03-26",
            "2025-06-18",
            "2025-11-25",
            "2026-07-28",
        ]))
    );
    assert_eq!(
        body.pointer("/result/capabilities"),
        Some(&json!({"tools": {}}))
    );
    assert!(body.pointer("/result/instructions").is_none());
}

#[tokio::test]
async fn modern_tools_list_has_complete_private_cache_fields() {
    let list = json!({
        "jsonrpc": "2.0",
        "id": 4,
        "method": "tools/list",
        "params": {
            "_meta": {
                "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                "io.modelcontextprotocol/clientCapabilities": {},
                "io.modelcontextprotocol/clientInfo": {"name": "kronika-test", "version": "1"}
            }
        }
    });
    let mut request = protocol_request(serde_json::to_vec(&list).expect("tools/list JSON"));
    request.headers_mut().insert(
        "mcp-protocol-version",
        HeaderValue::from_static("2026-07-28"),
    );
    request
        .headers_mut()
        .insert("mcp-method", HeaderValue::from_static("tools/list"));
    let (status, headers, body) = protocol_json(request).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(headers.get("mcp-session-id").is_none());
    assert_eq!(body.pointer("/result/resultType"), Some(&json!("complete")));
    assert_eq!(body.pointer("/result/ttlMs"), Some(&json!(0)));
    assert_eq!(body.pointer("/result/cacheScope"), Some(&json!("private")));
    assert_eq!(
        body.pointer("/result/tools")
            .and_then(Value::as_array)
            .map(Vec::len),
        Some(20)
    );
    assert!(
        body.pointer("/result/tools")
            .and_then(Value::as_array)
            .is_some_and(|tools| tools.iter().all(|tool| tool.get("outputSchema").is_none()))
    );
}

#[tokio::test]
async fn request_body_limit_accepts_exactly_64_kib_and_rejects_the_next_byte() {
    let ping = br#"{"jsonrpc":"2.0","id":4,"method":"ping"}"#.to_vec();
    let mut exact = ping.clone();
    exact.resize(REQUEST_BODY_BYTES, b' ');
    let (status, _headers, body) = protocol_json(protocol_request(exact)).await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let mut oversized = ping;
    oversized.resize(REQUEST_BODY_BYTES + 1, b' ');
    let response = super::response(&super::service(&config()), protocol_request(oversized)).await;
    assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
}
