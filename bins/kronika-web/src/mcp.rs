//! Stateless Model Context Protocol transport for recorded Kronika history.

mod catalog;
mod expert;
mod postgresql;
mod semantics;
mod tools;

use std::fmt::Display;
use std::path::PathBuf;
use std::sync::Arc;

use http_body_util::{BodyExt as _, Full, Limited};
use hyper::body::{Body, Bytes};
use hyper::header::{CACHE_CONTROL, HeaderValue, VARY};
use hyper::{Request, Response, StatusCode};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, Implementation, ListToolsResult,
    PaginatedRequestParams, ServerCapabilities, ServerInfo, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::never::NeverSessionManager;
use rmcp::transport::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler};
use tokio::sync::Semaphore;

use crate::WebBody;
use crate::body::BodyError;
use crate::config::Config;

pub(crate) const REQUEST_BODY_BYTES: usize = 64 * 1_024;
pub(crate) const RESPONSE_BODY_BYTES: usize = 128 * 1_024;
pub(crate) const STRUCTURED_CONTENT_BYTES: usize = 96 * 1_024;
pub(crate) const TEXT_SUMMARY_BYTES: usize = 2 * 1_024;

const INSTRUCTIONS: &str = "Start with kronika_rank_heatmap and kronika_list_findings for an interval. Kronika reads only recorded history; it never connects to PostgreSQL or executes commands. Ranked activity is not an anomaly or a cause. Drill into the direct Process, PostgreSQL, and Event tools, then request native metric history or exact row detail. Preserve exact timestamps, nulls, units, physical identities, and cursors.";
#[derive(Debug, Clone)]
pub(crate) struct State {
    pub(crate) data_root: PathBuf,
    pub(crate) sources: u32,
    pub(crate) synthetic_demo: bool,
    pub(crate) heavy_scans: Arc<Semaphore>,
}

#[derive(Debug, Clone)]
pub(crate) struct KronikaMcp {
    state: Arc<State>,
}

pub(crate) type Service = StreamableHttpService<KronikaMcp, NeverSessionManager>;

pub(crate) fn service(web: &Config) -> Service {
    // Host admission remains the existing listener/proxy boundary. The router
    // rejects every present Origin before the SDK reads the request body.
    let config = StreamableHttpServerConfig::default()
        .disable_allowed_hosts()
        .disable_allowed_origins()
        .with_legacy_session_mode(false)
        .with_json_response(true)
        .with_max_request_body_bytes(REQUEST_BODY_BYTES)
        .with_stateless_protocol_metadata_required(false)
        .with_sse_keep_alive(None)
        .with_sse_retry(None);
    let handler = KronikaMcp {
        state: Arc::new(State {
            data_root: web.data_root.clone(),
            sources: web.sources,
            synthetic_demo: web.synthetic_demo,
            heavy_scans: Arc::new(Semaphore::new(2)),
        }),
    };
    StreamableHttpService::new(
        move || Ok(handler.clone()),
        Arc::new(NeverSessionManager::default()),
        config,
    )
}

pub(crate) async fn response<B>(service: &Service, request: Request<B>) -> Response<WebBody>
where
    B: Body<Data = Bytes> + Send + 'static,
    B::Error: Display,
{
    let response = service.handle(request).await;
    let (mut parts, body) = response.into_parts();
    let collected = Limited::new(body, RESPONSE_BODY_BYTES).collect().await;
    let bytes = match collected {
        Ok(body) => body.to_bytes(),
        Err(_too_large) => {
            return crate::refused(
                StatusCode::INTERNAL_SERVER_ERROR,
                "output_budget_exceeded",
                None,
            );
        }
    };
    parts
        .headers
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    parts
        .headers
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
    parts.headers.remove("mcp-session-id");
    Response::from_parts(
        parts,
        Full::new(bytes).map_err(BodyError::from).boxed_unsync(),
    )
}

impl ServerHandler for KronikaMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("kronika", env!("CARGO_PKG_VERSION"))
                    .with_title("Kronika historical analysis")
                    .with_description("Read-only analysis of recorded Kronika history"),
            )
            .with_instructions(INSTRUCTIONS)
    }

    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, ErrorData>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(catalog::all().to_vec())))
    }

    fn get_tool(&self, name: &str) -> Option<Tool> {
        catalog::find(name).cloned()
    }

    fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<CallToolResponse, ErrorData>> + Send + '_ {
        let state = self.state.as_ref().clone();
        async move {
            tools::dispatch(state, request, move || context.ct.is_cancelled())
                .await
                .map(Into::into)
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod runtime_tests;
