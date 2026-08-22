//! Lock-wait chains: several independent groups of transactions that
//! deliberately serialize on the same row, so `pg_locks`, blocked sessions in
//! `pg_stat_activity`, and `pg_log_lock_waits` events are real, not
//! simulated. Every chain runs in a bounded round, followed by a quiet
//! interval with no demo-owned lock wait.
//!
//! Each chain targets its own row. Every link in a chain issues the exact
//! same `UPDATE ... WHERE id = <chain key>` inside its own transaction and
//! holds it for a fixed duration before committing. `PostgreSQL`'s row-lock
//! queue is FIFO, so the second link genuinely waits for the first, the
//! third waits for the second, and so on — no manual coordination between
//! the links is needed.

use super::{WorkloadConfig, connect, naming, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[cfg(test)]
mod tests;

/// Run lock-chain rounds until `stop` is set.
pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let table = naming::table_name(0, 0);
    if let Err(error) = ensure_keys(&config.dsn, &table, config.lock_chains).await {
        eprintln!("kronika-demo: could not seed lock-chain keys: {error:#}");
        return;
    }

    while !stop.load(Ordering::Relaxed) {
        run_one_round(config, &table).await;
        wait_for_stop(stop, Duration::from_secs(config.lock_round_interval_s)).await;
    }
}

/// Insert one row per chain, so every chain's `UPDATE` has a row to lock.
///
/// Uses `batch_execute` with the key inlined, not a bound parameter: see the
/// note on `hold_one_link` about `PgBouncer` transaction pooling.
async fn ensure_keys(dsn: &str, table: &str, chains: u32) -> anyhow::Result<()> {
    let client = connect(dsn).await?;
    for key in 0..chains {
        client
            .batch_execute(&format!(
                "insert into {table} (id) values ({key}) on conflict do nothing"
            ))
            .await?;
    }
    Ok(())
}

async fn run_one_round(config: &WorkloadConfig, table: &str) {
    let mut links = Vec::new();
    for chain in periodic_chain_keys(config.lock_chains) {
        for _link in 0..config.lock_chain_depth {
            let dsn = config.dsn.clone();
            let table = table.to_owned();
            let hold = Duration::from_millis(config.lock_hold_ms);
            links.push(tokio::spawn(async move {
                hold_one_link(&dsn, &table, i64::from(chain), hold).await;
            }));
        }
    }
    for link in links {
        let _joined = link.await;
    }
}

const fn periodic_chain_keys(chains: u32) -> std::ops::Range<u32> {
    0..chains
}

/// Lock row `key` in `table` for `hold`, inside its own transaction on its
/// own connection.
///
/// `batch_execute` with `key` inlined, not `execute` with a bound parameter:
/// the connection runs through `PgBouncer` in transaction-pooling mode, and
/// the simple query protocol has no prepared statement that could outlive
/// the pooled backend this transaction happens to land on.
async fn hold_one_link(dsn: &str, table: &str, key: i64, hold: Duration) {
    let client = match connect(dsn).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: lock-chain link could not connect: {error:#}");
            return;
        }
    };
    if let Err(error) = client.batch_execute("begin").await {
        eprintln!("kronika-demo: lock-chain link could not begin: {error:?}");
        return;
    }
    let locked = client
        .batch_execute(&format!("update {table} set id = id where id = {key}"))
        .await;
    if let Err(error) = locked {
        eprintln!("kronika-demo: lock-chain link could not lock row {key}: {error:?}");
        drop(client.batch_execute("rollback").await);
        return;
    }
    tokio::time::sleep(hold).await;
    if let Err(error) = client.batch_execute("commit").await {
        eprintln!("kronika-demo: lock-chain link could not commit: {error:?}");
    }
}
