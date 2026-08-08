use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kronika_source_pg::Pool;
use kronika_source_pg::databases::Database;
use kronika_source_pg::extension::ExtensionSchema;
use kronika_source_pg::statements::{StatementsCapability, StatementsVersion};
use kronika_source_pg::store_plans::{Flavour, StorePlansCapability};

use super::{
    BatchWrite, CachedSettings, DISCOVERY_INTERVAL, DatabaseCapabilities, GenerationProbe,
    PgObservation, PgSources, QueryFailure, QueryOutcome, SERVER_PROBE_SQL,
    cached_settings_for_generation, capability_sqlstate, discovery_due, measure,
    selected_statements, selected_statements_info, selected_store_plans, selected_store_plans_info,
    session_for_generation,
};

fn statements(version: StatementsVersion) -> StatementsCapability {
    StatementsCapability {
        version,
        schema: ExtensionSchema::new("monitoring"),
    }
}

fn plans(flavour: Flavour) -> StorePlansCapability {
    StorePlansCapability {
        flavour,
        schema: ExtensionSchema::new("monitoring"),
    }
}

fn capabilities(
    statements: Option<StatementsCapability>,
    plans: Option<StorePlansCapability>,
) -> DatabaseCapabilities {
    DatabaseCapabilities {
        statements,
        store_plans: plans,
        ..DatabaseCapabilities::default()
    }
}

#[test]
fn discovery_runs_immediately_then_every_five_minutes() {
    let now = Instant::now();
    assert!(discovery_due(None, now, false));
    assert!(!discovery_due(
        Some(now),
        now + DISCOVERY_INTERVAL.saturating_sub(Duration::from_millis(1)),
        false
    ));
    assert!(discovery_due(Some(now), now + DISCOVERY_INTERVAL, false));
}

#[test]
fn a_forced_tick_refreshes_discovery_before_the_deadline() {
    let now = Instant::now();
    assert!(discovery_due(Some(now), now, true));
}

#[test]
fn statements_choose_the_richest_layout_before_database_preference() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(Some(statements(StatementsVersion::V1)), None),
        ),
        (
            "beta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
        (
            "zeta".to_owned(),
            capabilities(Some(statements(StatementsVersion::V6)), None),
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities, Some("alpha")).map(|(database, _)| database),
        Some("beta".to_owned()),
        "an older current-database installation must not hide newer counters"
    );
    assert_eq!(
        selected_statements(&capabilities, Some("zeta")).map(|(database, _)| database),
        Some("zeta".to_owned()),
        "the current database wins between equal richest layouts"
    );
    assert_eq!(
        selected_statements(&capabilities, None).map(|(database, _)| database),
        Some("beta".to_owned()),
        "the database name is the deterministic final tie-break"
    );
}

