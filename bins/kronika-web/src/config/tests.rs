use super::{account, source_set};

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
fn the_source_bitset_is_explicit_and_typed() {
    assert_eq!(source_set(Some("5".to_owned())).expect("bitset"), 5);
    assert!(source_set(None).is_err());
    assert!(source_set(Some("postgres".to_owned())).is_err());
}
