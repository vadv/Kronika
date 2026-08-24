use std::io::{Error, ErrorKind};

use kronika_reader::ReaderError;
use serde_json::json;

#[test]
fn snapshot_source_errors_name_the_active_wal_position() {
    let missing = super::snapshot_active_position(&[]).expect_err("missing snapshot metadata");
    assert_eq!(missing.code, "snapshot_source_unavailable");
    assert_eq!(missing.message, "The snapshot has no active WAL position.");

    let invalid = super::snapshot_active_position(&[json!({
        "record": "snapshot",
        "segment": {"active_wal_position": 7},
    })])
    .expect_err("invalid active WAL position");
    assert_eq!(invalid.code, "snapshot_source_unavailable");
    assert_eq!(
        invalid.message,
        "The snapshot active WAL position is invalid."
    );
}

#[test]
fn api_source_change_is_a_direct_retryable_error() {
    let error = crate::api::ApiError::from(ReaderError::from(Error::new(
        ErrorKind::Interrupted,
        "source moved",
    )));
    let failure = super::api_failure(&error);

    assert_eq!(failure.code, "source_changed");
    assert_eq!(
        failure.message,
        "Source changed during the read; retry the request."
    );
    assert!(failure.retryable);
}
