//! Collecting metrics from a `PostgreSQL` server.
#![allow(
    clippy::multiple_crate_versions,
    reason = "tokio-postgres pulls duplicate transitive versions outside this crate"
)]

mod pool;
mod settings;

pub use pool::{MAX_AGE, Pool};
pub use settings::{SettingsRow, collect as collect_settings, to_section as settings_to_section};
