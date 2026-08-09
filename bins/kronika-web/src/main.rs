//! Serves the recorded history over HTTP.
//!
//! Configuration is environment-only; the one required variable is
//! `KRONIKA_OUT_DIR`. Four routes need no framework, so the server is hyper
//! and a match on the path.
//!
//! Nothing is held between requests: every one opens what it needs and drops
//! it. A process nobody is looking at costs a socket.
#![allow(
    clippy::multiple_crate_versions,
    reason = "the registry's arrow/parquet stack pulls duplicate transitive versions outside our control"
)]

mod api;
mod auth;
mod config;
mod route;

use std::convert::Infallible;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use http_body_util::Full;
use hyper::body::Bytes;
use hyper::header::{AUTHORIZATION, CONTENT_TYPE, WWW_AUTHENTICATE};
use hyper::server::conn::http1;
use hyper::service::service_fn;
use hyper::{Request, Response, StatusCode};
use hyper_util::rt::TokioIo;
use serde_json::{Value, json};
use tokio::net::TcpListener;

use config::Config;
use route::{Route, RouteError};

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
            // A connection that fails is that client's problem, not the
            // server's: log it and keep the listener.
            if let Err(error) = http1::Builder::new()
                .serve_connection(TokioIo::new(stream), service)
                .await
            {
                eprintln!("kronika-web: connection ended: {error}");
            }
        });
    }
}

/// One request, from the header check to the bytes that go back.
#[allow(
    clippy::unused_async,
    reason = "hyper's service takes a future; the handlers become async as they start waiting on disk"
)]
async fn answer(
    config: Arc<Config>,
    request: Request<hyper::body::Incoming>,
) -> Result<Response<Full<Bytes>>, Infallible> {
    let offered = request
        .headers()
        .get(AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    if !auth::admits(config.account.as_ref(), offered) {
        return Ok(unauthorized());
    }
    let target = request
        .uri()
        .path_and_query()
        .map_or_else(|| request.uri().path().to_owned(), ToString::to_string);
    Ok(match route::parse(&target) {
        Ok((Route::Health, window)) => match api::health(&config.data_root, window) {
            Ok(body) => ok(&body),
            Err(error) => {
                eprintln!("kronika-web: {target}: {error}");
                failed("unreadable")
            }
        },
        Ok((Route::Top(request), window)) => match api::top(&config.data_root, window, &request) {
            Ok(body) => ok(&body),
            Err(api::TopError::NoSuchSection) => {
                refused(StatusCode::BAD_REQUEST, "no_such_section")
            }
            Err(api::TopError::NoSuchColumn) => refused(StatusCode::BAD_REQUEST, "no_such_column"),
            Err(api::TopError::Unreadable(error)) => {
                eprintln!("kronika-web: {target}: {error}");
                failed("unreadable")
            }
        },
        Err(RouteError::NoSuchPath) => refused(StatusCode::NOT_FOUND, "no_such_path"),
        Err(RouteError::BadParameter(name)) => json_response(
            StatusCode::BAD_REQUEST,
            &json!({ "error": "bad_parameter", "parameter": name }),
        ),
    })
}

/// A JSON response with the body it was given.
fn ok(body: &Value) -> Response<Full<Bytes>> {
    json_response(StatusCode::OK, body)
}

/// A refusal the interface can act on: a code, not a sentence.
fn refused(status: StatusCode, error: &str) -> Response<Full<Bytes>> {
    json_response(status, &json!({ "error": error }))
}

/// The server could not read what it was asked for.
fn failed(error: &str) -> Response<Full<Bytes>> {
    json_response(
        StatusCode::INTERNAL_SERVER_ERROR,
        &json!({ "error": error }),
    )
}

fn unauthorized() -> Response<Full<Bytes>> {
    let mut response = json_response(
        StatusCode::UNAUTHORIZED,
        &json!({ "error": "unauthorized" }),
    );
    response.headers_mut().insert(
        WWW_AUTHENTICATE,
        "Basic realm=\"kronika\""
            .parse()
            .expect("a static header value"),
    );
    response
}

fn json_response(status: StatusCode, body: &Value) -> Response<Full<Bytes>> {
    Response::builder()
        .status(status)
        .header(CONTENT_TYPE, "application/json")
        .body(Full::new(Bytes::from(body.to_string())))
        .expect("a response built from a status and a body")
}
