use super::{FinderSurface, cadence_lookback, replay_source_change};

use std::io;

use kronika_reader::ReaderError;

use crate::api::ApiError;

fn stale(message: &'static str) -> ApiError {
    ApiError::Unreadable(Box::new(ReaderError::Io(io::Error::new(
        io::ErrorKind::Interrupted,
        message,
    ))))
}

#[test]
fn source_change_replays_once_then_returns_success() {
    let mut attempts = 0;
    let result = replay_source_change(|| {
        attempts += 1;
        if attempts == 1 {
            Err(stale("first active generation"))
        } else {
            Ok(42)
        }
    });

    assert_eq!(result.expect("second attempt succeeds"), 42);
    assert_eq!(attempts, 2);
}

#[test]
fn repeated_source_change_returns_the_second_error() {
    let mut attempts = 0;
    let error = replay_source_change(|| {
        attempts += 1;
        Err::<(), _>(stale(if attempts == 1 {
            "first active generation"
        } else {
            "second active generation"
        }))
    })
    .expect_err("second source change is returned");

    assert_eq!(attempts, 2);
    assert!(error.to_string().contains("second active generation"));
}

#[test]
fn source_change_replay_does_not_repeat_other_errors() {
    let mut refusal_attempts = 0;
    let refusal = replay_source_change(|| {
        refusal_attempts += 1;
        Err::<(), _>(ApiError::BadFilter("filter".to_owned()))
    })
    .expect_err("refusal is returned");
    assert!(matches!(refusal, ApiError::BadFilter(_)));
    assert_eq!(refusal_attempts, 1);

    let mut cancellation_attempts = 0;
    let cancellation = replay_source_change(|| {
        cancellation_attempts += 1;
        Err::<(), _>(ApiError::Cancelled)
    })
    .expect_err("cancellation is returned");
    assert!(matches!(cancellation, ApiError::Cancelled));
    assert_eq!(cancellation_attempts, 1);
}

#[test]
fn policies_keep_the_nine_surface_cadences() {
    let cases = [
        (FinderSurface::Processes, "os_process", Some(5), 20_000_000),
        (
            FinderSurface::Tables,
            "pg_stat_user_tables",
            Some(300),
            750_000_000,
        ),
        (
            FinderSurface::Indexes,
            "pg_stat_user_indexes",
            Some(300),
            750_000_000,
        ),
        (
            FinderSurface::Activity,
            "pg_stat_activity",
            None,
            75_000_000,
        ),
        (FinderSurface::Locks, "pg_locks", None, 75_000_000),
        (
            FinderSurface::Vacuum,
            "pg_stat_progress_vacuum",
            None,
            75_000_000,
        ),
        (
            FinderSurface::Databases,
            "pg_stat_database",
            None,
            75_000_000,
        ),
        (
            FinderSurface::Statements,
            "pg_stat_statements",
            None,
            75_000_000,
        ),
        (FinderSurface::Plans, "pg_store_plans", None, 75_000_000),
    ];

    for (surface, logical_name, cadence, lookback) in cases {
        let policy = surface.policy();
        assert_eq!(policy.logical_name, logical_name);
        assert_eq!(policy.fixed_cadence_seconds, cadence);
        assert_eq!(
            cadence_lookback(cadence.unwrap_or(30)).expect("bounded cadence"),
            lookback
        );
    }
}

#[test]
fn lookback_has_a_twenty_second_floor_and_checked_arithmetic() {
    assert_eq!(cadence_lookback(0).expect("zero cadence"), 20_000_000);
    assert_eq!(cadence_lookback(8).expect("eight seconds"), 20_000_000);
    assert_eq!(cadence_lookback(9).expect("nine seconds"), 22_500_000);
    assert!(cadence_lookback(u64::MAX).is_err());
}
