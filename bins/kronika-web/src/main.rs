//! Serves authenticated, per-segment Kronika resources over HTTP/1.1.
//!
//! Disk, Parquet, index construction, and JSON record production run on Tokio's
//! blocking pool. Request workers await only metadata and a bounded body
//! channel. No data or response cache is retained by the process.
#![allow(
    clippy::multiple_crate_versions,
    reason = "the registry's arrow/parquet stack pulls duplicate transitive versions outside our control"
)]

mod api;
mod auth;
mod body;
mod config;
mod encoding;
mod mcp;
mod mcp_schema;
mod product;
mod route;
mod ui;

use std::convert::Infallible;
use std::fmt;
use std::io::Write as _;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context as _, Result};
use flate2::Compression;
use flate2::write::GzEncoder;
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, COOKIE, ETAG, HeaderValue,
    IF_NONE_MATCH, SET_COOKIE, VARY, WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use api::{ApiError, CachePolicy};
use body::{BodyError, BodyItem, BodyProducer, ChannelBody, StreamHead};
use config::Config;
use encoding::{AcceptedEncodings, ContentCoding};
use product::activity::{ActivityError, ActivityQuery, execute_activity};
use product::execution::Execution;
use product::top_activity::{Query as TopActivityQuery, TopActivityError, execute_top_activity};
use route::RouteError;

type WebBody = UnsyncBoxBody<Bytes, BodyError>;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Arc::new(Config::from_env()?);
    let listener = TcpListener::bind(config.listen)
        .await
        .with_context(|| format!("listen on {}", config.listen))?;
    println!("ready {}", config.listen);

    loop {
        let (stream, _peer) = listener.accept().await.context("accept a connection")?;
        let config = Arc::clone(&config);
        tokio::spawn(async move {
            let service = service_fn(move |request| answer(Arc::clone(&config), request));
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                eprintln!("kronika-web: connection ended: {error}");
            }
        });
    }
}

