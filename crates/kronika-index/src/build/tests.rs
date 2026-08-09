use super::transaction_rate;

#[test]
fn transactions_per_second_uses_commit_and_rollback_delta_time() {
    assert_eq!(transaction_rate(1_000_000, 10, 3_000_000, 30), Some(10.0));
    assert_eq!(transaction_rate(1_000_000, 30, 3_000_000, 30), Some(0.0));
}

#[test]
fn reset_and_nonpositive_time_have_no_tps() {
    assert_eq!(transaction_rate(1_000_000, 30, 3_000_000, 10), None);
    assert_eq!(transaction_rate(3_000_000, 10, 3_000_000, 30), None);
}
