//! Stateless Model Context Protocol transport.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use http_body_util::BodyExt as _;
use hyper::body::Body;
use hyper::header::{CACHE_CONTROL, HeaderValue, VARY};
use hyper::{Request, Response};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, CompleteRequestMethod,
    CompleteRequestParams, CompleteResult, ContentBlock, GetPromptRequestMethod,
    GetPromptRequestParams, GetPromptResponse, Implementation, ListPromptsRequestMethod,
    ListPromptsResult, ListResourceTemplatesRequestMethod, ListResourceTemplatesResult,
    ListResourcesRequestMethod, ListResourcesResult, ListToolsResult, PaginatedRequestParams,
    ReadResourceRequestMethod, ReadResourceRequestParams, ReadResourceResponse, ServerCapabilities,
    ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use crate::PRODUCT_DEADLINE;
use crate::WebBody;
use crate::body::BodyError;
use crate::product::activity::{ActivityArgs, ActivityError, execute_activity, normalize_activity};
use crate::product::execution::Execution;
use crate::product::page::PageKey;
use crate::product::top_activity::{
    RawQuery as TopActivityArgs, execute_top_activity, normalize as normalize_top_activity,
};

#[derive(Debug, Clone)]
struct KronikaMcp {
    data_root: PathBuf,
    page_key: PageKey,
}

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
        std::future::ready(Ok(ListToolsResult::with_all_items(
            crate::mcp_schema::tools(),
        )
        .with_ttl_ms(0)
        .with_cache_scope(CacheScope::Private)))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, rmcp::ErrorData> {
        let arguments = serde_json::Value::Object(request.arguments.unwrap_or_default());
        let result = match request.name.as_ref() {
            "kronika_find_top_activity" => {
                let args = match serde_json::from_value::<TopActivityArgs>(arguments) {
                    Ok(args) => args,
                    Err(_error) => {
                        return Ok(tool_error(
                            "invalid_arguments",
                            "arguments do not match the top-activity input schema",
                        )
                        .into());
                    }
                };
                let query = match normalize_top_activity(args) {
                    Ok(query) => query,
                    Err(error) => {
                        return Ok(tool_error("invalid_arguments", error.message()).into());
                    }
                };
                let data_root = self.data_root.clone();
                let cancellation = context.ct.clone();
                let execution = Execution::new(
                    move || cancellation.is_cancelled(),
                    Instant::now() + PRODUCT_DEADLINE,
                );
                match tokio::task::spawn_blocking(move || {
                    execute_top_activity(&data_root, query, &execution)
                })
                .await
                {
                    Ok(Ok(result)) => structured_result(
                        result,
                        "heatmap_read_failed",
                        "recorded top activity could not be encoded",
                    ),
                    Ok(Err(error)) => tool_error(error.code(), error.message()),
                    Err(_error) => tool_error(
                        "heatmap_read_failed",
                        "recorded top activity could not be read",
                    ),
                }
            }
            "kronika_read_postgresql_activity" => {
                let args = match ActivityArgs::from_value(arguments) {
                    Ok(args) => args,
                    Err(error) => return Ok(activity_error(error).into()),
                };
                let query = match normalize_activity(args) {
                    Ok(query) => query,
                    Err(error) => return Ok(activity_error(error).into()),
                };
                let data_root = self.data_root.clone();
                let page_key = self.page_key.clone();
                let cancellation = context.ct.clone();
                let execution = Execution::new(
                    move || cancellation.is_cancelled(),
                    Instant::now() + PRODUCT_DEADLINE,
                );
                match tokio::task::spawn_blocking(move || {
                    execute_activity(&data_root, &query, &page_key, &execution)
                })
                .await
                {
                    Ok(Ok(result)) => structured_result(
                        result,
                        "activity_read_failed",
                        "the recorded Activity result could not be encoded",
                    ),
                    Ok(Err(error)) => activity_error(error),
                    Err(_error) => tool_error(
                        "activity_read_failed",
                        "the pinned recorded Activity data could not be read",
                    ),
                }
            }
            _ => {
                return Err(rmcp::ErrorData::invalid_params(
                    "tool not found",
                    Some(serde_json::json!({ "name": request.name })),
                ));
            }
        };
        Ok(result.into())
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
pub(crate) async fn response<B>(
    request: Request<B>,
    data_root: PathBuf,
    page_key: PageKey,
) -> Response<WebBody>
where
    B: Body + Send + 'static,
    B::Data: Send + 'static,
    B::Error: std::fmt::Display,
{
    let handler = KronikaMcp {
        data_root,
        page_key,
    };
    let service = StreamableHttpService::new(
        move || Ok(handler.clone()),
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

fn structured_result(
    result: impl serde::Serialize,
    encoding_code: &str,
    encoding_message: &str,
) -> CallToolResult {
    match serde_json::to_value(result) {
        Ok(value) => {
            let mut result = CallToolResult::success(Vec::new());
            result.structured_content = Some(value);
            result
        }
        Err(_error) => tool_error(encoding_code, encoding_message),
    }
}

fn activity_error(error: ActivityError) -> CallToolResult {
    tool_error(error.code(), error.message())
}

fn tool_error(code: &str, message: &str) -> CallToolResult {
    CallToolResult::error(vec![ContentBlock::text(
        serde_json::json!({ "error": { "code": code, "message": message } }).to_string(),
    )])
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
    use std::path::{Path, PathBuf};
    use std::sync::Arc;

    use http_body_util::{BodyExt as _, Full};
    use hyper::body::Bytes;
    use hyper::header::{
        ACCEPT, ACCEPT_ENCODING, CACHE_CONTROL, CONTENT_TYPE, HOST, HeaderMap, HeaderValue, VARY,
    };
    use hyper::{Method, Request, StatusCode};
    use kronika_format::DictLimits;
    use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
    use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
    use kronika_registry::pg_stat_activity::PgStatActivityV3;
    use kronika_registry::{StrId, Ts};
    use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
    use serde_json::{Value, json};
    use tokio::sync::Notify;
    use tokio::time::timeout;

    use super::response;
    use crate::config::{Account, Config, SOURCE_POSTGRESQL};
    use crate::encoding::AcceptedEncodings;
    use crate::product::page::PageKey;

    struct McpResponse {
        status: StatusCode,
        headers: HeaderMap,
        body: Value,
    }

    const HOUR_START: i64 = 1_709_164_800_000_000;
    const ACTIVITY_AT: i64 = HOUR_START + 120_000_000;

    struct ProductFixture {
        directory: tempfile::TempDir,
        _writer: WriterOwner,
        journal: Journal,
        address: SegmentAddress,
    }

    #[derive(Clone)]
    struct CancellationProbe {
        started: Arc<Notify>,
        cancelled: Arc<Notify>,
    }

    impl super::ServerHandler for CancellationProbe {
        fn get_info(&self) -> super::ServerInfo {
            super::ServerInfo::new(super::ServerCapabilities::builder().enable_tools().build())
        }

        async fn call_tool(
            &self,
            _request: super::CallToolRequestParams,
            context: super::RequestContext<super::RoleServer>,
        ) -> Result<super::CallToolResponse, rmcp::ErrorData> {
            self.started.notify_one();
            context.ct.cancelled().await;
            self.cancelled.notify_one();
            Ok(super::tool_error("cancelled", "the probe was cancelled").into())
        }
    }

    impl ProductFixture {
        fn new() -> Self {
            let directory = tempfile::tempdir().expect("temporary product root");
            let root = DataRoot::open(directory.path()).expect("open product root");
            let writer = root
                .acquire_writer(LayoutLimits::default())
                .expect("acquire fixture writer");
            let journal =
                Journal::open(&writer, JournalConfig::default()).expect("open fixture journal");
            let address = SegmentAddress::new(SegmentId::new(HOUR_START).expect("segment id"))
                .expect("segment address");
            Self {
                directory,
                _writer: writer,
                journal,
                address,
            }
        }

        fn root(&self) -> &Path {
            self.directory.path()
        }

        fn append_products(&mut self) {
            let mut interner = Interner::new(DictLimits::default());
            let cgroup_path = intern(&mut interner, "/postgresql.slice");
            let database = intern(&mut interner, "appdb");
            let role = intern(&mut interner, "operator");
            let application = intern(&mut interner, "psql");
            let client = intern(&mut interner, "127.0.0.1");
            let backend = intern(&mut interner, "client backend");
            let state = intern(&mut interner, "active");
            let wait_type = intern(&mut interner, "IO");
            let wait_event = intern(&mut interner, "DataFileRead");
            let query = intern(&mut interner, "select * from recorded_table");
            let mut buffers = SectionBuffers::new();
            for (timestamp, usage) in [(HOUR_START + 60_000_000, 100_i64), (ACTIVITY_AT, 220_i64)] {
                buffers
                    .push(OsCgroupCpu {
                        ts: Ts(timestamp),
                        cgroup_path,
                        usage_usec: usage,
                        user_usec: usage,
                        system_usec: 0,
                        throttled_usec: 0,
                        nr_throttled: 0,
                        quota_usec: -1,
                        period_usec: 100_000,
                        scope: 3,
                    })
                    .expect("cgroup row fits");
            }
            buffers
                .push(PgStatActivityV3 {
                    ts: Ts(ACTIVITY_AT),
                    pid: 42,
                    leader_pid: None,
                    datid: Some(7),
                    datname: Some(database),
                    usename: Some(role),
                    application_name: application,
                    client_addr: client,
                    backend_type: backend,
                    state: Some(state),
                    wait_event_type: Some(wait_type),
                    wait_event: Some(wait_event),
                    query: Some(query),
                    query_id: Some(-9),
                    backend_xid_age: Some(11),
                    backend_xmin_age: Some(12),
                    backend_start: Ts(ACTIVITY_AT - 10_000_000),
                    xact_start: Some(Ts(ACTIVITY_AT - 5_000_000)),
                    query_start: Some(Ts(ACTIVITY_AT - 2_000_000)),
                    state_change: Some(Ts(ACTIVITY_AT - 1_000_000)),
                })
                .expect("Activity row fits");
            let dictionary = dict::encode(interner.window()).expect("encode fixture dictionary");
            let part = buffers
                .flush(&dictionary)
                .expect("encode fixture part")
                .expect("nonempty fixture part");
            self.journal
                .append(self.address.id, &part)
                .expect("append fixture part");
        }
    }

    fn intern(interner: &mut Interner, value: &str) -> StrId {
        StrId(
            interner
                .intern(value.as_bytes())
                .expect("intern fixture text")
                .get(),
        )
    }

    async fn post(body: Value, headers: &[(&str, &str)]) -> McpResponse {
        let root = tempfile::tempdir().expect("empty recorded store root");
        post_at(
            root.path().to_path_buf(),
            PageKey::derive(b"mcp-test-account"),
            body,
            headers,
        )
        .await
    }

    async fn post_at(
        root: PathBuf,
        page_key: PageKey,
        body: Value,
        headers: &[(&str, &str)],
    ) -> McpResponse {
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
        let response = response(request, root, page_key).await;
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
            Some(&HeaderValue::from_static("private,no-store"))
        );
        assert_eq!(
            response.headers.get(VARY),
            Some(&HeaderValue::from_static("Authorization, Cookie"))
        );
        assert!(!response.headers.contains_key("mcp-session-id"));
    }

    fn tool_call(name: &str, arguments: &Value, version: &str) -> Value {
        json!({
            "jsonrpc": "2.0",
            "id": format!("call-{name}"),
            "method": "tools/call",
            "params": {
                "name": name,
                "arguments": arguments,
                "_meta": request_meta(version)
            }
        })
    }

    fn assert_success(response: &McpResponse, modern: bool) -> &Value {
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body["result"]["content"], json!([]));
        assert!(
            response.body["result"]
                .get("isError")
                .is_none_or(|value| value == false)
        );
        if modern {
            assert_eq!(response.body["result"]["resultType"], "complete");
        } else {
            assert!(response.body["result"].get("resultType").is_none());
        }
        assert_private_stateless(response);
        &response.body["result"]["structuredContent"]
    }

    fn assert_tool_failure(response: &McpResponse, code: &str) {
        assert_eq!(response.status, StatusCode::OK);
        assert_eq!(response.body["result"]["isError"], true);
        assert!(response.body["result"].get("structuredContent").is_none());
        let text = response.body["result"]["content"][0]["text"]
            .as_str()
            .expect("one textual tool failure");
        let failure: Value = serde_json::from_str(text).expect("structured textual failure");
        assert_eq!(failure["error"]["code"], code);
        assert_private_stateless(response);
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
        let listed = tools.body["result"]["tools"]
            .as_array()
            .expect("tool descriptors");
        assert_eq!(listed.len(), 2);
        assert_eq!(listed[0]["name"], "kronika_find_top_activity");
        assert_eq!(
            listed[0]["title"],
            "Find top load across system and PostgreSQL"
        );
        assert_eq!(listed[1]["name"], "kronika_read_postgresql_activity");
        assert_eq!(
            listed[1]["title"],
            "Read recorded PostgreSQL backend activity"
        );
        assert_eq!(tools.body["result"]["ttlMs"], 0);
        assert_eq!(tools.body["result"]["cacheScope"], "private");
        assert_private_stateless(&tools);
    }

    #[tokio::test]
    async fn legacy_http_lists_and_calls_both_recorded_product_tools() {
        const VERSION: &str = "2025-11-25";
        let mut fixture = ProductFixture::new();
        fixture.append_products();
        let root = fixture.root().to_path_buf();
        let key = PageKey::derive(b"legacy-http-product-calls");

        let listed = post_at(
            root.clone(),
            key.clone(),
            json!({
                "jsonrpc": "2.0",
                "id": "legacy-tools",
                "method": "tools/list",
                "params": {}
            }),
            &[("MCP-Protocol-Version", VERSION)],
        )
        .await;
        assert_eq!(listed.status, StatusCode::OK);
        assert_eq!(
            listed.body["result"]["tools"].as_array().map(Vec::len),
            Some(2)
        );
        assert!(listed.body["result"].get("resultType").is_none());
        assert_private_stateless(&listed);

        let top = post_at(
            root.clone(),
            key.clone(),
            tool_call(
                "kronika_find_top_activity",
                &json!({
                    "hour": HOUR_START.to_string(),
                    "surface": "cgroup_cpu",
                    "metric": "cg_cpu",
                    "top": 10
                }),
                VERSION,
            ),
            &[("MCP-Protocol-Version", VERSION)],
        )
        .await;
        let top_result = assert_success(&top, false);
        assert_eq!(top_result["surface"], "cgroup_cpu");
        assert_eq!(top_result["metric"], "cg_cpu");
        assert_eq!(top_result["top"], 1);
        assert_eq!(top_result["rows"][0]["entity"]["kind"], "cgroup_cpu");
        assert_eq!(top_result["rows"][0]["entity"]["path"], "/postgresql.slice");

        let activity = post_at(
            root,
            key,
            tool_call(
                "kronika_read_postgresql_activity",
                &json!({
                    "at": ACTIVITY_AT.to_string(),
                    "sort": "pid",
                    "direction": "asc"
                }),
                VERSION,
            ),
            &[("MCP-Protocol-Version", VERSION)],
        )
        .await;
        let activity_result = assert_success(&activity, false);
        assert_eq!(activity_result["requested_at"], ACTIVITY_AT.to_string());
        assert_eq!(activity_result["observed_at"], ACTIVITY_AT.to_string());
        assert_eq!(activity_result["rows"].as_array().map(Vec::len), Some(1));
        assert_eq!(activity_result["rows"][0]["pid"], 42);
        assert_eq!(activity_result["rows"][0]["query_id"], "-9");
    }

    #[tokio::test]
    async fn current_http_discovery_calls_both_tools_with_modern_headers() {
        const VERSION: &str = "2026-07-28";
        let mut fixture = ProductFixture::new();
        fixture.append_products();
        let root = fixture.root().to_path_buf();
        let key = PageKey::derive(b"current-http-product-calls");

        let top = post_at(
            root.clone(),
            key.clone(),
            tool_call(
                "kronika_find_top_activity",
                &json!({
                    "hour": HOUR_START.to_string(),
                    "surface": "cgroup_cpu"
                }),
                VERSION,
            ),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "kronika_find_top_activity"),
            ],
        )
        .await;
        assert_eq!(assert_success(&top, true)["rows"][0]["total"], 120.0);

        let activity = post_at(
            root,
            key,
            tool_call(
                "kronika_read_postgresql_activity",
                &json!({"at": ACTIVITY_AT.to_string()}),
                VERSION,
            ),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "kronika_read_postgresql_activity"),
            ],
        )
        .await;
        assert_eq!(
            assert_success(&activity, true)["rows"][0]["backend_type"],
            "client backend"
        );
    }

    #[tokio::test]
    async fn product_http_and_mcp_envelopes_carry_identical_typed_results() {
        const VERSION: &str = "2026-07-28";
        let mut fixture = ProductFixture::new();
        fixture.append_products();
        let root = fixture.root().to_path_buf();
        let key = PageKey::derive(b"http-mcp-activity-parity");
        let mut encoding_headers = HeaderMap::new();
        encoding_headers.insert(ACCEPT_ENCODING, HeaderValue::from_static("identity"));
        let accepted = AcceptedEncodings::from_headers(&encoding_headers).expect("identity coding");
        let config = Arc::new(Config {
            data_root: root.clone(),
            listen: "127.0.0.1:0".parse().expect("test listen address"),
            account: Account {
                user: "dba".to_owned(),
                password: "secret".to_owned(),
            },
            page_key: key.clone(),
            authentication_required: true,
            cookie_secure: false,
            sources: SOURCE_POSTGRESQL,
            synthetic_demo: false,
        });

        let top_query = crate::route::parse_top_activity(Some(&format!(
            "hour={HOUR_START}&surface=cgroup_cpu&metric=cg_cpu&top=10"
        )))
        .expect("semantic top-activity HTTP query");
        let top_http = crate::top_activity_response(Arc::clone(&config), top_query, accepted).await;
        assert_eq!(top_http.status(), StatusCode::OK);
        let top_http_value: Value = serde_json::from_slice(
            &top_http
                .into_body()
                .collect()
                .await
                .expect("top-activity HTTP response body")
                .to_bytes(),
        )
        .expect("typed top-activity HTTP result");
        let top_mcp = post_at(
            root.clone(),
            key.clone(),
            tool_call(
                "kronika_find_top_activity",
                &json!({
                    "hour": HOUR_START.to_string(),
                    "surface": "cgroup_cpu",
                    "metric": "cg_cpu",
                    "top": 10
                }),
                VERSION,
            ),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "kronika_find_top_activity"),
            ],
        )
        .await;
        assert_eq!(top_http_value, *assert_success(&top_mcp, true));

        let activity_query = crate::route::parse_activity(Some(&format!(
            "at={ACTIVITY_AT}&sort=pid&direction=asc&page_size=1"
        )))
        .expect("semantic Activity HTTP query");
        let http = crate::activity_response(config, activity_query, accepted).await;
        assert_eq!(http.status(), StatusCode::OK);
        assert_eq!(
            http.headers().get(CACHE_CONTROL),
            Some(&HeaderValue::from_static("private,no-store"))
        );
        let http_value: Value = serde_json::from_slice(
            &http
                .into_body()
                .collect()
                .await
                .expect("Activity HTTP response body")
                .to_bytes(),
        )
        .expect("typed Activity HTTP result");

        let mcp = post_at(
            root,
            key,
            tool_call(
                "kronika_read_postgresql_activity",
                &json!({
                    "at": ACTIVITY_AT.to_string(),
                    "sort": "pid",
                    "direction": "asc",
                    "page_size": 1
                }),
                VERSION,
            ),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "kronika_read_postgresql_activity"),
            ],
        )
        .await;
        assert_eq!(http_value, *assert_success(&mcp, true));
    }

    #[tokio::test]
    async fn real_http_tool_calls_return_stable_product_errors() {
        const VERSION: &str = "2026-07-28";
        let top = post(
            tool_call(
                "kronika_find_top_activity",
                &json!({
                    "hour": HOUR_START.to_string(),
                    "surface": "processes",
                    "metric": "cg_cpu"
                }),
                VERSION,
            ),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "kronika_find_top_activity"),
            ],
        )
        .await;
        assert_tool_failure(&top, "invalid_arguments");

        let activity = post(
            tool_call(
                "kronika_read_postgresql_activity",
                &json!({"at": "00"}),
                VERSION,
            ),
            &[
                ("MCP-Protocol-Version", VERSION),
                ("Mcp-Method", "tools/call"),
                ("Mcp-Name", "kronika_read_postgresql_activity"),
            ],
        )
        .await;
        assert_tool_failure(&activity, "invalid_arguments");
    }

    #[tokio::test]
    async fn stateless_http_disconnect_cancels_the_in_flight_tool_context() {
        const VERSION: &str = "2025-11-25";
        let started = Arc::new(Notify::new());
        let cancelled = Arc::new(Notify::new());
        let probe = CancellationProbe {
            started: Arc::clone(&started),
            cancelled: Arc::clone(&cancelled),
        };
        let service = super::StreamableHttpService::new(
            move || Ok(probe.clone()),
            Arc::new(super::NeverSessionManager::default()),
            super::StreamableHttpServerConfig::default()
                .with_legacy_session_mode(false)
                .with_json_response(true)
                .disable_allowed_hosts()
                .disable_allowed_origins(),
        );
        let bytes =
            serde_json::to_vec(&tool_call("probe", &json!({}), VERSION)).expect("probe request");
        let request = Request::builder()
            .method(Method::POST)
            .uri("http://kronika.test/mcp")
            .header(HOST, "kronika.test")
            .header(CONTENT_TYPE, "application/json")
            .header(ACCEPT, "application/json, text/event-stream")
            .header("MCP-Protocol-Version", VERSION)
            .body(Full::new(Bytes::from(bytes)))
            .expect("HTTP probe request");
        let in_flight = tokio::spawn(async move { service.handle(request).await });
        timeout(std::time::Duration::from_secs(2), started.notified())
            .await
            .expect("tool handler started");
        in_flight.abort();
        assert!(in_flight.await.is_err(), "HTTP client task was aborted");
        timeout(std::time::Duration::from_secs(2), cancelled.notified())
            .await
            .expect("disconnect reached the tool cancellation token");
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
