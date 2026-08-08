//! Type `2_100_001`: what a `PgBouncer` log carries.

use crate::{Section, StrId, Ts};

/// Type `2_100_001`: one row per recognized `PgBouncer` line.
///
/// There is no `kind` column. The message text is the category, identical texts
/// already cost one dictionary entry between them, and a taxonomy maintained by
/// hand goes stale against a moving upstream.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 2_100_001,
    name = "pgbouncer_events",
    semantics = event_stream,
    sort_key("ts", "level", "text")
)]
pub struct PgBouncerEvents {
    /// Line time, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// `0` fatal, `1` error, `2` warning, `3` log, `4` debug, `5` noise.
    #[column(l)]
    pub level: u8,
    /// The `pgbouncer.ini` section the connection used, which is not the
    /// `dbname` it resolves to. `NULL` on a line with no connection behind it.
    #[column(l)]
    pub database: Option<StrId>,
    /// The login user; `NULL` on a line with no connection behind it.
    #[column(l)]
    pub username: Option<StrId>,
    /// The client or server address. The port is not stored: it is the client's
    /// ephemeral port, so keeping it would add a dictionary entry per
    /// connection.
    #[column(l)]
    pub host: Option<StrId>,
    /// What happened, without the `closing because:` wrapper and without the
    /// connection's age.
    #[column(l)]
    pub text: StrId,
}

#[cfg(test)]
mod tests;
