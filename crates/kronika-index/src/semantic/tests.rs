use std::collections::BTreeSet;

use crate::{
    FINDING_SEMANTICS, HEALTH_SEMANTICS, LOCKS_BLOCKED_BY_SEMANTIC, SemanticBoundary,
    SemanticOrigin, finding_semantic, health_semantic, semantic_definition,
};

#[test]
fn evaluator_descriptors_have_unique_stable_ids() {
    let ids = HEALTH_SEMANTICS
        .iter()
        .chain(FINDING_SEMANTICS)
        .map(|definition| definition.id)
        .collect::<BTreeSet<_>>();

    assert_eq!(ids.len(), HEALTH_SEMANTICS.len() + FINDING_SEMANTICS.len());
    assert!(
        HEALTH_SEMANTICS
            .iter()
            .chain(FINDING_SEMANTICS)
            .all(|definition| semantic_definition(definition.id) == Some(*definition))
    );
}

#[test]
fn every_known_bad_evaluator_locator_resolves_to_its_descriptor() {
    let mappings = [
        ((0, 1), "finding.health.overall_health"),
        ((1_102_001, 5), "finding.os_cpu.cpu_busy"),
        ((1_105_001, 1), "finding.os_loadavg.load_per_cpu"),
        ((1_104_001, 3), "finding.os_meminfo.memory_available"),
        ((1_106_001, 11), "finding.os_vmstat.oom_kill_increase"),
        ((1_112_002, 9), "finding.os_mountinfo.mount_used"),
        (
            (1_008_001, 4),
            "finding.pg_stat_archiver.failed_count_increase",
        ),
        ((2_004_001, 6), "finding.pg_log_slow_queries.duration"),
        (
            (2_001_001, 4),
            "finding.pg_log_errors.data_corruption_category",
        ),
        ((1_011_001, 2), "finding.pg_locks.blocked_by_nonempty"),
        ((1_011_002, 2), "finding.pg_locks.blocked_by_nonempty"),
        (
            (1_202_001, 12),
            "finding.os_cgroup_memory.oom_kill_increase",
        ),
        (
            (1_202_002, 13),
            "finding.os_cgroup_memory.oom_kill_increase",
        ),
        (
            (1_001_001, 7),
            "finding.pg_stat_activity.active_backends_per_cpu",
        ),
        (
            (1_001_002, 8),
            "finding.pg_stat_activity.active_backends_per_cpu",
        ),
        (
            (1_001_004, 8),
            "finding.pg_stat_activity.active_backends_per_cpu",
        ),
    ];
    for (locator, expected) in mappings {
        assert_eq!(
            finding_semantic(locator.0, locator.1).map(|definition| definition.id),
            Some(expected),
            "locator {locator:?}",
        );
    }

    for type_id in 1_005_001..=1_005_004 {
        assert_eq!(
            finding_semantic(type_id, 16).map(|definition| definition.id),
            Some("finding.pg_stat_database.deadlocks_increase"),
        );
        assert_eq!(
            finding_semantic(type_id, 20).map(|definition| definition.id),
            Some("finding.pg_stat_database.frozen_xid_age"),
        );
        assert_eq!(
            finding_semantic(type_id, 21).map(|definition| definition.id),
            Some("finding.pg_stat_database.min_mxid_age"),
        );
    }
    for type_id in 1_005_002..=1_005_004 {
        assert_eq!(
            finding_semantic(type_id, 25).map(|definition| definition.id),
            Some("finding.pg_stat_database.checksum_failures_increase"),
        );
    }
    for type_id in [1_005_003, 1_005_004] {
        assert_eq!(
            finding_semantic(type_id, 32).map(|definition| definition.id),
            Some("finding.pg_stat_database.sessions_fatal_increase"),
        );
        assert_eq!(
            finding_semantic(type_id, 33).map(|definition| definition.id),
            Some("finding.pg_stat_database.sessions_killed_increase"),
        );
    }

    assert_eq!(FINDING_SEMANTICS.len(), 18);
    assert!(finding_semantic(1_005_001, 25).is_none());
    assert!(finding_semantic(1_005_002, 32).is_none());
    assert!(finding_semantic(2_004_001, 0).is_none());
}

#[test]
fn lock_descriptor_names_the_boundary_owned_by_the_finding_evaluator() {
    assert_eq!(
        (
            LOCKS_BLOCKED_BY_SEMANTIC.origin,
            LOCKS_BLOCKED_BY_SEMANTIC.operands,
            LOCKS_BLOCKED_BY_SEMANTIC.boundary,
        ),
        (
            SemanticOrigin::KronikaDerived,
            &["blocked_by"][..],
            Some(SemanticBoundary::Nonempty),
        )
    );
}

#[test]
fn emitted_health_series_resolve_to_their_evaluator_descriptors() {
    let resolved = ["os_health", "postgres_health", "overall_health"]
        .into_iter()
        .map(|series| health_semantic(series).map(|definition| definition.id))
        .collect::<Vec<_>>();

    assert_eq!(
        resolved,
        [
            Some("health.os"),
            Some("health.postgresql"),
            Some("health.overall"),
        ]
    );
}
