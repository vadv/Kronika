use std::collections::BTreeMap;

use super::{
    HealthMetadata, combined_active_points, overall_points, postgres_health_points,
    transaction_rate,
};
use crate::{ActiveBackendPoint, HealthPoint};

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

fn metadata() -> HealthMetadata {
    HealthMetadata {
        timestamp: 1,
        postgresql_enabled: Some(true),
        postgresql_effective_cpus: Some(2),
        postgresql_interval_seconds: 30,
        pgbouncer_enabled: Some(false),
    }
}

#[test]
fn postgres_health_starts_above_two_active_backends_per_cpu() {
    let points =
        postgres_health_points(&metadata(), &[(10, Some(4)), (20, Some(5)), (30, Some(8))])
            .expect("enabled PostgreSQL has a component");
    assert_eq!(
        points,
        [
            HealthPoint {
                timestamp: 10,
                value: Some(100),
            },
            HealthPoint {
                timestamp: 20,
                value: Some(80),
            },
            HealthPoint {
                timestamp: 30,
                value: Some(50),
            },
        ]
    );
}

#[test]
fn missing_capacity_or_snapshot_is_explicitly_unknown() {
    let mut facts = metadata();
    facts.postgresql_effective_cpus = None;
    assert_eq!(
        postgres_health_points(&facts, &[(10, Some(4))]),
        Some(vec![HealthPoint {
            timestamp: 10,
            value: None,
        }])
    );
    assert_eq!(
        postgres_health_points(&metadata(), &[]),
        Some(vec![HealthPoint {
            timestamp: 1,
            value: None,
        }])
    );
}

#[test]
fn overall_uses_latest_nonfuture_postgres_value_only_through_its_interval() {
    let os = [
        HealthPoint {
            timestamp: 9_000_000,
            value: Some(90),
        },
        HealthPoint {
            timestamp: 40_000_000,
            value: Some(90),
        },
        HealthPoint {
            timestamp: 40_000_001,
            value: Some(90),
        },
    ];
    let postgres = [HealthPoint {
        timestamp: 10_000_000,
        value: Some(80),
    }];
    assert_eq!(
        overall_points(&os, Some(&postgres), &metadata()),
        [
            HealthPoint {
                timestamp: 9_000_000,
                value: None,
            },
            HealthPoint {
                timestamp: 40_000_000,
                value: Some(70),
            },
            HealthPoint {
                timestamp: 40_000_001,
                value: None,
            },
        ]
    );
}

#[test]
fn disabled_sources_cost_nothing_but_configured_pgbouncer_is_unknown() {
    let os = [HealthPoint {
        timestamp: 10,
        value: Some(73),
    }];
    let mut facts = metadata();
    facts.postgresql_enabled = Some(false);
    assert_eq!(overall_points(&os, None, &facts)[0].value, Some(73));
    facts.pgbouncer_enabled = Some(true);
    assert_eq!(overall_points(&os, None, &facts)[0].value, None);
    facts.postgresql_enabled = None;
    facts.pgbouncer_enabled = None;
    assert_eq!(overall_points(&os, None, &facts)[0].value, None);
}

#[test]
fn simultaneous_different_activity_layouts_are_unknown_not_double_counted() {
    let mut activity = BTreeMap::new();
    activity.insert(
        1_001_002,
        vec![ActiveBackendPoint {
            timestamp: 10,
            count: 2,
        }],
    );
    activity.insert(
        1_001_003,
        vec![ActiveBackendPoint {
            timestamp: 10,
            count: 3,
        }],
    );
    assert_eq!(combined_active_points(&activity), vec![(10, None)]);
}
