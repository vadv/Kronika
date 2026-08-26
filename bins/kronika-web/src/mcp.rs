//! Stateless Model Context Protocol transport: `kronika_overview`,
//! `kronika_get_context`, `kronika_find_postgresql_tables`,
//! `kronika_find_postgresql_indexes`, `kronika_find_postgresql_activity`,
//! `kronika_find_postgresql_locks`, `kronika_find_postgresql_vacuum`,
//! `kronika_find_postgresql_databases`, `kronika_find_postgresql_statements`,
//! `kronika_find_postgresql_plans`, `kronika_find_processes`,
//! `kronika_get_row_detail`, and `kronika_find_events` served over
//! Streamable HTTP.

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

// `ServerHandler` supplies a default for every method (read directly from
// rmcp 3.1.4's `server_handler_methods!` macro, `src/handler/server.rs`).
// The ones this struct leaves unimplemented already behave correctly for a
// tools-only server: `get_prompt`/`read_resource`/`set_level`/`subscribe`/
// `unsubscribe` default to `method_not_found`, and `list_prompts`/
// `list_resources`/`list_resource_templates` default to an empty result,
// which is the correct reply since `get_info` never advertises those
// capabilities. No overrides needed beyond the three below.
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
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, rmcp::ErrorData>> + Send + '_ {
        let config = Arc::clone(&self.config);
        async move {
            // `dispatch` opens segments, parses catalogs and decompresses rows
            // with `std::fs`-based blocking I/O. Running it directly here
            // would tie up a Tokio worker thread for the whole read, the same
            // hazard `main.rs`'s `/api/*` path avoids with `spawn_blocking`.
            let result = tokio::task::spawn_blocking(move || dispatch::dispatch(&config, &request))
                .await
                .unwrap_or_else(|join_error| {
                    semantics::mcp_error(format!("tool dispatch panicked: {join_error}"))
                });
            Ok(CallToolResponse::from(result))
        }
    }
}

/// Serve one authenticated request through the stateless Streamable HTTP
/// transport. Caller has already run the same admission check `/api/*`
/// uses before this is called.
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

/// Every `/mcp` response carries this cache policy: the response depends on
/// who authenticated, so it is never a shared cache candidate.
pub(crate) fn with_private_headers(mut response: Response<WebBody>) -> Response<WebBody> {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("private,no-store"));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
    response
}