async fn answer(
    config: Arc<Config>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<WebBody>, Infallible> {
    let is_mcp = request.uri().path() == "/mcp" && request.uri().query().is_none();
    let routed = if config.authentication_required {
        route_request(&config.account, &request)
    } else {
        route_request_without_authentication(&request)
    };
    let target = match routed {
        Ok(target) => target,
        Err(error) => {
            let response = error.response();
            return Ok(if is_mcp {
                mcp::with_private_headers(response)
            } else {
                response
            });
        }
    };
    let if_none_match = if_none_match_values(request.headers());
    Ok(match target {
        RequestTarget::Ui { head, coding } => ui::response(head, if_none_match.as_deref(), coding)
            .unwrap_or_else(|error| {
                eprintln!("kronika-web: serve embedded interface: {error}");
                failed()
            }),
        RequestTarget::Session(session) => {
            session_response(&config.account, config.cookie_secure, session).unwrap_or_else(failed)
        }
        RequestTarget::Api { route, accepted } => {
            streamed(config, route, if_none_match, accepted).await
        }
        RequestTarget::TopActivity { query, accepted } => {
            top_activity_response(config, query, accepted).await
        }
        RequestTarget::Activity { query, accepted } => {
            activity_response(config, query, accepted).await
        }
        RequestTarget::Mcp => {
            mcp::response(request, config.data_root.clone(), config.page_key.clone()).await
        }
    })
}

fn route_request<B>(
    account: &config::Account,
    request: &Request<B>,
) -> Result<RequestTarget, RequestError> {
    route_request_at(account, request, unix_time())
}

fn route_request_without_authentication<B>(
    request: &Request<B>,
) -> Result<RequestTarget, RequestError> {
    route_request_with_authentication(None, request, unix_time())
}

fn route_request_at<B>(
    account: &config::Account,
    request: &Request<B>,
    now: u64,
) -> Result<RequestTarget, RequestError> {
    route_request_with_authentication(Some(account), request, now)
}

fn route_request_with_authentication<B>(
    account: Option<&config::Account>,
    request: &Request<B>,
    now: u64,
) -> Result<RequestTarget, RequestError> {
    let path = request.uri().path();
    if ui::is_path(path) {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return Err(RequestError::MethodNotAllowed("GET, HEAD"));
        }
        let accepted = AcceptedEncodings::from_headers(request.headers())
            .ok_or(RequestError::UiEncodingNotAcceptable)?;
        return Ok(RequestTarget::Ui {
            head: request.method() == Method::HEAD,
            coding: accepted.for_ui(),
        });
    }
    if path == "/auth/session" && request.uri().query().is_none() {
        if request.method() == Method::GET {
            return Ok(RequestTarget::Session(SessionTarget::Check {
                admitted: account
                    .is_none_or(|account| admitted_session(account, request.headers(), now)),
            }));
        }
        if request.method() == Method::POST {
            let Some(account) = account else {
                return Ok(RequestTarget::Session(SessionTarget::Check {
                    admitted: true,
                }));
            };
            let admitted = matches!(
                authorization(request.headers()),
                SingleHeader::Value(value) if auth::admits_basic(account, Some(value))
            );
            return Ok(RequestTarget::Session(SessionTarget::Login {
                issued_at: admitted.then_some(now),
            }));
        }
        if request.method() == Method::DELETE {
            return Ok(RequestTarget::Session(SessionTarget::Clear));
        }
        return Ok(RequestTarget::Session(SessionTarget::MethodNotAllowed));
    }
    if path == "/mcp" && request.uri().query().is_none() {
        validate_mcp_origin(request.headers())?;
        if account.is_some_and(|account| !admitted_api(account, request.headers(), now)) {
            return Err(RequestError::Unauthorized { challenge: true });
        }
        return Ok(RequestTarget::Mcp);
    }
    if path != "/api" && !path.starts_with("/api/") {
        return Err(RequestError::Route(RouteError::NoSuchPath));
    }
    if account.is_some_and(|account| !admitted_api(account, request.headers(), now)) {
        return Err(RequestError::Unauthorized {
            challenge: !is_ui_request(request.headers()),
        });
    }
    if path == "/api/heatmap" {
        let query =
            route::parse_top_activity(request.uri().query()).map_err(RequestError::Route)?;
        if request.method() != Method::GET {
            return Err(RequestError::MethodNotAllowed("GET"));
        }
        let accepted = AcceptedEncodings::from_headers(request.headers())
            .ok_or(RequestError::ApiEncodingNotAcceptable)?;
        return Ok(RequestTarget::TopActivity { query, accepted });
    }
    if path == "/api/postgresql/activity" {
        let query = route::parse_activity(request.uri().query()).map_err(RequestError::Route)?;
        if request.method() != Method::GET {
            return Err(RequestError::MethodNotAllowed("GET"));
        }
        let accepted = AcceptedEncodings::from_headers(request.headers())
            .ok_or(RequestError::ApiEncodingNotAcceptable)?;
        return Ok(RequestTarget::Activity { query, accepted });
    }
    let route = route::parse(path, request.uri().query()).map_err(RequestError::Route)?;
    if request.method() != Method::GET {
        return Err(RequestError::MethodNotAllowed("GET"));
    }
    let accepted = AcceptedEncodings::from_headers(request.headers())
        .ok_or(RequestError::ApiEncodingNotAcceptable)?;
    Ok(RequestTarget::Api { route, accepted })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestTarget {
    Ui {
        head: bool,
        coding: ContentCoding,
    },
    Session(SessionTarget),
    Api {
        route: route::Route,
        accepted: AcceptedEncodings,
    },
    TopActivity {
        query: TopActivityQuery,
        accepted: AcceptedEncodings,
    },
    Activity {
        query: ActivityQuery,
        accepted: AcceptedEncodings,
    },
    Mcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SessionTarget {
    Check { admitted: bool },
    Login { issued_at: Option<u64> },
    Clear,
    MethodNotAllowed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestError {
    Unauthorized { challenge: bool },
    Route(RouteError),
    MethodNotAllowed(&'static str),
    UiEncodingNotAcceptable,
    ApiEncodingNotAcceptable,
    InvalidOrigin,
    OriginNotAllowed,
}

impl RequestError {
    fn response(self) -> Response<WebBody> {
        match self {
            Self::Unauthorized { challenge } => unauthorized(challenge),
            Self::Route(RouteError::NoSuchPath) => {
                refused(StatusCode::NOT_FOUND, "no_such_path", None)
            }
            Self::Route(RouteError::BadParameter(parameter)) => {
                refused(StatusCode::BAD_REQUEST, "bad_parameter", Some(&parameter))
            }
            Self::MethodNotAllowed(allow) => method_not_allowed(allow),
            Self::UiEncodingNotAcceptable => encoding_not_acceptable(false),
            Self::ApiEncodingNotAcceptable => encoding_not_acceptable(true),
            Self::InvalidOrigin => refused(StatusCode::BAD_REQUEST, "invalid_origin", None),
            Self::OriginNotAllowed => refused(StatusCode::FORBIDDEN, "origin_not_allowed", None),
        }
    }
}

fn validate_mcp_origin(headers: &HeaderMap) -> Result<(), RequestError> {
    let origin = unique_header(headers.get_all(hyper::header::ORIGIN).iter());
    let SingleHeader::Value(origin) = origin else {
        return match origin {
            SingleHeader::Absent => Ok(()),
            SingleHeader::Invalid | SingleHeader::Value(_) => Err(RequestError::InvalidOrigin),
        };
    };
    let origin: hyper::Uri = origin
        .parse()
        .map_err(|_error| RequestError::InvalidOrigin)?;
    if !matches!(origin.scheme_str(), Some("http" | "https"))
        || origin.path() != "/"
        || origin.query().is_some()
        || origin.authority().is_none()
    {
        return Err(RequestError::InvalidOrigin);
    }
    Err(RequestError::OriginNotAllowed)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SingleHeader<'a> {
    Absent,
    Value(&'a str),
    Invalid,
}

impl fmt::Debug for SingleHeader<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Absent => f.write_str("Absent"),
            Self::Value(_) => f.write_str("Value([redacted])"),
            Self::Invalid => f.write_str("Invalid"),
        }
    }
}

fn authorization(headers: &HeaderMap) -> SingleHeader<'_> {
    unique_header(headers.get_all(AUTHORIZATION).iter())
}

fn cookie_header(headers: &HeaderMap) -> SingleHeader<'_> {
    unique_header(headers.get_all(COOKIE).iter())
}

