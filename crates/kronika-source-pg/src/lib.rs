//! Collecting metrics from a `PostgreSQL` server.
#![allow(
    clippy::multiple_crate_versions,
    reason = "tokio-postgres pulls duplicate transitive versions outside this crate"
)]

pub mod activity;
pub mod archiver;
pub mod bgwriter;
pub mod checkpointer;
pub mod database;
pub mod databases;
pub mod extension;
pub mod io;
pub mod locks;
mod pool;
pub mod prepared_xacts;
pub mod progress_vacuum;
pub mod query;
pub mod settings;
pub mod statements;
pub mod statements_info;
pub mod store_plans;
pub mod store_plans_info;
pub mod user_indexes;
pub mod user_tables;
pub mod wal;

pub use pool::{CONNECT_TIMEOUT, ConnectError, Pool};
pub use query::Session;
