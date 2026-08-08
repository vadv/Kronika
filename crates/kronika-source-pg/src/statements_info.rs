//! `pg_stat_statements_info` collection for type `1_015_001`.

use kronika_registry::Ts;
use kronika_registry::pg_stat_statements_info::PgStatStatementsInfo;
use tokio_postgres::Client;

use crate::extension::ExtensionVersion;

const MARKER: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " crates/kronika-source-pg/src/statements_info.rs */ "
);

/// Whether the extension exposes `pg_stat_statements_info`.
#[must_use]
pub const fn supported(version: ExtensionVersion) -> bool {
    version.major == 1 && version.minor >= 9
}

/// Build the one-row query for a safely qualified view supplied by discovery.
#[must_use]
pub fn query(qualified_view: &str) -> String {
    format!(
        "{MARKER}SELECT dealloc, \
         (extract(epoch from stats_reset) * 1e6)::int8 AS stats_reset_us, \
         (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us \
         FROM {qualified_view}"
    )
}

/// Collect the singleton module-level statistics row with an unnamed statement.
///
/// # Errors
///
/// Returns `PostgreSQL` protocol or row-decoding errors.
pub async fn collect(
    client: &Client,
    qualified_view: &str,
) -> Result<PgStatStatementsInfo, tokio_postgres::Error> {
    let row = client.query_typed_one(&query(qualified_view), &[]).await?;
    Ok(PgStatStatementsInfo {
        ts: Ts(row.try_get("ts_us")?),
        dealloc: row.try_get("dealloc")?,
        stats_reset: Ts(row.try_get("stats_reset_us")?),
    })
}

#[cfg(test)]
mod tests;
