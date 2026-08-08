use super::redact;

#[test]
fn a_keyword_password_is_hidden() {
    assert_eq!(
        redact("host=/var/run user=mon password=hunter2 dbname=postgres"),
        "host=/var/run user=mon password=*** dbname=postgres"
    );
}

#[test]
fn a_url_loses_everything_before_the_host() {
    assert_eq!(
        redact("postgres://mon:hunter2@127.0.0.1:6432/pgbouncer"),
        "postgres://***@127.0.0.1:6432/pgbouncer"
    );
}

#[test]
fn a_dsn_without_credentials_is_left_alone() {
    assert_eq!(redact("host=/var/run user=mon"), "host=/var/run user=mon");
    assert_eq!(redact("postgres:///postgres"), "postgres:///postgres");
}
