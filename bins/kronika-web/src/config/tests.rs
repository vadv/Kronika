use super::account;

#[test]
fn both_halves_make_an_account() {
    let made = account(Some("dba".to_owned()), Some("secret".to_owned()))
        .expect("both halves are an account")
        .expect("an account");
    assert_eq!(made.user, "dba");
    assert_eq!(made.password, "secret");
}

#[test]
fn neither_half_is_no_account_rather_than_an_error() {
    assert!(
        account(None, None)
            .expect("neither half is allowed")
            .is_none()
    );
}

#[test]
fn half_an_account_stops_the_server() {
    assert!(account(Some("dba".to_owned()), None).is_err());
    assert!(account(None, Some("secret".to_owned())).is_err());
}
