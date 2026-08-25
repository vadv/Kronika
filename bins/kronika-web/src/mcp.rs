//! Stateless Model Context Protocol transport.

use std::convert::Infallible;
use std::sync::Arc;

use http_body_util::BodyExt as _;
use hyper::body::Body;
use hyper::header::{CACHE_CONTROL, HeaderValue, VARY};
use hyper::{Request, Response};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, CompleteRequestMethod, CompleteRequestParams, CompleteResult,
    GetPromptRequestMethod, GetPromptRequestParams, GetPromptResponse, Implementation,
    ListPromptsRequestMethod, ListPromptsResult, ListResourceTemplatesRequestMethod,
    ListResourceTemplatesResult, ListResourcesRequestMethod, ListResourcesResult, ListToolsResult,
    PaginatedRequestParams, ReadResourceRequestMethod, ReadResourceRequestParams,
    ReadResourceResponse, ServerCapabilities, ServerInfo,
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

    fn complete(
        &self,
        _request: CompleteRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CompleteResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Err(rmcp::ErrorData::method_not_found::<
            CompleteRequestMethod,
        >()))
    }

    fn get_prompt(
        &self,
        _request: GetPromptRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<GetPromptResponse, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Err(rmcp::ErrorData::method_not_found::<
            GetPromptRequestMethod,
        >()))
    }

    fn list_prompts(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListPromptsResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Err(rmcp::ErrorData::method_not_found::<
            ListPromptsRequestMethod,
        >()))
    }

    fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourcesResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Err(rmcp::ErrorData::method_not_found::<
            ListResourcesRequestMethod,
        >()))
    }

    fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListResourceTemplatesResult, rmcp::ErrorData>> + Send + '_
    {
        std::future::ready(Err(rmcp::ErrorData::method_not_found::<
            ListResourceTemplatesRequestMethod,
        >()))
    }

    fn read_resource(
        &self,
        _request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ReadResourceResponse, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Err(rmcp::ErrorData::method_not_found::<
            ReadResourceRequestMethod,
        >()))
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
    let response =
        Response::from_parts(parts, body.map_err(infallible_to_body_error).boxed_unsync());
    with_private_headers(response)
}

/// Apply the transport's authentication-sensitive cache policy to any `/mcp` response.
pub(crate) fn with_private_headers(mut response: Response<WebBody>) -> Response<WebBody> {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private,no-store"));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
    response
}

fn infallible_to_body_error(never: Infallible) -> BodyError {
    BodyError::from(never)
}

#[cfg(test)]
mod tests {
    use http_body_util::{BodyExt as _, Full};
    use hyper::body::Bytes;
    use hyper::header::{ACCEPT, CACHE_CONTROL, CONTENT_TYPE, HOST, HeaderMap, VARY};
    use hyper::{Method, Request, StatusCode};
    use serde_json::{Value, json};

    use super::response;

    struct McpResponse {
        status: StatusCode,
        headers: HeaderMap,
        body: Value,
    }

    async fn post(body: Value, headers: &[(&str, &str)]) -> McpResponse {
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
        let headers = response.headers().clone();
        let body = response
            .into_body()
            .collect()
            .await
            .expect("MCP response body")
            .to_bytes();
        let value = serde_json::from_slice(&body).expect("JSON-RPC response");
        McpResponse {
            status,
            headers,
            body: value,
        }
    }

    fn request_meta(version: &str) -> Value {
        json!({
            "io.modelcontextprotocol/protocolVersion": version,
            "io.modelcontextprotocol/clientInfo": {
                "name": "kronika-web-test",
                "version": "1"
            },
            "io.modelcontextprotocol/clientCapabilities": {}
        })
    }

    fn assert_private_stateless(response: &McpResponse) {
        assert_eq!(
            response.headers.get(CACHE_CONTROL),
            Some(&hyper::header::HeaderValue::from_static("private,no-store"))
        );
        assert_eq!(
            response.headers.get(VARY),
            Some(&hyper::header::HeaderValue::from_static(
                "Authorization, Cookie"
            ))
        );
        assert!(!response.headers.contains_key("mcp-session-id"));
    }