#[test]
fn main_readers_and_info_views_are_selected_independently() {
    let schema = ExtensionSchema::new("monitoring");
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            DatabaseCapabilities {
                statements_info: Some(schema.clone()),
                store_plans_info: Some(schema),
                ..DatabaseCapabilities::default()
            },
        ),
        (
            "beta".to_owned(),
            capabilities(
                Some(statements(StatementsVersion::V1)),
                Some(plans(Flavour::OsscCompatible)),
            ),
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities, None).map(|(database, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_statements_info(&capabilities, None).map(|(database, _)| database),
        Some("alpha".to_owned())
    );
    assert_eq!(
        selected_store_plans(&capabilities, None).map(|(database, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans_info(&capabilities, None).map(|(database, _)| database),
        Some("alpha".to_owned())
    );
}

#[test]
fn store_plans_flavours_are_not_ranked() {
    let capabilities = BTreeMap::from([
        (
            "alpha".to_owned(),
            capabilities(None, Some(plans(Flavour::OsscCompatible))),
        ),
        (
            "beta".to_owned(),
            capabilities(None, Some(plans(Flavour::Datasentinel))),
        ),
        (
            "gamma".to_owned(),
            capabilities(None, Some(plans(Flavour::Vadv))),
        ),
        (
            "zeta".to_owned(),
            capabilities(None, Some(plans(Flavour::Vadv))),
        ),
    ]);

    assert_eq!(
        selected_store_plans(&capabilities, Some("alpha")).map(|(database, _)| database),
        Some("alpha".to_owned()),
        "the current database wins without ranking independent implementations"
    );
    assert_eq!(
        selected_store_plans(&capabilities, None).map(|(database, _)| database),
        Some("alpha".to_owned()),
        "the database name is the deterministic fallback"
    );
}

#[test]
fn capability_invalidation_preserves_independent_info_and_tracks_a_move() {
    let mut sources = PgSources::disabled();
    let schema = ExtensionSchema::new("monitoring");
    sources.capabilities.insert(
        "alpha".to_owned(),
        DatabaseCapabilities {
            statements: Some(statements(StatementsVersion::V1)),
            store_plans: Some(plans(Flavour::OsscCompatible)),
            statements_info: Some(schema.clone()),
            store_plans_info: Some(schema.clone()),
        },
    );
    sources.invalidate_statements("alpha");
    sources.invalidate_store_plans("alpha");
    assert!(selected_statements(&sources.capabilities, None).is_none());
    assert!(selected_store_plans(&sources.capabilities, None).is_none());
    assert!(selected_statements_info(&sources.capabilities, None).is_some());
    assert!(selected_store_plans_info(&sources.capabilities, None).is_some());

    sources.invalidate_statements_info("alpha");
    sources.invalidate_store_plans_info("alpha");
    sources.capabilities.insert(
        "beta".to_owned(),
        DatabaseCapabilities {
            statements_info: Some(schema.clone()),
            store_plans_info: Some(schema),
            ..DatabaseCapabilities::default()
        },
    );
    assert_eq!(
        selected_statements_info(&sources.capabilities, None).map(|(database, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans_info(&sources.capabilities, None).map(|(database, _)| database),
        Some("beta".to_owned())
    );
}

#[test]
fn settings_cache_is_scoped_to_one_connection_generation() {
    let cached = Some(CachedSettings {
        generation: 7,
        rows: Arc::from([]),
    });
    assert!(cached_settings_for_generation(cached.as_ref(), 7).is_some());
    assert!(cached_settings_for_generation(cached.as_ref(), 8).is_none());
}

#[test]
fn visibility_refresh_on_the_same_connection_keeps_generation_scoped_state() {
    let mut sources = PgSources::disabled();
    let now = Instant::now();
    sources.settings = Some(CachedSettings {
        generation: 7,
        rows: Arc::from([]),
    });
    sources
        .capabilities
        .insert("app".to_owned(), DatabaseCapabilities::default());
    sources.last_discovery = Some(now);
    sources.probe = Some(GenerationProbe {
        generation: 7,
        major: 18,
        user: "monitor".to_owned(),
        database: "app".to_owned(),
        full_visibility: false,
    });

    sources.update_probe_cache(
        GenerationProbe {
            generation: 7,
            major: 18,
            user: "monitor".to_owned(),
            database: "app".to_owned(),
            full_visibility: true,
        },
        true,
    );

    assert!(sources.settings.is_some());
    assert!(sources.capabilities.contains_key("app"));
    assert_eq!(sources.last_discovery, Some(now));
    assert!(
        sources
            .probe
            .as_ref()
            .is_some_and(|probe| probe.full_visibility)
    );
}

#[test]
fn a_new_primary_generation_discards_secondary_pools() {
    let mut sources = PgSources::disabled();
    sources.databases.insert(
        "other".to_owned(),
        Pool::new("host=127.0.0.1 dbname=other").expect("the DSN parses"),
    );

    sources.update_probe_cache(
        GenerationProbe {
            generation: 8,
            major: 18,
            user: "monitor".to_owned(),
            database: "app".to_owned(),
            full_visibility: true,
        },
        false,
    );

    assert!(sources.databases.is_empty());
}

#[tokio::test]
async fn losing_the_primary_during_extension_collection_ends_the_cycle() {
    let mut sources = PgSources::disabled();
    sources.server = Some(Pool::new("host=127.0.0.1 dbname=app").expect("the primary DSN parses"));
    sources.server_database = Some("app".to_owned());
    sources.databases.insert(
        "other".to_owned(),
        Pool::new("host=127.0.0.1 dbname=other").expect("the DSN parses"),
    );
    sources.discovered.push(Database {
        oid: 7,
        name: "other".to_owned(),
        is_current: false,
    });
    sources.capabilities.insert(
        "app".to_owned(),
        capabilities(Some(statements(StatementsVersion::V6)), None),
    );
    let probe = GenerationProbe {
        generation: 7,
        major: 18,
        user: "monitor".to_owned(),
        database: "app".to_owned(),
        full_visibility: true,
    };
    let mut admitted = false;
    let continued = sources
        .collect_extensions_dynamic(&probe, &mut |_observation| {}, None, &mut |_, _| {
            admitted = true;
            Ok::<_, ()>(BatchWrite::default())
        })
        .await
        .expect("no collector sink error");

    assert!(!continued);
    assert!(!admitted);
    assert!(sources.databases.is_empty());
    assert!(sources.discovered.is_empty());
    assert!(sources.capabilities.is_empty());
}

#[test]
fn server_and_extension_visibility_use_immediately_usable_role_privileges() {
    assert!(SERVER_PROBE_SQL.contains("pg_has_role('pg_read_all_stats', 'USAGE')"));
}

#[test]
fn runtime_capability_failures_schedule_rediscovery() {
    for code in ["42P01", "42883", "42704", "42703", "3F000", "42501"] {
        assert!(capability_sqlstate(code), "{code}");
    }
    assert!(!capability_sqlstate("22003"));
}

#[test]
fn timeout_observation_retains_partial_query_accounting() {
    let mut observations = Vec::new();
    let mut observe = |observation| observations.push(observation);
    let mut measured = measure(&mut observe, "probe", "monitor@db.example:5432", "postgres");
    measured.stats_mut().rows = 9;
    measured.timeout();

    let PgObservation::Query(observation) = observations.pop().expect("one observation") else {
        panic!("expected a query observation");
    };
    assert_eq!(observation.outcome, QueryOutcome::Timeout);
    assert_eq!(observation.stats.rows, 9);
}

#[test]
fn unavailable_session_is_accounted_as_a_closed_connection() {
    let mut pool = Pool::new("host=127.0.0.1 dbname=metrics")
        .expect("syntactically valid connection settings");
    let mut observations = Vec::new();
    {
        let mut observe = |observation| observations.push(observation);
        assert!(matches!(
            session_for_generation(&mut pool, 1, &mut observe),
            Err(QueryFailure::Connection)
        ));
    }

    let PgObservation::Connection(observation) =
        observations.pop().expect("one connection observation")
    else {
        panic!("expected a connection observation");
    };
    assert_eq!(observation.database, "metrics");
    assert!(observation.closed);
    assert!(!observation.timeout);
}
