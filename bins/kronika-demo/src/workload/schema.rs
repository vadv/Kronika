//! Creates the workload's schemas and tables.
//!
//! Table shapes rotate through a fixed set so the workload's queries and
//! rows are not one shape repeated thousands of times: `pg_stat_statements`
//! and the system tables view get something genuinely varied to show.

use super::{WorkloadConfig, connect, naming};
use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;

/// One work item: a schema-qualified table to create.
#[derive(Debug, Clone, Copy)]
struct Table {
    schema: u32,
    index_in_schema: u32,
}

/// The `CREATE TABLE` column list for one of the rotating shapes, keyed by
/// `table.index_in_schema % SHAPES.len()`.
const SHAPES: [&str; 4] = [
    "(id bigint primary key, customer text not null, total_cents bigint not null, \
     placed_at timestamptz not null default now())",
    "(id bigint primary key, occurred_at timestamptz not null default now(), \
     kind text not null, payload text)",
    "(id bigint primary key, profile jsonb not null default '{}'::jsonb, \
     updated_at timestamptz not null default now())",
    "(id bigint primary key, a numeric, b numeric, c numeric, d numeric, \
     e numeric, f numeric)",
];

/// Log a setup progress line at most this often, in tables created.
const PROGRESS_STEP: usize = 500;

/// The table's name and its `CREATE TABLE ... (...)` statement.
fn table_ddl(table: Table) -> (String, String) {
    let name = naming::table_name(table.schema, table.index_in_schema);
    let shape = SHAPES[table.index_in_schema as usize % SHAPES.len()];
    (
        name.clone(),
        format!("create table if not exists {name} {shape}"),
    )
}

/// Create every configured schema and table, fanning DDL out over
/// `config.ddl_concurrency` independent connections.
///
/// A single failed statement is logged and does not stop the rest: this is a
/// demo aid, not the product's durability contract.
///
/// # Errors
///
/// Returns an error when the schema-setup connection cannot be opened.
pub(crate) async fn create_all(config: &WorkloadConfig) -> Result<()> {
    let setup = connect(&config.dsn)
        .await
        .context("open the schema-setup connection")?;
    for schema in 0..config.schemas {
        let name = naming::schema_name(schema);
        setup
            .execute(&format!("create schema if not exists {name}"), &[])
            .await
            .with_context(|| format!("create schema {name}"))?;
    }
    drop(setup);

    let mut work = Vec::new();
    for schema in 0..config.schemas {
        for index_in_schema in 0..config.tables_per_schema {
            work.push(Table {
                schema,
                index_in_schema,
            });
        }
    }
    let total = work.len();
    let concurrency = config.ddl_concurrency.max(1) as usize;
    let chunk_size = total.div_ceil(concurrency).max(1);
    let progress = Arc::new(AtomicUsize::new(0));
    let mut tasks = JoinSet::new();
    for chunk in work.chunks(chunk_size).map(<[Table]>::to_vec) {
        let dsn = config.dsn.clone();
        let progress = Arc::clone(&progress);
        tasks.spawn(async move { create_chunk(&dsn, &chunk, &progress, total).await });
    }
    while let Some(outcome) = tasks.join_next().await {
        match outcome {
            Ok(Ok(())) => {}
            Ok(Err(error)) => eprintln!("kronika-demo: workload DDL chunk failed: {error:#}"),
            Err(error) => eprintln!("kronika-demo: workload DDL task panicked: {error}"),
        }
    }
    println!(
        "kronika-demo: workload schema setup created {total} tables across {} schemas",
        config.schemas
    );
    Ok(())
}

async fn create_chunk(
    dsn: &str,
    chunk: &[Table],
    progress: &AtomicUsize,
    total: usize,
) -> Result<()> {
    let client = connect(dsn).await.context("open a DDL connection")?;
    for table in chunk {
        let (name, ddl) = table_ddl(*table);
        if let Err(error) = client.execute(&ddl, &[]).await {
            eprintln!("kronika-demo: create table {name} failed: {error}");
            continue;
        }
        let done = progress.fetch_add(1, Ordering::Relaxed) + 1;
        if done.is_multiple_of(PROGRESS_STEP) || done == total {
            println!("kronika-demo: workload schema setup {done}/{total} tables");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests;
