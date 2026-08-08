//! Collecting metrics from a `PostgreSQL` server.
#![allow(
    clippy::multiple_crate_versions,
    reason = "tokio-postgres pulls duplicate transitive versions outside this crate"
)]

mod pool;

pub use pool::{MAX_AGE, Pool};
