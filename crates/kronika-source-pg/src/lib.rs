//! Collecting metrics from a `PostgreSQL` server.
#![allow(
    clippy::multiple_crate_versions,
    reason = "tokio-postgres pulls duplicate transitive versions outside this crate"
)]

pub mod activity;
pub mod archiver;
pub mod database;
pub mod databases;
pub mod io;
mod pool;
pub mod prepared_xacts;
pub mod progress_vacuum;
pub mod settings;
pub mod user_indexes;
pub mod wal;

pub use pool::{MAX_AGE, Pool};
