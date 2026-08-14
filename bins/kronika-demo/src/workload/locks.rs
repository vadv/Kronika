//! Lock-wait chains: several independent groups of transactions that
//! deliberately serialize on the same row, so `pg_locks`, blocked sessions in
//! `pg_stat_activity`, and `pg_log_lock_waits` events are real, not
//! simulated.
//!
//! Each chain targets its own row. Every link in a chain issues the exact
//! same `UPDATE ... WHERE id = <chain key>` inside its own transaction and
//! holds it for a fixed duration before committing. `PostgreSQL`'s row-lock
//! queue is FIFO, so the second link genuinely waits for the first, the
//! third waits for the second, and so on — no manual coordination between
//! the links is needed.

use super::{WorkloadConfig, connect, naming};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// Run lock-chain rounds until `stop` is set.
pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let table = naming::table_name(0, 0);
    if let Err(error) = ensure_keys(&config.dsn, &table, config.lock_chains).await {
        eprintln!("kronika-demo: could not seed lock-chain keys: {error:#}");
        return;
    }
    while !stop.load(Ordering::Relaxed) {
        run_one_round(config, &table).await;
        tokio::time::sleep(Duration::from_secs(config.lock_round_interval_s)).await;
    }
}

/// Insert one row per chain, so every chain's `UPDATE` has a row to lock.
async fn ensure_keys(dsn: &str, table: &str, chains: u32) -> anyhow::Result<()> {
    let client = connect(dsn).await?;
    for key in 0..chains {
        client
            .execute(
                &format!("insert into {table} (id) values ($1) on conflict do nothing"),
                &[&i64::from(key)],
            )
            .await?;
    }
    Ok(())
}

async fn run_one_round(config: &WorkloadConfig, table: &str) {
    let mut links = Vec::new();
    for chain in 0..config.lock_chains {
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

/// Lock row `key` in `table` for `hold`, inside its own transaction on its
/// own connection.
async fn hold_one_link(dsn: &str, table: &str, key: i64, hold: Duration) {
    let client = match connect(dsn).await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: lock-chain link could not connect: {error:#}");
            return;
        }
    };
    if let Err(error) = client.batch_execute("begin").await {
        eprintln!("kronika-demo: lock-chain link could not begin: {error}");
        return;
    }
    let locked = client
        .execute(
            &format!("update {table} set id = id where id = $1"),
            &[&key],
        )
        .await;
    if let Err(error) = locked {
        eprintln!("kronika-demo: lock-chain link could not lock row {key}: {error}");
        drop(client.batch_execute("rollback").await);
        return;
    }
    tokio::time::sleep(hold).await;
    if let Err(error) = client.batch_execute("commit").await {
        eprintln!("kronika-demo: lock-chain link could not commit: {error}");
    }
}
