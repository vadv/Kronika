//! BDD runner for the collector.
//!
//! Every scenario spawns the real binary over a temporary data root and reads
//! back the two artifacts an operator sees: the files on disk and the log
//! lines. Nothing is mocked.
//!
//! The runner is meant to be executed inside the cached BDD image, where
//! `KRONIKA_COLLECTOR_BIN` points at the compiled collector. From a checkout it
//! falls back to the binary next to itself.
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

mod collector;
mod steps;

use collector::Run;
use cucumber::World as _;

/// One scenario's state: the environment it built up, the fixture root that
/// backs it, and the run under test.
#[derive(Debug, Default, cucumber::World)]
struct BddWorld {
    env: Vec<(&'static str, String)>,
    fixture: Option<tempfile::TempDir>,
    run: Option<Run>,
}

#[tokio::main]
async fn main() {
    let features = std::env::var("KRONIKA_FEATURES").unwrap_or_else(|_| "features".to_owned());
    BddWorld::cucumber()
        .fail_on_skipped()
        .run_and_exit(features)
        .await;
}
