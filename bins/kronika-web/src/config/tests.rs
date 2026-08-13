use super::{account, cookie_secure, source_set};

#[test]
fn both_nonempty_credentials_are_required() {
    let made = account(Some("dba".to_owned()), Some("secret".to_owned())).expect("account");
    assert_eq!(made.user, "dba");
    assert_eq!(made.password, "secret");
    assert!(account(None, None).is_err());
    assert!(account(Some(String::new()), Some("secret".to_owned())).is_err());
    assert!(account(Some("dba".to_owned()), Some(String::new())).is_err());
}

#[test]
fn account_debug_output_redacts_credentials() {
    let made = account(Some("dba".to_owned()), Some("secret".to_owned())).expect("account");
    let debug = format!("{made:?}");
    assert_eq!(debug, "Account { credentials: [redacted] }");
    assert!(!debug.contains("dba"));
    assert!(!debug.contains("secret"));
}

#[test]
fn secure_session_cookies_are_an_explicit_tls_option() {
    assert!(!cookie_secure(None).expect("default"));
    assert!(!cookie_secure(Some("false")).expect("HTTP"));
    assert!(cookie_secure(Some("true")).expect("TLS"));
    assert!(cookie_secure(Some("yes")).is_err());
    assert!(cookie_secure(Some("")).is_err());
}

#[test]
fn the_source_bitset_is_explicit_and_typed() {
    assert_eq!(source_set(Some("1".to_owned())).expect("OS"), 1);
    assert_eq!(source_set(Some("3".to_owned())).expect("all sources"), 3);
    assert!(source_set(None).is_err());
    assert!(source_set(Some("postgres".to_owned())).is_err());
    assert!(source_set(Some("4".to_owned())).is_err());
}
