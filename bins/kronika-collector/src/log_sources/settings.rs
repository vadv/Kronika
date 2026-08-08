//! Reading one server setting the `stderr` parser needs.
//!
//! `log_line_prefix` decides what a `stderr` line carries in front of its
//! severity. It is the server's own setting, so it is read from the server
//! rather than declared a second time in the collector's environment.

use anyhow::{Context as _, Result};
use tokio_postgres::NoTls;

/// Ask the server how it lays out a `stderr` line.
///
/// # Errors
///
/// Returns an error when the connection cannot be made or the query fails.
pub(super) async fn log_line_prefix(dsn: &str) -> Result<String> {
    let (client, connection) = tokio_postgres::connect(dsn, NoTls)
        .await
        .context("connect to PostgreSQL")?;
    // The connection drives the protocol and ends when the client is dropped.
    let driver = tokio::spawn(connection);
    let row = client
        .query_one(
            "SELECT setting FROM pg_settings WHERE name = 'log_line_prefix'",
            &[],
        )
        .await
        .context("read log_line_prefix from pg_settings");
    drop(client);
    driver.abort();
    Ok(row?.get(0))
}
