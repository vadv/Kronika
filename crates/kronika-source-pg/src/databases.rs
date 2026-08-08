//! The databases a per-database section has to visit.
//!
//! `pg_stat_user_tables` and its neighbours only ever report the database the
//! connection is attached to, so those sections need one connection per
//! database. The list is asked for on every tick: a database created between
//! ticks starts being collected, and one that was dropped stops.

use std::collections::BTreeMap;

use anyhow::{Context as _, Result};
use tokio_postgres::Client;

use crate::Pool;

/// Prefix a query literal with the kronika marker (SQL-transparency rule).
macro_rules! marked {
    ($sql:literal) => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " crates/kronika-source-pg/src/databases.rs */ ",
            $sql,
        )
    };
}

/// A database this collector can connect to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Database {
    /// Database oid, the `datid` every per-database row carries.
    pub oid: u32,
    /// Database name, which is also the key of its connection.
    pub name: String,
}

/// Every database that accepts connections, ordered by name.
///
/// Templates are left out: connecting to one blocks `CREATE DATABASE`, and a
/// template holds no workload worth recording.
///
/// # Errors
///
/// Returns the error of the query.
pub async fn enumerate(client: &Client) -> Result<Vec<Database>> {
    let rows = client
        .query(
            marked!(
                "SELECT oid, datname::text AS datname \
                 FROM pg_database \
                 WHERE datallowconn AND NOT datistemplate \
                 ORDER BY datname"
            ),
            &[],
        )
        .await
        .context("list the databases to collect")?;
    Ok(rows
        .iter()
        .map(|row| Database {
            oid: row.get("oid"),
            name: row.get("datname"),
        })
        .collect())
}

/// Bring `pools` in line with `found`, using `primary` as the connection
/// template.
///
/// A database that appeared gets a pool; one that disappeared has its pool
/// dropped, which closes the connection. Pools that stay keep their age, so a
/// tick does not restart every connection.
pub fn refresh(pools: &mut BTreeMap<String, Pool>, found: &[Database], primary: &Pool) {
    pools.retain(|name, _pool| found.iter().any(|database| &database.name == name));
    for database in found {
        pools
            .entry(database.name.clone())
            .or_insert_with(|| primary.on_database(&database.name));
    }
}

#[cfg(test)]
mod tests;