fn unique_header<'a>(mut values: impl Iterator<Item = &'a HeaderValue>) -> SingleHeader<'a> {
    let Some(value) = values.next() else {
        return SingleHeader::Absent;
    };
    if values.next().is_some() {
        return SingleHeader::Invalid;
    }
    value
        .to_str()
        .map_or(SingleHeader::Invalid, SingleHeader::Value)
}

fn admitted_session(account: &config::Account, headers: &HeaderMap, now: u64) -> bool {
    matches!(
        cookie_header(headers),
        SingleHeader::Value(value) if auth::admits_session(account, Some(value), now)
    )
}

fn admitted_api(account: &config::Account, headers: &HeaderMap, now: u64) -> bool {
    match authorization(headers) {
        SingleHeader::Value(value) => auth::admits_basic(account, Some(value)),
        SingleHeader::Absent => admitted_session(account, headers, now),
        SingleHeader::Invalid => false,
    }
}

fn is_ui_request(headers: &HeaderMap) -> bool {
    matches!(
        unique_header(headers.get_all("x-kronika-ui").iter()),
        SingleHeader::Value("1")
    )
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}

fn if_none_match_values(headers: &HeaderMap) -> Option<String> {
    let mut combined = String::new();
    for value in headers.get_all(IF_NONE_MATCH) {
        let value = value.to_str().ok()?;
        if !combined.is_empty() {
            combined.push(',');
        }
        combined.push_str(value);
    }
    (!combined.is_empty()).then_some(combined)
}

pub(crate) const PRODUCT_DEADLINE: Duration = Duration::from_secs(30);

struct CancelBlockingRead(Arc<AtomicBool>);

impl Drop for CancelBlockingRead {
    fn drop(&mut self) {
        self.0.store(true, AtomicOrdering::Release);
    }
}

