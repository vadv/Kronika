use super::{MAX_AGE, Pool};
use std::time::Duration;

/// A DSN nothing listens on; these tests never open a connection.
const UNUSED: &str = "host=/nonexistent dbname=kronika";

fn pool(max_age: Duration) -> Pool {
    Pool::new(UNUSED, max_age).expect("the DSN parses")
}

#[test]
fn a_fresh_pool_holds_no_connection() {
    assert_eq!(pool(MAX_AGE).age(), None);
}

#[test]
fn an_age_limit_above_the_ceiling_is_brought_down_to_it() {
    assert_eq!(pool(Duration::from_hours(24)).max_age, MAX_AGE);
}

#[test]
fn a_shorter_age_limit_is_kept() {
    let minute = Duration::from_mins(1);
    assert_eq!(pool(minute).max_age, minute);
}

#[test]
fn the_ceiling_is_an_hour() {
    assert_eq!(MAX_AGE, Duration::from_hours(1));
}

#[test]
fn closing_a_pool_that_never_opened_is_not_an_error() {
    let mut pool = pool(MAX_AGE);
    pool.close();
    assert_eq!(pool.age(), None);
}

#[test]
fn a_dsn_that_is_not_a_connection_string_is_rejected() {
    assert!(Pool::new("host=", MAX_AGE).is_err());
}

#[test]
fn a_url_dsn_is_accepted_as_well_as_keywords() {
    let pool = Pool::new("postgres://reader@example:5433/appdb", MAX_AGE).expect("the URL parses");
    assert_eq!(pool.config.get_dbname(), Some("appdb"));
}

#[test]
fn another_database_keeps_the_server_and_the_age_limit() {
    let minute = Duration::from_mins(1);
    let other = pool(minute).on_database("payments");
    assert_eq!(other.config.get_dbname(), Some("payments"));
    assert_eq!(other.max_age, minute);
}
