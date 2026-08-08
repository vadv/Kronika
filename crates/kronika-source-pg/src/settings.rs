//! `pg_settings` for section `1_019_001`.
//!
//! One query returns every parameter of the running server, and the layout has
//! been stable since PG 10. Rows arrive ordered by `name`, which is the section
//! sort key, so interning in arrival order keeps that order meaningful.

use anyhow::{Context as _, Result};
use kronika_registry::{PgSettings, StrId, Ts};
use tokio_postgres::types::Type;

use crate::Session;
use crate::query::{self, QueryStats};

/// Prefix a query with a marker naming who is asking.
///
/// A DBA reading `pg_stat_activity` sees which collector and which file the
/// query came from, without having to guess.
macro_rules! marked {
    ($sql:literal) => {
        concat!(
            "/* kronika:",
            env!("CARGO_PKG_VERSION"),
            " crates/kronika-source-pg/src/settings.rs */ ",
            $sql,
        )
    };
}

/// One `pg_settings` row as the server sent it, before interning.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsRow {
    /// Collection time, unix microseconds.
    pub ts: i64,
    /// Parameter name.
    pub name: String,
    /// Running value, in `unit` units.
    pub setting: String,
    /// Unit of `setting`; `None` for unitless parameters.
    pub unit: Option<String>,
    /// How the running value was set.
    pub source: String,
    /// Config file that set the value.
    pub sourcefile: Option<String>,
    /// Line within `sourcefile`.
    pub sourceline: Option<i32>,
    /// The value changed but takes effect only after a restart.
    pub pending_restart: bool,
    /// Required context to change the value.
    pub context: String,
    /// Value type.
    pub vartype: String,
    /// Compiled-in default.
    pub boot_val: Option<String>,
    /// Value `RESET` would restore.
    pub reset_val: Option<String>,
}

/// Every `pg_settings` row, ordered by name.
///
/// # Errors
///
/// Returns the error of the query.
pub async fn collect(session: Session<'_>, stats: &mut QueryStats) -> Result<Vec<SettingsRow>> {
    query::read_all(
        session,
        marked!(
            "SELECT \
                 (extract(epoch from statement_timestamp()) * 1e6)::int8 AS ts_us, \
                 name, left(setting, 65536) AS setting, unit, source, \
                 left(sourcefile, 65536) AS sourcefile, sourceline, \
                 pending_restart, context, vartype, \
                 left(boot_val, 65536) AS boot_val, \
                 left(reset_val, 65536) AS reset_val \
             FROM pg_settings \
             ORDER BY name"
        ),
        std::iter::empty::<(String, Type)>(),
        0,
        stats,
        |row| SettingsRow {
            ts: row.get("ts_us"),
            name: row.get("name"),
            setting: row.get("setting"),
            unit: row.get("unit"),
            source: row.get("source"),
            sourcefile: row.get("sourcefile"),
            sourceline: row.get("sourceline"),
            pending_restart: row.get("pending_restart"),
            context: row.get("context"),
            vartype: row.get("vartype"),
            boot_val: row.get("boot_val"),
            reset_val: row.get("reset_val"),
        },
    )
    .await
    .context("read pg_settings")
}

/// Intern the row's strings and build the section row.
///
/// # Errors
///
/// Passes on whatever the interner returned.
pub fn to_section<E>(
    row: &SettingsRow,
    mut intern: impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<PgSettings, E> {
    Ok(PgSettings {
        ts: Ts(row.ts),
        name: intern(row.name.as_bytes())?,
        setting: intern(row.setting.as_bytes())?,
        unit: intern_opt(row.unit.as_deref(), &mut intern)?,
        source: intern(row.source.as_bytes())?,
        sourcefile: intern_opt(row.sourcefile.as_deref(), &mut intern)?,
        sourceline: row.sourceline,
        pending_restart: row.pending_restart,
        context: intern(row.context.as_bytes())?,
        vartype: intern(row.vartype.as_bytes())?,
        boot_val: intern_opt(row.boot_val.as_deref(), &mut intern)?,
        reset_val: intern_opt(row.reset_val.as_deref(), &mut intern)?,
    })
}

/// Intern an optional string, keeping absence as absence.
fn intern_opt<E>(
    value: Option<&str>,
    intern: &mut impl FnMut(&[u8]) -> Result<StrId, E>,
) -> Result<Option<StrId>, E> {
    value.map(|text| intern(text.as_bytes())).transpose()
}

#[cfg(test)]
mod tests;
