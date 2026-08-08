use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use kronika_source_pg::extension::ExtensionSchema;
use kronika_source_pg::statements::{StatementsCapability, StatementsVersion};
use kronika_source_pg::store_plans::{Flavour, StorePlansCapability};

use super::{
    DISCOVERY_INTERVAL, DatabaseCapabilities, PgSources, discovery_due, selected_statements,
    selected_store_plans,
};

fn statements() -> StatementsCapability {
    StatementsCapability {
        version: StatementsVersion::V1,
        schema: ExtensionSchema::new("monitoring"),
        full_visibility: true,
    }
}

fn plans() -> StorePlansCapability {
    StorePlansCapability {
        flavour: Flavour::OsscCompatible,
        schema: ExtensionSchema::new("monitoring"),
        full_visibility: true,
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
fn instance_global_extensions_choose_one_database_deterministically() {
    let capabilities = BTreeMap::from([
        (
            "zeta".to_owned(),
            DatabaseCapabilities {
                statements: Some(statements()),
                store_plans: Some(plans()),
            },
        ),
        (
            "alpha".to_owned(),
            DatabaseCapabilities {
                statements: Some(statements()),
                store_plans: Some(plans()),
            },
        ),
    ]);

    assert_eq!(
        selected_statements(&capabilities).map(|(database, _)| database),
        Some("alpha".to_owned())
    );
    assert_eq!(
        selected_store_plans(&capabilities).map(|(database, _)| database),
        Some("alpha".to_owned())
    );
}

#[test]
fn a_failed_capability_is_not_selected_again_before_discovery() {
    let mut sources = PgSources::disabled();
    sources.capabilities.insert(
        "alpha".to_owned(),
        DatabaseCapabilities {
            statements: Some(statements()),
            store_plans: Some(plans()),
        },
    );
    sources.capabilities.insert(
        "beta".to_owned(),
        DatabaseCapabilities {
            statements: Some(statements()),
            store_plans: Some(plans()),
        },
    );

    sources.invalidate_statements("alpha");
    sources.invalidate_store_plans("alpha");

    assert_eq!(
        selected_statements(&sources.capabilities).map(|(database, _)| database),
        Some("beta".to_owned())
    );
    assert_eq!(
        selected_store_plans(&sources.capabilities).map(|(database, _)| database),
        Some("beta".to_owned())
    );
}
