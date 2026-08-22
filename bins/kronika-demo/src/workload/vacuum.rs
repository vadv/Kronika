//! Periodic table churn and throttled `VACUUM` episodes.

use super::{WorkloadConfig, connect_as, wait_for_stop};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio_postgres::Client;

const TABLE: &str = "shop.event_log";

fn setup_sql(rows: u32) -> String {
    format!(
        "create table if not exists {TABLE} (\
             id bigint primary key, occurred_at timestamptz not null default now(), \
             kind text not null default 'checkout', payload text\
         ); \
         alter table {TABLE} set (fillfactor = 80); \
         insert into {TABLE} (id, kind, payload, occurred_at) \
         select series, 'fulfillment', repeat(md5(series::text), 8), clock_timestamp() \
         from generate_series(1, {rows}) as series \
         on conflict (id) do nothing"
    )
}

fn run_sql(timeout_s: u64) -> Vec<String> {
    vec![
        format!(
            "set statement_timeout = '{timeout_s}s'; \
             set vacuum_cost_delay = '8ms'; \
             set vacuum_cost_limit = 200"
        ),
        format!(
            "update {TABLE} \
             set payload = reverse(coalesce(payload, '')), occurred_at = clock_timestamp()"
        ),
        format!("vacuum (analyze) {TABLE}"),
        "reset vacuum_cost_delay; reset vacuum_cost_limit; reset statement_timeout".to_owned(),
    ]
}

/// Run a real update-plus-vacuum episode immediately and then at a fixed,
/// quiet cadence. This uses a direct `PostgreSQL` connection because the
/// throttling settings are session-scoped and must not cross `PgBouncer`'s
/// transaction-pooling boundary.
pub(crate) async fn run_rounds(config: &WorkloadConfig, stop: &Arc<AtomicBool>) {
    let client = match connect_as(&config.vacuum_dsn, "vacuum-worker").await {
        Ok(client) => client,
        Err(error) => {
            eprintln!("kronika-demo: vacuum workload could not connect: {error:#}");
            return;
        }
    };
    if let Err(error) = client.batch_execute(&setup_sql(config.vacuum_rows)).await {
        eprintln!("kronika-demo: vacuum table setup failed: {error:#}");
        return;
    }

    wait_for_stop(stop, Duration::from_secs(95)).await;

    while !stop.load(Ordering::Relaxed) {
        println!("kronika-demo: vacuum story starting event-log maintenance");
        if let Err(error) = run_one_round(&client, config.vacuum_statement_timeout_s).await {
            eprintln!("kronika-demo: vacuum episode failed: {error:#}");
        }
        wait_for_stop(stop, Duration::from_secs(config.vacuum_round_interval_s)).await;
    }
}

async fn run_one_round(client: &Client, timeout_s: u64) -> anyhow::Result<()> {
    let statements = run_sql(timeout_s);
    client.batch_execute(&statements[0]).await?;
    let work = async {
        client.batch_execute(&statements[1]).await?;
        client.batch_execute(&statements[2]).await?;
        anyhow::Ok(())
    }
    .await;
    let reset = client.batch_execute(&statements[3]).await;
    work?;
    reset?;
    Ok(())
}

#[cfg(test)]
mod tests;
