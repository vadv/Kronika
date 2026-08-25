use std::sync::Arc;

use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{ACCEPT, CONTENT_TYPE, HOST};
use hyper::{Method, Request};
use serde_json::json;

use super::response;
use crate::config::{Account, Config};

// No existing test in this crate builds a `Config`: `bins/kronika-web/src/tests.rs`
// only ever constructs an `Account` (its `account()` helper), because routing
// there is tested through `route_request`/`route_request_at`, which take
// `&Account` rather than the full `Config`. This is the smallest `Config` that
// satisfies the transport, mirroring that file's `account()` helper. The data
// root is never read by `tools/list`, so an arbitrary path is fine.
fn test_config() -> Arc<Config> {
    Arc::new(Config {
        data_root: std::env::temp_dir(),
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
async fn tools_list_returns_the_two_tool_catalog() {
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

    let response = response(test_config(), request).await;
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
    assert_eq!(names, vec!["kronika_overview", "kronika_get_context"]);
}
