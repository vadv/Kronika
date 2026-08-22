use super::WorkloadConfig;

fn config() -> WorkloadConfig {
    WorkloadConfig {
        dsn: "host=127.0.0.1 password=private".to_owned(),
        vacuum_dsn: "host=/var/run/postgresql password=also-private".to_owned(),
        schemas: 4,
        tables_per_schema: 40,
        ddl_concurrency: 4,
        sessions: 4,
        lock_chains: 1,
        lock_chain_depth: 4,
        lock_hold_ms: 4_000,
        lock_round_interval_s: 45,
        event_round_interval_s: 60,
        vacuum_rows: 100_000,
        vacuum_round_interval_s: 180,
        vacuum_statement_timeout_s: 30,
    }
}

#[test]
fn debug_output_redacts_the_workload_dsn() {
    let output = format!("{:?}", config());
    assert!(output.contains("[redacted]"));
    assert!(!output.contains("private"));
    assert!(!output.contains("also-private"));
    assert!(!output.contains("127.0.0.1"));
}

#[test]
fn workload_dimensions_and_timers_must_be_positive() {
    let valid = config();
    assert!(valid.validate().is_ok());

    for invalidate in [
        |value: &mut WorkloadConfig| value.schemas = 0,
        |value: &mut WorkloadConfig| value.tables_per_schema = 0,
        |value: &mut WorkloadConfig| value.ddl_concurrency = 0,
        |value: &mut WorkloadConfig| value.sessions = 0,
        |value: &mut WorkloadConfig| value.lock_chains = 0,
        |value: &mut WorkloadConfig| value.vacuum_rows = 0,
    ] {
        let mut invalid = valid.clone();
        invalidate(&mut invalid);
        assert!(invalid.validate().is_err());
    }

    for depth in [0, 1] {
        let mut invalid = valid.clone();
        invalid.lock_chain_depth = depth;
        assert!(invalid.validate().is_err());
    }

    let mut no_timed_out_tail = valid.clone();
    no_timed_out_tail.lock_chain_depth = 3;
    assert!(no_timed_out_tail.validate().is_err());

    let mut invalid = valid.clone();
    invalid.lock_hold_ms = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = valid.clone();
    invalid.lock_round_interval_s = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = valid;
    invalid.event_round_interval_s = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = config();
    invalid.vacuum_round_interval_s = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = config();
    invalid.vacuum_statement_timeout_s = 0;
    assert!(invalid.validate().is_err());
}
