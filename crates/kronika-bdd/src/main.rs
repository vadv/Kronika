//! BDD runner for the collector.
//!
//! Every scenario spawns the real binary over a temporary data root and reads
//! back the artifacts an operator sees: the segments on disk, decoded through
//! `kronika-reader` by the same path web uses, and the log lines. Nothing is
//! mocked.
//!
//! The runner is meant to be executed inside the cached BDD image, where
//! `KRONIKA_COLLECTOR_BIN` points at the compiled collector and
//! `KRONIKA_FEATURES`/`KRONIKA_FIXTURES` point at this crate's data. From a
//! checkout it falls back to the paths beside itself.
#![allow(
    clippy::trivial_regex,
    reason = "cucumber step phrases are literal English, matched as plain text, not real regexes"
)]
#![allow(
    clippy::multiple_crate_versions,
    reason = "cucumber's dependency tree pulls duplicate transitive versions outside our control"
)]
#![allow(
    clippy::needless_pass_by_ref_mut,
    reason = "cucumber passes &mut World to every step by contract, even read-only ones"
)]
#![allow(
    clippy::needless_pass_by_value,
    reason = "cucumber builds a step's captured parameters by value; a borrow does not bind"
)]

mod collector;
mod services;
mod steps;

use collector::Run;
use cucumber::World as _;

/// One scenario's state: the settings it declared, the fixture and data root
/// that back them, and the run under test.
#[derive(Debug, Default, cucumber::World)]
struct BddWorld {
    env: Vec<(String, String)>,
    fixture: Option<tempfile::TempDir>,
    prepared_root: Option<tempfile::TempDir>,
    run: Option<Run>,
    postgres: Option<services::Postgres>,
    pgbouncer: Option<services::PgBouncer>,
}

#[tokio::main]
async fn main() {
    let features = std::env::var("KRONIKA_FEATURES").unwrap_or_else(|_unset| "features".to_owned());
    // One at a time: the log scenarios start a real PostgreSQL and a real
    // PgBouncer on fixed ports and data directories, and two of those at once
    // are two scenarios wiping each other's server.
    BddWorld::cucumber()
        .max_concurrent_scenarios(1)
        .fail_on_skipped()
        .run_and_exit(features)
        .await;
}
