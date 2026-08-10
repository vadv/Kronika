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
mod config;
mod route;

use std::convert::Infallible;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{Context, Poll};

use anyhow::{Context as _, Result};
use http_body_util::combinators::UnsyncBoxBody;
use http_body_util::{BodyExt as _, Full};
use hyper::body::{Bytes, Frame, SizeHint};
use hyper::header::{
    ALLOW, AUTHORIZATION, CACHE_CONTROL, CONTENT_TYPE, ETAG, HeaderValue, IF_NONE_MATCH, VARY,
    WWW_AUTHENTICATE,
};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{HeaderMap, Method, Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::json;
use tokio::net::TcpListener;
use tokio::sync::{mpsc, oneshot};

use api::{ApiError, CachePolicy, ResponseMeta};
use config::Config;
use route::RouteError;

type WebBody = UnsyncBoxBody<Bytes, Infallible>;

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
    let route = match route_request(&config.account, &request) {
        Ok(route) => route,
        Err(error) => return Ok(error.response()),
    };
    let if_none_match = if_none_match_values(request.headers());
    Ok(streamed(config, route, if_none_match).await)
}

fn route_request<B>(
    account: &config::Account,
    request: &Request<B>,
) -> Result<route::Route, RequestError> {
    if !auth::admits(account, authorization(request.headers())) {
        return Err(RequestError::Unauthorized);
    }
    let route =
        route::parse(request.uri().path(), request.uri().query()).map_err(RequestError::Route)?;
    if request.method() != Method::GET {
        return Err(RequestError::MethodNotAllowed);
    }
    Ok(route)
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RequestError {
    Unauthorized,
    Route(RouteError),
    MethodNotAllowed,
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
            Self::MethodNotAllowed => method_not_allowed(),
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
) -> Response<WebBody> {
    blocking_stream(move || {
        api::prepare(
            &config.data_root,
            config.sources,
            route,
            if_none_match.as_deref(),
        )
    })
    .await
}

async fn blocking_stream(
    prepare: impl FnOnce() -> Result<api::Prepared, ApiError> + Send + 'static,
) -> Response<WebBody> {
    let (body_tx, body_rx) = mpsc::channel::<Vec<u8>>(8);
    let (meta_tx, meta_rx) = oneshot::channel::<Result<ResponseMeta, ApiError>>();
    let handle = tokio::task::spawn_blocking(move || match prepare() {
        Ok(prepared) => {
            let meta = prepared.meta();
            let no_body = meta.status == StatusCode::NOT_MODIFIED;
            if meta_tx.send(Ok(meta)).is_err() || no_body {
                return;
            }
            let cancellation_tx = body_tx.clone();
            let cancelled = || cancellation_tx.is_closed();
            let mut emit = |bytes| body_tx.blocking_send(bytes).is_ok();
            if let Err(error) = prepared.stream(&mut emit, &cancelled) {
                eprintln!("kronika-web: streamed resource failed: {error}");
                let _sent = emit(error_record());
            }
        }
        Err(error) => {
            let _sent = meta_tx.send(Err(error));
        }
    });
    drop(handle);

    match meta_rx.await {
        Ok(Ok(meta)) => response_from_meta(meta, body_rx),
        Ok(Err(error)) => {
            if matches!(error, ApiError::Unreadable(_)) {
                eprintln!("kronika-web: resource preparation failed: {error}");
            }
            refused(error.status(), error.code(), error.parameter())
        }
        Err(_closed) => failed(),
    }
}

fn response_from_meta(meta: ResponseMeta, receiver: mpsc::Receiver<Vec<u8>>) -> Response<WebBody> {
    let body = if meta.status == StatusCode::NOT_MODIFIED {
        Full::new(Bytes::new()).boxed_unsync()
    } else {
        ChannelBody { receiver }.boxed_unsync()
    };
    let mut response = Response::new(body);
    *response.status_mut() = meta.status;
    common_headers(&mut response, meta.cache);
    if meta.status != StatusCode::NOT_MODIFIED {
        response.headers_mut().insert(
            CONTENT_TYPE,
            HeaderValue::from_static("application/x-ndjson; charset=utf-8"),
        );
    }
    if let Some(etag) = meta.etag
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

fn method_not_allowed() -> Response<WebBody> {
    let mut response = json_response(
        StatusCode::METHOD_NOT_ALLOWED,
        json!({ "error": "method_not_allowed" }).to_string(),
    );
    response
        .headers_mut()
        .insert(ALLOW, HeaderValue::from_static("GET"));
    response
}

fn json_response(status: StatusCode, body: String) -> Response<WebBody> {
    let mut response = Response::new(Full::new(Bytes::from(body)).boxed_unsync());
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

fn error_record() -> Vec<u8> {
    b"{\"record\":\"error\",\"error\":\"unreadable\"}\n".to_vec()
}

struct ChannelBody {
    receiver: mpsc::Receiver<Vec<u8>>,
}

impl hyper::body::Body for ChannelBody {
    type Data = Bytes;
    type Error = Infallible;

    fn poll_frame(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<Frame<Self::Data>, Self::Error>>> {
        self.receiver
            .poll_recv(cx)
            .map(|item| item.map(|bytes| Ok(Frame::data(Bytes::from(bytes)))))
    }

    fn is_end_stream(&self) -> bool {
        self.receiver.is_closed() && self.receiver.is_empty()
    }

    fn size_hint(&self) -> SizeHint {
        SizeHint::default()
    }
}

#[cfg(test)]
mod tests;
