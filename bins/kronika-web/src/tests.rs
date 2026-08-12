use http_body_util::BodyExt as _;
use hyper::header::{
    ACCEPT_ENCODING, ALLOW, CACHE_CONTROL, CONTENT_ENCODING, ETAG, IF_NONE_MATCH, VARY,
    WWW_AUTHENTICATE,
};
use hyper::{Method, Request, StatusCode};
use tokio::sync::{mpsc, oneshot};

use super::{
    RequestTarget, authorization, if_none_match_values, response_from_meta, route_request,
};
use crate::api::{CachePolicy, Prepared, ResponseMeta};
use crate::body::StreamHead;
use crate::config::Account;
use crate::encoding::AcceptedEncodings;

mod artifacts;
mod multi_layout;

const AUTHORIZATION: &str = "Basic ZGJhOnNlY3JldA==";

fn account() -> Account {
    Account {
        user: "dba".to_owned(),
        password: "secret".to_owned(),
    }
}

fn request(method: Method, target: &str) -> Request<()> {
    Request::builder()
        .method(method)
        .uri(target)
        .header(hyper::header::AUTHORIZATION, AUTHORIZATION)
        .body(())
        .expect("request")
}

fn rejection(method: Method, target: &str) -> hyper::Response<super::WebBody> {
    route_request(&account(), &request(method, target))
        .expect_err("request is rejected")
        .response()
}

#[test]
fn authentication_is_mandatory_and_central() {
    let request = Request::builder()
        .uri("/api/catalog")
        .body(())
        .expect("request");
    let response = route_request(&account(), &request)
        .expect_err("missing credentials")
        .response();
    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    assert_eq!(
        response.headers().get(WWW_AUTHENTICATE),
        Some(&hyper::header::HeaderValue::from_static(
            "Basic realm=\"kronika\""
        ))
    );
    assert_eq!(response.headers().get(VARY), Some(&auth_header()));
    assert!(
        !response
            .headers()
            .contains_key("access-control-allow-origin")
    );

    let request = Request::builder().uri("/").body(()).expect("UI request");
    assert_eq!(
        route_request(&account(), &request)
            .expect_err("UI also requires credentials")
            .response()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn duplicate_authorization_fields_are_refused() {
    let mut request = request(Method::GET, "/api/catalog");
    request.headers_mut().append(
        hyper::header::AUTHORIZATION,
        hyper::header::HeaderValue::from_static(AUTHORIZATION),
    );
    assert!(authorization(request.headers()).is_none());
    assert_eq!(
        route_request(&account(), &request)
            .expect_err("ambiguous credentials")
            .response()
            .status(),
        StatusCode::UNAUTHORIZED
    );
}

#[test]
fn route_recognition_precedes_the_method_check() {
    let unknown = rejection(Method::POST, "/api/not-a-resource");
    assert_eq!(unknown.status(), StatusCode::NOT_FOUND);

    let known = rejection(Method::POST, "/api/catalog");
    assert_eq!(known.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        known.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET"))
    );

    let api_head = rejection(Method::HEAD, "/api/catalog");
    assert_eq!(api_head.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        api_head.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET"))
    );
}

#[test]
fn only_the_two_exact_ui_paths_admit_get_and_head() {
    assert_eq!(
        route_request(&account(), &request(Method::GET, "/")),
        Ok(RequestTarget::Ui { head: false })
    );
    assert_eq!(
        route_request(&account(), &request(Method::HEAD, "/index.html")),
        Ok(RequestTarget::Ui { head: true })
    );
    assert!(matches!(
        route_request(&account(), &request(Method::GET, "/api/catalog")),
        Ok(RequestTarget::Api {
            route: crate::route::Route::Catalog(_),
            ..
        })
    ));

    let post = rejection(Method::POST, "/");
    assert_eq!(post.status(), StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(
        post.headers().get(ALLOW),
        Some(&hyper::header::HeaderValue::from_static("GET, HEAD"))
    );
    assert_eq!(
        rejection(Method::GET, "/index.html/").status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        rejection(Method::GET, "/api/not-a-resource").status(),
        StatusCode::NOT_FOUND
    );
}

#[test]
fn a_ui_client_that_refuses_gzip_gets_an_explicit_406() {
    let mut request = request(Method::GET, "/");
    request.headers_mut().insert(
        ACCEPT_ENCODING,
        hyper::header::HeaderValue::from_static("identity"),
    );
    let response = route_request(&account(), &request)
        .expect_err("identity-only UI request")
        .response();
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        response.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Accept-Encoding"
        ))
    );
}