async fn top_activity_response(
    config: Arc<Config>,
    query: TopActivityQuery,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelBlockingRead(Arc::clone(&cancelled));
    let coding = accepted.for_large();
    let data_root = config.data_root.clone();
    let deadline = Instant::now() + PRODUCT_DEADLINE;
    let result = tokio::task::spawn_blocking(move || {
        let observed_cancel = Arc::clone(&cancelled);
        let execution = Execution::new(
            move || observed_cancel.load(AtomicOrdering::Acquire),
            deadline,
        );
        let result = execute_top_activity(&data_root, query, &execution)?;
        let bytes = serde_json::to_vec(&result)
            .map_err(|error| TopActivityError::Read(ApiError::from(error)))?;
        encode_json(bytes, coding)
            .map_err(|error| TopActivityError::Read(ApiError::Unreadable(Box::new(error))))
    })
    .await;
    match result {
        Ok(Ok(bytes)) => product_json_response(bytes, coding),
        Ok(Err(error)) => top_activity_failure(&error),
        Err(error) => {
            eprintln!("kronika-web: top-activity worker failed: {error}");
            failed()
        }
    }
}

async fn activity_response(
    config: Arc<Config>,
    query: ActivityQuery,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    let cancelled = Arc::new(AtomicBool::new(false));
    let _cancel_on_drop = CancelBlockingRead(Arc::clone(&cancelled));
    let coding = accepted.for_large();
    let data_root = config.data_root.clone();
    let page_key = config.page_key.clone();
    let deadline = Instant::now() + PRODUCT_DEADLINE;
    let result = tokio::task::spawn_blocking(move || {
        let observed_cancel = Arc::clone(&cancelled);
        let execution = Execution::new(
            move || observed_cancel.load(AtomicOrdering::Acquire),
            deadline,
        );
        let result = execute_activity(&data_root, &query, &page_key, &execution)?;
        let bytes = serde_json::to_vec(&result).map_err(|_error| {
            ActivityError::ReadFailed("the recorded Activity result could not be encoded")
        })?;
        encode_json(bytes, coding).map_err(|_error| {
            ActivityError::ReadFailed("the recorded Activity result could not be encoded")
        })
    })
    .await;
    match result {
        Ok(Ok(bytes)) => product_json_response(bytes, coding),
        Ok(Err(error)) => activity_failure(error),
        Err(error) => {
            eprintln!("kronika-web: Activity worker failed: {error}");
            failed()
        }
    }
}

fn encode_json(bytes: Vec<u8>, coding: ContentCoding) -> std::io::Result<Vec<u8>> {
    match coding {
        ContentCoding::Identity => Ok(bytes),
        ContentCoding::Gzip => {
            let mut encoder = GzEncoder::new(Vec::new(), Compression::default());
            encoder.write_all(&bytes)?;
            encoder.finish()
        }
    }
}

fn product_json_response(bytes: Vec<u8>, coding: ContentCoding) -> Response<WebBody> {
    let mut response = Response::new(
        Full::new(Bytes::from(bytes))
            .map_err(BodyError::from)
            .boxed_unsync(),
    );
    common_headers(&mut response, CachePolicy::NoStore);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    if let Some(coding) = coding.header() {
        response
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static(coding));
    }
    response
}

fn top_activity_failure(error: &TopActivityError) -> Response<WebBody> {
    let status = match error {
        TopActivityError::Read(_) => StatusCode::INTERNAL_SERVER_ERROR,
        TopActivityError::ResultTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        TopActivityError::Cancelled => StatusCode::REQUEST_TIMEOUT,
        TopActivityError::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
    };
    let mut response = refused(status, error.code(), None);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    response
}

fn activity_failure(error: ActivityError) -> Response<WebBody> {
    let status = match error {
        ActivityError::InvalidArguments(_) => StatusCode::BAD_REQUEST,
        ActivityError::ReadFailed(_) => StatusCode::INTERNAL_SERVER_ERROR,
        ActivityError::ResultTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
        ActivityError::Cancelled => StatusCode::REQUEST_TIMEOUT,
        ActivityError::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
    };
    let mut response = refused(status, error.code(), None);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    response
}

