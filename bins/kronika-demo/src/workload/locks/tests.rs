use super::{lock_update_sql, periodic_chain_keys, round_has_timed_out_tail};

#[test]
fn all_chains_run_in_periodic_rounds() {
    assert_eq!(periodic_chain_keys(1).collect::<Vec<_>>(), [0]);
    assert_eq!(periodic_chain_keys(2).collect::<Vec<_>>(), [0, 1]);
}

#[test]
fn the_default_round_times_out_only_after_an_earlier_waiter_can_run() {
    assert!(round_has_timed_out_tail(4, 4_000));
    assert!(!round_has_timed_out_tail(3, 4_000));
    assert!(!round_has_timed_out_tail(4, 10_000));
}

#[test]
fn every_lock_update_sets_a_finite_statement_timeout() {
    assert_eq!(
        lock_update_sql("tenant_0.workload_0", 7),
        "set local statement_timeout = '10s'; update tenant_0.workload_0 set id = id where id = 7"
    );
}
