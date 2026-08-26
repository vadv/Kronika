//! Stateless, tools-only MCP transport over Streamable HTTP.

use std::sync::Arc;

use http_body_util::BodyExt as _;
use hyper::body::Body;
use hyper::header::{CACHE_CONTROL, HeaderValue, VARY};
use hyper::{Request, Response};
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};

use crate::WebBody;
use crate::body::BodyError;
use crate::config::Config;

mod catalog;
mod context;
mod dispatch;
mod event_labels;
mod events;
mod filter;
mod overview;
mod postgresql;
mod processes;
mod row_detail;
mod semantics;

#[cfg(test)]
mod tests;

#[derive(Clone)]
struct KronikaMcp {
    config: Arc<Config>,
}

// Unadvertised prompt and resource methods use rmcp's default responses.
impl ServerHandler for KronikaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("kronika", env!("CARGO_PKG_VERSION")))
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, rmcp::ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(catalog::tools())))
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send + '_ {
        let config = Arc::clone(&self.config);
        async move {
            // Segment reads and decoding block; keep them off Tokio worker
            // threads. The request's cancellation token rides along so an
            // abandoned request stops scanning.
            let token = context.ct.clone();
            let result = tokio::task::spawn_blocking(move || {
                dispatch::dispatch(&config, request, &|| token.is_cancelled())
            })
            .await
            .unwrap_or_else(|join_error| {
                semantics::mcp_error(format!("tool dispatch panicked: {join_error}"))
            });
            Ok(CallToolResponse::from(result))
        }
    }
}

/// Handles an already-authenticated request with stateless Streamable HTTP.
pub(crate) async fn response<B>(config: Arc<Config>, request: Request<B>) -> Response<WebBody>
where
    B: Body + Send + 'static,
    B::Error: std::fmt::Display,
{
    let service = StreamableHttpService::new(
        move || {
            Ok(KronikaMcp {
                config: Arc::clone(&config),
            })
        },
        Arc::new(NeverSessionManager::default()),
        StreamableHttpServerConfig::default()
            .with_legacy_session_mode(false)
            .with_json_response(true)
            .disable_allowed_hosts()
            .disable_allowed_origins(),
    );
    let response = service.handle(request).await;
    let (parts, body) = response.into_parts();
    let response = Response::from_parts(parts, body.map_err(BodyError::from).boxed_unsync());
    with_private_headers(response)
}

/// Makes MCP responses private, non-cacheable, and varied by authentication headers.
pub(crate) fn with_private_headers(mut response: Response<WebBody>) -> Response<WebBody> {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private,no-store"));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
    response
}
