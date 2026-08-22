//! Steady-state DML sessions: a bounded mix of ordinary reads and writes.
//! Slow and failing showcase statements run separately in `events`, so the
//! baseline stays useful between anomaly episodes.

use super::{WorkloadConfig, connect_as, naming};
use rand::rngs::StdRng;
use rand::{Rng as _, SeedableRng as _};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_postgres::Client;

/// What one iteration of a session does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Action {
    /// Insert one row.
    Insert,
    /// Update rows in place.
    Update,
    /// Read a bounded page of rows.
    Select,
    /// A delete that matches nothing, exercising the statement without
    /// shrinking the table.
    Delete,
    /// A query that deliberately crosses the 5s known-bad boundary.
    SlowQuery,
    /// Deliberately invalid SQL, exercising the syntax-error category.
    BadStatement,
    /// A connection attempt to a database that does not exist, exercising
    /// `PgBouncer`'s "no such database" event.
    BadDatabase,
}

/// Maps a `0..100` roll to the next ordinary DML action.
pub(crate) const fn next_action(roll: u32) -> Action {
    match roll % 100 {
        0..=29 => Action::Insert,
        30..=54 => Action::Update,
        55..=89 => Action::Select,
        _ => Action::Delete,
    }
}

const JITTER_MS: [u64; 3] = [200, 500, 900];
const WORKLOAD_SEED: u64 = 0x4b52_4f4e_494b_4100;
const SESSION_APPLICATIONS: [&str; 4] = [
    "checkout-api",
    "catalog-api",
    "payments-worker",
    "fulfillment-worker",
];

pub(crate) const fn session_application_name(session: u32) -> &'static str {
    SESSION_APPLICATIONS[session as usize % SESSION_APPLICATIONS.len()]
}

fn session_rng(session: u32) -> StdRng {
    StdRng::seed_from_u64(WORKLOAD_SEED ^ u64::from(session))
}

/// Run one session until `stop` is set.
pub(crate) async fn run_session(session: u32, config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let Ok(client) = connect_as(&config.dsn, session_application_name(session)).await else {
        eprintln!("kronika-demo: workload session {session} could not connect");
        return;
    };
    let mut rng = session_rng(session);
    while !stop.load(Ordering::Relaxed) {
        let table = naming::table_name(
            rng.gen_range(0..config.schemas),
            rng.gen_range(0..config.tables_per_schema),
        );
        let action = next_action(rng.gen_range(0..100));
        let pause_ms = JITTER_MS[rng.gen_range(0..JITTER_MS.len())];
        let id = rng.gen_range(0..i64::MAX);
        if let Err(error) = perform(&client, config, &table, action, id).await {
            eprintln!("kronika-demo: session {session} {action:?} on {table} failed: {error:#}");
        }
        tokio::time::sleep(Duration::from_millis(pause_ms)).await;
    }
}

// Every statement here runs through `batch_execute` with values inlined as
// literals, never `execute`/`query` with bound parameters. The workload
// connects through PgBouncer in transaction-pooling mode, and a bound
// parameter would need `execute`'s implicit prepared statement, which does
// not survive a pooled connection switching backend between calls. Inlining
// is safe: every value here is a number this module generated, never
// external input.
pub(super) async fn perform(
    client: &Client,
    config: &WorkloadConfig,
    table: &str,
    action: Action,
    id: i64,
) -> anyhow::Result<()> {
    if let Some(sql) = ordinary_sql(table, action, id) {
        client.batch_execute(&sql).await?;
        return Ok(());
    }
    match action {
        Action::Insert | Action::Update | Action::Select | Action::Delete => unreachable!(),
        Action::SlowQuery => {
            client.batch_execute("select pg_sleep(6)").await?;
        }
        Action::BadStatement => {
            // The server's rejection is the point, not a workload failure.
            drop(client.batch_execute("slect 1").await);
        }
        Action::BadDatabase => {
            drop(connect_as(&format!("{} dbname=nope", config.dsn), "misconfigured-api").await);
        }
    }
    Ok(())
}

pub(crate) fn ordinary_sql(table: &str, action: Action, id: i64) -> Option<String> {
    match action {
        Action::Insert => Some(format!(
            "insert into {table} (id) values ({id}) on conflict do nothing"
        )),
        Action::Update => Some(format!("update {table} set id = id where id = {id}")),
        Action::Select => Some(format!("select * from {table} limit 50")),
        Action::Delete => Some(format!("delete from {table} where false")),
        Action::SlowQuery | Action::BadStatement | Action::BadDatabase => None,
    }
}

#[cfg(test)]
mod tests;