async fn streamed(
    config: Arc<Config>,
    route: route::Route,
    if_none_match: Option<String>,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    blocking_stream_with_replay(
        move || {
            api::prepare_with_demo(
                &config.data_root,
                config.sources,
                config.synthetic_demo,
                route.clone(),
                if_none_match.as_deref(),
                &config.page_key,
            )
        },
        accepted,
    )
    .await
}

#[cfg(test)]
async fn blocking_stream(
    prepare: impl FnOnce() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    let mut prepare = Some(prepare);
    blocking_stream_inner(
        move || {
            let Some(prepare) = prepare.take() else {
                return Err(ApiError::BadCursor);
            };
            prepare()
        },
        accepted,
        false,
    )
    .await
}

async fn blocking_stream_with_replay(
    prepare: impl FnMut() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    blocking_stream_inner(prepare, accepted, true).await
}

async fn blocking_stream_inner(
    mut prepare: impl FnMut() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
    replay_source_change: bool,
) -> Response<WebBody> {
    let (body_tx, body_rx) = mpsc::channel::<BodyItem>(8);
    let (head_tx, head_rx) = oneshot::channel::<Result<StreamHead, ApiError>>();
    let handle = tokio::task::spawn_blocking(move || {
        let mut head_tx = Some(head_tx);
        let mut producer: Option<BodyProducer> = None;
        let mut replayed = false;
        loop {
            let prepared = match prepare() {
                Ok(prepared) => prepared,
                Err(error)
                    if replay_source_change && !replayed && error.source_changed_during_read() =>
                {
                    replayed = true;
                    continue;
                }
                Err(error) => {
                    if let Some(producer) = producer.take() {
                        producer.fail(error);
                    } else if let Some(head_tx) = head_tx.take() {
                        let _sent = head_tx.send(Err(error));
                    }
                    return;
                }
            };
            let meta = prepared.meta();
            if meta.status == StatusCode::NOT_MODIFIED {
                if let Some(producer) = producer.take() {
                    producer.complete_not_modified(meta);
                } else if let Some(head_tx) = head_tx.take() {
                    let _sent = head_tx.send(Ok(StreamHead::not_modified(meta)));
                }
                return;
            }
            if producer.is_some() {
                let restarted = producer
                    .as_mut()
                    .is_some_and(|producer| producer.restart(meta));
                if !restarted {
                    if let Some(producer) = producer.take() {
                        producer.fail(ApiError::Unreadable(Box::new(std::io::Error::other(
                            "response replay began after headers",
                        ))));
                    }
                    return;
                }
            } else {
                let Some(head_tx) = head_tx.take() else {
                    return;
                };
                producer = Some(BodyProducer::new(accepted, meta, head_tx, body_tx.clone()));
            }
            let cancellation_tx = body_tx.clone();
            let cancelled = || cancellation_tx.is_closed();
            let Some(current) = producer.as_mut() else {
                return;
            };
            let result = prepared.stream(&mut |bytes| current.emit(&bytes), &cancelled);
            match result {
                Ok(()) => {
                    if let Some(producer) = producer.take() {
                        producer.complete();
                    }
                    return;
                }
                Err(error)
                    if replay_source_change
                        && !replayed
                        && error.source_changed_during_read()
                        && current.can_restart() =>
                {
                    replayed = true;
                }
                Err(error) => {
                    if let Some(producer) = producer.take() {
                        producer.fail(error);
                    }
                    return;
                }
            }
        }
    });
    drop(handle);

    match head_rx.await {
        Ok(Ok(head)) => response_from_meta(head, body_rx),
        Ok(Err(error)) => {
            if matches!(error, ApiError::Unreadable(_)) {
                eprintln!("kronika-web: resource preparation failed: {error}");
            }
            refused(error.status(), error.code(), error.parameter())
        }
        Err(_closed) => failed(),
    }
}

