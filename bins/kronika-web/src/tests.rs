use hyper::header::{ALLOW, CACHE_CONTROL, ETAG, IF_NONE_MATCH, VARY, WWW_AUTHENTICATE};
use hyper::{Method, Request, StatusCode};
use tokio::sync::mpsc;

use super::{authorization, if_none_match_values, response_from_meta, route_request};
use crate::api::{CachePolicy, ResponseMeta};
use crate::config::Account;

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

#[test]
fn response_metadata_controls_private_cache_headers_and_etag() {
    let (_sender, receiver) = mpsc::channel(1);
    let response = response_from_meta(
        ResponseMeta {
            status: StatusCode::NOT_MODIFIED,
            cache: CachePolicy::Revalidate,
            etag: Some("\"1234abcd\"".to_owned()),
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
        Some(&hyper::header::HeaderValue::from_static("\"1234abcd\""))
    );
    assert_eq!(response.headers().get(VARY), Some(&auth_header()));
}

const fn auth_header() -> hyper::header::HeaderValue {
    hyper::header::HeaderValue::from_static("Authorization")
}