#[test]
fn an_api_client_that_refuses_every_coding_gets_an_explicit_406() {
    let mut request = request(Method::GET, "/api/catalog");
    request.headers_mut().insert(
        ACCEPT_ENCODING,
        hyper::header::HeaderValue::from_static("gzip;q=0, identity;q=0"),
    );
    let response = route_request(&account(), &request)
        .expect_err("no API representation is acceptable")
        .response();
    assert_eq!(response.status(), StatusCode::NOT_ACCEPTABLE);
    assert_eq!(
        response.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Accept-Encoding"
        ))
    );
}

#[test]
fn all_if_none_match_fields_reach_the_matcher() {
    let mut request = request(Method::GET, "/api/segments/7/sections/os_cpu/index");
    request.headers_mut().append(
        IF_NONE_MATCH,
        hyper::header::HeaderValue::from_static("\"first\""),
    );
    request.headers_mut().append(
        IF_NONE_MATCH,
        hyper::header::HeaderValue::from_static("W/\"second\", \"third\""),
    );
    assert_eq!(
        if_none_match_values(request.headers()).as_deref(),
        Some("\"first\",W/\"second\", \"third\"")
    );
}

#[tokio::test]
async fn response_metadata_controls_private_cache_headers_etag_and_vary() {
    let (_sender, receiver) = mpsc::channel(1);
    let response = response_from_meta(
        StreamHead {
            meta: ResponseMeta {
                status: StatusCode::NOT_MODIFIED,
                cache: CachePolicy::Revalidate,
                etag: Some("W/\"1234abcd\"".to_owned()),
            },
            coding: None,
        },
        receiver,
    );
    assert_eq!(response.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(
        response.headers().get(CACHE_CONTROL),
        Some(&hyper::header::HeaderValue::from_static("private,no-cache"))
    );
    assert_eq!(
        response.headers().get(ETAG),
        Some(&hyper::header::HeaderValue::from_static("W/\"1234abcd\""))
    );
    assert_eq!(
        response.headers().get(VARY),
        Some(&hyper::header::HeaderValue::from_static(
            "Authorization, Accept-Encoding"
        ))
    );
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    assert!(
        response
            .into_body()
            .collect()
            .await
            .expect("304 body")
            .to_bytes()
            .is_empty()
    );
}

const fn auth_header() -> hyper::header::HeaderValue {
    hyper::header::HeaderValue::from_static("Authorization")
}

#[tokio::test(flavor = "current_thread")]
async fn blocking_resource_work_does_not_stall_the_current_thread_runtime() {
    let (entered_tx, entered_rx) = oneshot::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    let response = tokio::spawn(super::blocking_stream(
        move || {
            let _sent = entered_tx.send(());
            release_rx.recv().expect("release blocking producer");
            Ok(Prepared::Empty(ResponseMeta {
                status: StatusCode::OK,
                cache: CachePolicy::NoStore,
                etag: None,
            }))
        },
        AcceptedEncodings::default(),
    ));

    entered_rx.await.expect("producer entered blocking pool");
    let (serviced_tx, serviced_rx) = oneshot::channel();
    tokio::spawn(async move {
        tokio::task::yield_now().await;
        let _sent = serviced_tx.send(());
    });
    serviced_rx
        .await
        .expect("unrelated future runs while producer is blocked");
    release_tx.send(()).expect("release producer");

    assert_eq!(
        response.await.expect("response task").status(),
        StatusCode::OK
    );
}