fn response_from_meta(head: StreamHead, receiver: mpsc::Receiver<BodyItem>) -> Response<WebBody> {
    let body = if head.meta.status == StatusCode::NOT_MODIFIED {
        Full::new(Bytes::new())
            .map_err(BodyError::from)
            .boxed_unsync()
    } else {
        ChannelBody { receiver }.boxed_unsync()
    };
    let mut response = Response::new(body);
    *response.status_mut() = head.meta.status;
    common_headers(&mut response, head.meta.cache);
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
    );
    if head.meta.status != StatusCode::NOT_MODIFIED {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
        );
    }
    if let Some(coding) = head.coding.and_then(ContentCoding::header) {
        response
            .headers_mut()
            .insert(CONTENT_ENCODING, HeaderValue::from_static(coding));
    }
    if let Some(etag) = head.meta.etag
        && let Ok(value) = HeaderValue::from_str(&etag)
    {
        response.headers_mut().insert(ETAG, value);
    }
    response
}

fn refused(status: StatusCode, error: &str, parameter: Option<&str>) -> Response<WebBody> {
    let value = parameter.map_or_else(
        || json!({ "error": error }),
        |parameter| json!({ "error": error, "parameter": parameter }),
    );
    json_response(status, value.to_string())
}

fn failed() -> Response<WebBody> {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({ "error": "unreadable" }).to_string(),
    )
}

fn unauthorized(challenge: bool) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "unauthorized" }).to_string(),
    );
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, X-Kronika-UI"),
    );
    if challenge {
        response.headers_mut().insert(
            WWW_AUTHENTICATE,
            HeaderValue::from_static("Basic realm=\"kronika\""),
        );
    }
    response
}

fn session_response(
    account: &config::Account,
    cookie_secure: bool,
    target: SessionTarget,
) -> Option<Response<WebBody>> {
    let (status, cookie, allow) = match target {
        SessionTarget::Check { admitted: true } => (StatusCode::NO_CONTENT, None, None),
        SessionTarget::Check { admitted: false } | SessionTarget::Login { issued_at: None } => {
            (StatusCode::UNAUTHORIZED, None, None)
        }
        SessionTarget::Login {
            issued_at: Some(now),
        } => (
            StatusCode::NO_CONTENT,
            Some(auth::issue_cookie(account, now, cookie_secure)),
            None,
        ),
        SessionTarget::Clear => (
            StatusCode::NO_CONTENT,
            Some(auth::clear_cookie(cookie_secure)),
            None,
        ),
        SessionTarget::MethodNotAllowed => (
            StatusCode::METHOD_NOT_ALLOWED,
            None,
            Some("GET, POST, DELETE"),
        ),
    };
    let mut response = Response::new(
        Full::new(Bytes::new())
            .map_err(BodyError::from)
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response.headers_mut().insert(
        VARY,
        HeaderValue::from_static("Authorization, Cookie, X-Kronika-UI"),
    );
    if let Some(cookie) = cookie {
        let value = HeaderValue::from_str(&cookie).ok()?;
        response.headers_mut().insert(SET_COOKIE, value);
    }
    if let Some(allow) = allow {
        response
            .headers_mut()
            .insert(ALLOW, HeaderValue::from_static(allow));
    }
    Some(response)
}

fn method_not_allowed(allow: &'static str) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({ "error": "method_not_allowed" }).to_string(),
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static(allow));
    response
}

fn encoding_not_acceptable(api: bool) -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::NOT_ACCEPTABLE,
        json!({ "error": "encoding_not_acceptable" }).to_string(),
    );
    if api {
        response.headers_mut().insert(
            VARY,
            HeaderValue::from_static("Authorization, Cookie, Accept-Encoding"),
        );
    } else {
        ui::set_vary(&mut response);
    }
    response
}

fn json_response(status: StatusCode, body: String) -> Response<WebBody> {
    let mut response = Response::new(
        Full::new(Bytes::from(body))
            .map_err(BodyError::from)
            .boxed_unsync(),
    );
    *response.status_mut() = status;
    common_headers(&mut response, CachePolicy::NoStore);
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
    response
}

fn common_headers(response: &mut Response<WebBody>, cache: CachePolicy) {
    response
        .headers_mut()
        .insert(CACHE_CONTROL, HeaderValue::from_static(cache.header()));
    response
        .headers_mut()
        .insert(VARY, HeaderValue::from_static("Authorization, Cookie"));
}

#[cfg(test)]
mod tests;
