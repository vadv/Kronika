use super::WorkloadConfig;

fn config() -> WorkloadConfig {
    WorkloadConfig {
        dsn: "host=127.0.0.1 password=private".to_owned(),
        schemas: 4,
        tables_per_schema: 40,
        ddl_concurrency: 4,
        sessions: 4,
        lock_chains: 1,
        lock_chain_depth: 3,
        lock_hold_ms: 4_000,
        lock_round_interval_s: 45,
    }
}

#[test]
fn debug_output_redacts_the_workload_dsn() {
    let output = format!("{:?}", config());
    assert!(output.contains("[redacted]"));
    assert!(!output.contains("private"));
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

    let mut invalid = valid.clone();
    invalid.lock_hold_ms = 0;
    assert!(invalid.validate().is_err());

    let mut invalid = valid;
    invalid.lock_round_interval_s = 0;
    assert!(invalid.validate().is_err());
}