    #[tokio::test]
    async fn legacy_initialize_is_tools_only_and_stateless() {
        for version in ["2025-06-18", "2025-11-25"] {
            let response = post(
                json!({
                    "jsonrpc": "2.0",
                    "id": format!("initialize-{version}"),
                    "method": "initialize",
                    "params": {
                        "protocolVersion": version,
                        "capabilities": {},
                        "clientInfo": { "name": "kronika-web-test", "version": "1" }
                    }
                }),
                &[("MCP-Protocol-Version", version)],
            )
            .await;
            assert_eq!(response.status, StatusCode::OK, "{version}");
            assert_eq!(response.body["result"]["protocolVersion"], version);
            assert_eq!(
                response.body["result"]["capabilities"],
                json!({ "tools": {} })
            );
            assert!(response.body["result"].get("resultType").is_none());
            assert!(response.body["result"].get("instructions").is_none());
            assert!(
                response.body["result"]["capabilities"]
                    .get("prompts")
                    .is_none()
            );
            assert!(
                response.body["result"]["capabilities"]
                    .get("resources")
                    .is_none()
            );
            assert_private_stateless(&response);
        }
    }

    #[tokio::test]
    async fn current_discovery_then_tools_list_is_self_contained_and_stateless() {
        const VERSION: &str = "2026-07-28";
        let discovery = post(
            json!({
                "jsonrpc": "2.0",
                "id": "discover",
                "method": "server/discover",
                "params": { "_meta": request_meta(VERSION) }
            }),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "server/discover"),
            ],
        )
        .await;
        assert_eq!(discovery.status, StatusCode::OK);
        assert_eq!(discovery.body["result"]["resultType"], "complete");
        assert_eq!(
            discovery.body["result"]["capabilities"],
            json!({ "tools": {} })
        );
        assert!(
            discovery.body["result"]["supportedVersions"]
                .as_array()
                .is_some_and(|versions| versions.iter().any(|item| item == VERSION))
        );
        assert_eq!(discovery.body["result"]["ttlMs"], 0);
        assert_eq!(discovery.body["result"]["cacheScope"], "private");
        assert_eq!(
            discovery.body["result"]["_meta"]["io.modelcontextprotocol/serverInfo"]["name"],
            "kronika"
        );
        assert!(discovery.body["result"].get("instructions").is_none());
        assert_private_stateless(&discovery);

        let tools = post(
            json!({
                "jsonrpc": "2.0",
                "id": "tools",
                "method": "tools/list",
                "params": { "_meta": request_meta(VERSION) }
            }),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/list"),
            ],
        )
        .await;
        assert_eq!(tools.status, StatusCode::OK);
        assert_eq!(tools.body["result"]["resultType"], "complete");
        assert_eq!(tools.body["result"]["tools"], json!([]));
        assert_eq!(tools.body["result"]["ttlMs"], 0);
        assert_eq!(tools.body["result"]["cacheScope"], "private");
        assert_private_stateless(&tools);
    }

    #[tokio::test]
    async fn non_tool_product_methods_are_not_published_as_empty_collections() {
        let requests = [
            (
                "completion/complete",
                json!({
                    "ref": { "type": "ref/prompt", "name": "unused" },
                    "argument": { "name": "unused", "value": "" }
                }),
            ),
            ("prompts/list", json!({})),
            ("prompts/get", json!({ "name": "unused" })),
            ("resources/list", json!({})),
            ("resources/templates/list", json!({})),
            ("resources/read", json!({ "uri": "kronika://unused" })),
        ];

        for (index, (method, params)) in requests.into_iter().enumerate() {
            let response = post(
                json!({
                    "jsonrpc": "2.0",
                    "id": index,
                    "method": method,
                    "params": params
                }),
                &[("MCP-Protocol-Version", "2025-06-18")],
            )
            .await;
            assert_eq!(response.status, StatusCode::OK, "{method}");
            assert_eq!(response.body["error"]["code"], -32601, "{method}");
            assert_eq!(response.body["error"]["message"], method, "{method}");
            assert_private_stateless(&response);
        }
    }
}
