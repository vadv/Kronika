use super::admits;
use crate::config::Account;

fn account() -> Account {
    Account {
        user: "dba".to_owned(),
        password: "secret".to_owned(),
    }
}

/// `dba:secret`, as a browser sends it.
const CORRECT: &str = "Basic ZGJhOnNlY3JldA==";

#[test]
fn a_server_without_an_account_lets_everything_through() {
    assert!(admits(None, None));
    assert!(admits(None, Some("Basic nonsense")));
}

#[test]
fn the_right_credentials_are_admitted() {
    assert!(admits(Some(&account()), Some(CORRECT)));
}

#[test]
fn a_request_without_the_header_is_refused() {
    assert!(!admits(Some(&account()), None));
}

#[test]
fn the_wrong_password_is_refused() {
    // dba:wrong
    assert!(!admits(Some(&account()), Some("Basic ZGJhOndyb25n")));
}

#[test]
fn the_wrong_user_is_refused() {
    // other:secret
    assert!(!admits(Some(&account()), Some("Basic b3RoZXI6c2VjcmV0")));
}

#[test]
fn a_scheme_this_server_does_not_speak_is_refused() {
    assert!(!admits(Some(&account()), Some("Bearer ZGJhOnNlY3JldA==")));
}

#[test]
fn something_that_is_not_base64_is_refused_rather_than_crashing() {
    assert!(!admits(Some(&account()), Some("Basic not-base64!!")));
}

#[test]
fn a_prefix_of_the_credentials_is_refused() {
    // dba:secre
    assert!(!admits(Some(&account()), Some("Basic ZGJhOnNlY3Jl")));
}
