//! Steady-state DML sessions: a mix of ordinary reads and writes plus
//! deliberately slow and deliberately bad statements, so `pg_stat_statements`
//! and the log-derived findings have more than a happy path to show.

use super::{WorkloadConfig, connect, naming};
use rand::Rng as _;
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

/// Maps a `0..100` roll to the next action. Ordinary DML dominates; the
/// deliberately slow and deliberately bad actions are rare enough that most
/// of the timeline still looks unremarkable.
pub(crate) const fn next_action(roll: u32) -> Action {
    match roll % 100 {
        0..=29 => Action::Insert,
        30..=54 => Action::Update,
        55..=89 => Action::Select,
        90..=95 => Action::Delete,
        96 => Action::SlowQuery,
        97..=98 => Action::BadStatement,
        _ => Action::BadDatabase,
    }
}

const JITTER_MS: [u64; 3] = [200, 500, 900];

/// Run one session until `stop` is set.
pub(crate) async fn run_session(session: u32, config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let Ok(client) = connect(&config.dsn).await else {
        eprintln!("kronika-demo: workload session {session} could not connect");
        return;
    };
    while !stop.load(Ordering::Relaxed) {
        // `ThreadRng` is not `Send`, so it must not be held across an
        // `.await`: draw everything this iteration needs up front, in a
        // block that ends before the first await point.
        let (table, action, pause_ms) = {
            let mut rng = rand::thread_rng();
            let table = naming::table_name(
                rng.gen_range(0..config.schemas.max(1)),
                rng.gen_range(0..config.tables_per_schema.max(1)),
            );
            let action = next_action(rng.gen_range(0..100));
            let pause_ms = JITTER_MS[rng.gen_range(0..JITTER_MS.len())];
            (table, action, pause_ms)
        };
        if let Err(error) = perform(&client, config, &table, action).await {
            eprintln!("kronika-demo: session {session} {action:?} on {table} failed: {error:#}");
        }
        tokio::time::sleep(Duration::from_millis(pause_ms)).await;
    }
}

async fn perform(
    client: &Client,
    config: &WorkloadConfig,
    table: &str,
    action: Action,
) -> anyhow::Result<()> {
    match action {
        Action::Insert => {
            let id: i64 = rand::thread_rng().gen_range(0..i64::MAX);
            client
                .execute(
                    &format!("insert into {table} (id) values ($1) on conflict do nothing"),
                    &[&id],
                )
                .await?;
        }
        Action::Update => {
            client
                .execute(
                    &format!("update {table} set id = id where id is not null"),
                    &[],
                )
                .await?;
        }
        Action::Select => {
            client
                .query(&format!("select * from {table} limit 50"), &[])
                .await?;
        }
        Action::Delete => {
            client
                .execute(&format!("delete from {table} where false"), &[])
                .await?;
        }
        Action::SlowQuery => {
            client.execute("select pg_sleep(6)", &[]).await?;
        }
        Action::BadStatement => {
            // The server's rejection is the point, not a workload failure.
            drop(client.execute("slect 1", &[]).await);
        }
        Action::BadDatabase => {
            drop(connect(&format!("{} dbname=nope", config.dsn)).await);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
