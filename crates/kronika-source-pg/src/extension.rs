//! Which extensions a database has, and at which version.
//!
//! `pg_stat_statements` and `pg_store_plans` keep instance-wide counters, but
//! their views only exist in the databases where the extension was created. The
//! installed version also selects the column set, which follows the extension
//! release rather than the server major.

use anyhow::{Context as _, Result};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

/// Prefix a query literal with the kronika marker (SQL-transparency rule).
macro_rules! marked {
    ($sql:literal) => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " crates/kronika-source-pg/src/extension.rs */ ",
            $sql,
        )
    };
}

/// An installed extension version, as `pg_extension.extversion` spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct ExtensionVersion {
    /// Major component, before the first dot.
    pub major: u32,
    /// Minor component; `0` when the version is a bare major.
    pub minor: u32,
}

/// Read `major.minor` from an `extversion` string.
///
/// Anything after the second component is ignored: no extension this collector
/// reads changes its columns in a patch release, and a suffix like `1.10-beta`
/// still names the 1.10 column set.
#[must_use]
pub fn parse_version(text: &str) -> Option<ExtensionVersion> {
    let mut parts = text.split('.');
    let major = digits(parts.next()?)?;
    let minor = parts.next().map_or(Some(0), digits)?;
    Some(ExtensionVersion { major, minor })
}

/// The leading run of digits as a number, or `None` when there is none.
fn digits(text: &str) -> Option<u32> {
    let leading: String = text.chars().take_while(char::is_ascii_digit).collect();
    leading.parse().ok()
}

/// The version of `name` in the connected database, or `None` when the
/// extension is not installed there.
///
/// # Errors
///
/// Returns the error of the query.
pub async fn installed(
    session: Session<'_>,
    name: &str,
    stats: &mut QueryStats,
) -> Result<Option<ExtensionVersion>> {
    let row = query::read_optional(
        session,
        marked!("SELECT extversion FROM pg_extension WHERE extname = $1"),
        std::iter::once((name.to_owned(), Type::TEXT)),
        name.len(),
        stats,
        |row| row.get::<_, String>("extversion"),
    )
    .await
    .with_context(|| format!("look for the {name} extension"))?;
    Ok(row.as_deref().and_then(parse_version))
}

#[cfg(test)]
mod tests;
