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
mod route;
mod ui;

use std::convert::Infallible;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full};
use hyper::body::Bytes;
use hyper::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_ENCODING, CONTENT_TYPE, ETAG, HeaderValue,
    IF_NONE_MATCH, VARY, WWW_AUTHENTICATE,
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
use encoding::AcceptedEncodings;
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
    let target = match route_request(&config.account, &request) {
        Ok(target) => target,
        Err(error) => return Ok(error.response()),
    };
    let if_none_match = if_none_match_values(request.headers());
    Ok(match target {
        RequestTarget::Ui { head } => ui::response(head, if_none_match.as_deref()),
        RequestTarget::Api { route, accepted } => {
            streamed(config, route, if_none_match, accepted).await
        }
    })
}

fn route_request<B>(
    account: &config::Account,
    request: &Request<B>,
) -> Result<RequestTarget, RequestError> {
    let path = request.uri().path();
    if ui::is_path(path) {
        if request.method() != Method::GET && request.method() != Method::HEAD {
            return Err(RequestError::MethodNotAllowed("GET, HEAD"));
        }
        let accepted = AcceptedEncodings::from_headers(request.headers())
            .ok_or(RequestError::EncodingNotAcceptable)?;
        if !accepted.allows_gzip() {
            return Err(RequestError::EncodingNotAcceptable);
        }
        return Ok(RequestTarget::Ui {
            head: request.method() == Method::HEAD,
        });
    }
    if path != "/api" && !path.starts_with("/api/") {
        return Err(RequestError::Route(RouteError::NoSuchPath));
    }
    if !auth::admits(account, authorization(request.headers())) {
        return Err(RequestError::Unauthorized);
    }
    let route = route::parse(path, request.uri().query()).map_err(RequestError::Route)?;
    if request.method() != Method::GET {
        return Err(RequestError::MethodNotAllowed("GET"));
    }
    let accepted = AcceptedEncodings::from_headers(request.headers())
        .ok_or(RequestError::EncodingNotAcceptable)?;
    Ok(RequestTarget::Api { route, accepted })
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestTarget {
    Ui {
        head: bool,
    },
    Api {
        route: route::Route,
        accepted: AcceptedEncodings,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestError {
    Unauthorized,
    Route(RouteError),
    MethodNotAllowed(&'static str),
    EncodingNotAcceptable,
}

impl RequestError {
    fn response(self) -> Response<WebBody> {
        match self {
            Self::Unauthorized => unauthorized(),
            Self::Route(RouteError::NoSuchPath) => {
                refused(StatusCode::NOT_FOUND, "no_such_path", None)
            }
            Self::Route(RouteError::BadParameter(parameter)) => {
                refused(StatusCode::BAD_REQUEST, "bad_parameter", Some(&parameter))
            }
            Self::MethodNotAllowed(allow) => method_not_allowed(allow),
            Self::EncodingNotAcceptable => encoding_not_acceptable(),
        }
    }
}

fn authorization(headers: &HeaderMap) -> Option<&str> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let value = values.next()?;
    if values.next().is_some() {
        return None;
    }
    value.to_str().ok()
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

async fn streamed(
    config: Arc<Config>,
    route: route::Route,
    if_none_match: Option<String>,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    blocking_stream(
        move || {
            api::prepare(
                &config.data_root,
                config.sources,
                route,
                if_none_match.as_deref(),
            )
        },
        accepted,
    )
    .await
}

async fn blocking_stream(
    prepare: impl FnOnce() -> Result<api::Prepared, ApiError> + Send + 'static,
    accepted: AcceptedEncodings,
) -> Response<WebBody> {
    let (body_tx, body_rx) = mpsc::channel::<BodyItem>(8);
    let (head_tx, head_rx) = oneshot::channel::<Result<StreamHead, ApiError>>();
    let handle = tokio::task::spawn_blocking(move || match prepare() {
        Ok(prepared) => {
            let meta = prepared.meta();
            if meta.status == StatusCode::NOT_MODIFIED {
                let _sent = head_tx.send(Ok(StreamHead::not_modified(meta)));
                return;
            }
            let cancellation_tx = body_tx.clone();
            let cancelled = || cancellation_tx.is_closed();
            let mut producer = BodyProducer::new(accepted, meta, head_tx, body_tx);
            let result = prepared.stream(&mut |bytes| producer.emit(&bytes), &cancelled);
            match result {
                Ok(()) => producer.complete(),
                Err(error) => producer.fail(error),
            }
        }
        Err(error) => {
            let _sent = head_tx.send(Err(error));
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
        HeaderValue::from_static("Authorization, Accept-Encoding"),
    );
    if head.meta.status != StatusCode::NOT_MODIFIED {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
        );
    }
    if let Some(coding) = head.coding.and_then(encoding::ContentCoding::header) {
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

fn unauthorized() -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::UNAUTHORIZED,
        json!({ "error": "unauthorized" }).to_string(),
    );
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        HeaderValue::from_static("Basic realm=\"kronika\""),
    );
    response
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

fn encoding_not_acceptable() -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::NOT_ACCEPTABLE,
        json!({ "error": "encoding_not_acceptable" }).to_string(),
    );
    ui::set_vary(&mut response);
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
        .insert(VARY, HeaderValue::from_static("Authorization"));
}

#[cfg(test)]
mod tests;
