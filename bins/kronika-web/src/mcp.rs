//! Stateless Model Context Protocol transport.

use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::BodyExt as _;
use hyper::body::Body;
use hyper::{Request, Response};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use crate::WebBody;
use crate::body::BodyError;

#[derive(Debug, Clone, Copy)]
struct KronikaMcp;

impl ServerHandler for KronikaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new("kronika", env!("CARGO_PKG_VERSION")).with_title("Kronika"),
        )
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(Vec::new())
            .with_ttl_ms(0)
            .with_cache_scope(CacheScope::Private)))
    }
}

/// Serve one authenticated request through the stateless Streamable HTTP transport.
pub(crate) async fn response<B>(request: Request<B>) -> Response<WebBody>
where
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: std::fmt::Display,
{
    let service = StreamableHttpService::new(
        || Ok(KronikaMcp),
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts()
            .disable_allowed_origins(),
    );
    let response = service.handle(request).await;
    let (parts, body) = response.into_parts();
    Response::from_parts(parts, body.map_err(infallible_to_body_error).boxed_unsync())
}

fn infallible_to_body_error(never: Infallible) -> BodyError {
    BodyError::from(never)
}

#[cfg(test)]
mod tests {
    use http_body_util::{BodyExt as _, Full};
    use hyper::body::Bytes;
    use hyper::header::{ACCEPT, CONTENT_TYPE, HOST};
    use hyper::{Method, Request, StatusCode};
    use serde_json::{Value, json};

    use super::response;

    async fn post(body: Value, headers: &[(&str, &str)]) -> (StatusCode, Value) {
        let bytes = serde_json::to_vec(&body).expect("JSON-RPC request");
        let mut builder = Request::builder()
            .method(Method::POST)
            .uri("http://kronika.test/mcp")
            .header(HOST, "kronika.test")
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream");
        for (name, value) in headers {
            builder = builder.header(*name, *value);
        }
        let request = builder
            .body(Full::new(Bytes::from(bytes)))
            .expect("MCP request");
        let response = response(request).await;
        let status = response.status();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("MCP response body")
            .to_bytes();
        let value = serde_json::from_slice(&body).expect("JSON-RPC response");
        (status, value)
    }

    #[tokio::test]
    async fn initialize_advertises_tools_only_without_global_instructions() {
        let (status, body) = post(
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2026-07-28",
                    "capabilities": {},
                    "clientInfo": { "name": "test-client", "version": "1" }
                }
            }),
            &[("MCP-Protocol-Version", "2026-07-28")],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["protocolVersion"], "2026-07-28");
        assert_eq!(body["result"]["capabilities"], json!({ "tools": {} }));
        assert!(body["result"].get("instructions").is_none());
        assert!(body["result"]["capabilities"].get("prompts").is_none());
        assert!(body["result"]["capabilities"].get("resources").is_none());
    }

    #[tokio::test]
    async fn current_tools_list_is_private_and_not_cacheable() {
        let (status, body) = post(
            json!({
                "jsonrpc": "2.0",
                "id": "tools",
                "method": "tools/list",
                "params": {
                    "_meta": {
                        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                        "io.modelcontextprotocol/clientCapabilities": {}
                    }
                }
            }),
            &[
                ("MCP-Protocol-Version", "2026-07-28"),
                ("Mcp-Method", "tools/list"),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["result"]["tools"], json!([]));
        assert_eq!(body["result"]["ttlMs"], 0);
        assert_eq!(body["result"]["cacheScope"], "private");
    }
}
