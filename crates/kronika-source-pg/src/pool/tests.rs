use super::{MAX_AGE, Pool};
use std::time::Duration;

/// A DSN nothing listens on; these tests never open a connection.
const UNUSED: &str = "host=/nonexistent dbname=kronika";

#[test]
fn a_fresh_pool_holds_no_connection() {
    let pool = Pool::new(UNUSED.to_owned(), MAX_AGE);
    assert_eq!(pool.age(), None);
}

#[test]
fn an_age_limit_above_the_ceiling_is_brought_down_to_it() {
    let pool = Pool::new(UNUSED.to_owned(), Duration::from_hours(24));
    assert_eq!(pool.max_age, MAX_AGE);
}

#[test]
fn a_shorter_age_limit_is_kept() {
    let minute = Duration::from_mins(1);
    assert_eq!(Pool::new(UNUSED.to_owned(), minute).max_age, minute);
}

#[test]
fn the_ceiling_is_an_hour() {
    assert_eq!(MAX_AGE, Duration::from_hours(1));
}

#[test]
fn closing_a_pool_that_never_opened_is_not_an_error() {
    let mut pool = Pool::new(UNUSED.to_owned(), MAX_AGE);
    pool.close();
    assert_eq!(pool.age(), None);
}
