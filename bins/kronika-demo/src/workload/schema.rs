use super::{WorkloadConfig, connect_as, naming};
use anyhow::{Context, Result};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use tokio::task::JoinSet;

#[derive(Debug, Clone, Copy)]
struct Table {
    schema: u32,
    index_in_schema: u32,
}

// Every non-id column has a default; DML inserts only id across all shapes.
const SHAPES: [&str; 8] = [
    "(id bigint primary key, customer_id bigint not null default 0, \
     status text not null default 'paid', total_cents bigint not null default 0, \
     placed_at timestamptz not null default now())",
    "(id bigint primary key, email text not null default 'demo@example.test', \
     tier text not null default 'standard', created_at timestamptz not null default now())",
    "(id bigint primary key, order_id bigint not null default 0, product_id bigint not null default 0, \
     quantity integer not null default 1, unit_cents bigint not null default 0)",
    "(id bigint primary key, sku text not null default 'DEMO', name text not null default 'Demo product', \
     price_cents bigint not null default 0)",
    "(id bigint primary key, product_id bigint not null default 0, available integer not null default 0, \
     reserved integer not null default 0)",
    "(id bigint primary key, order_id bigint not null default 0, state text not null default 'captured', \
     amount_cents bigint not null default 0, paid_at timestamptz)",
    "(id bigint primary key, occurred_at timestamptz not null default now(), \
     kind text not null default 'checkout', payload text)",
    "(id bigint primary key, customer_id bigint not null default 0, \
     expires_at timestamptz not null default now() + interval '1 hour', \
     metadata jsonb not null default '{}'::jsonb)",
];

const PROGRESS_STEP: usize = 500;

fn table_ddl(table: Table) -> (String, String) {
    let name = naming::table_name(table.schema, table.index_in_schema);
    let shape = SHAPES[table.index_in_schema as usize % SHAPES.len()];
    (
        name.clone(),
        format!("create table if not exists {name} {shape}"),
    )
}

/// Creates schemas and tables concurrently.
/// Individual table failures are logged; setup errors are returned.
pub(crate) async fn create_all(config: &WorkloadConfig) -> Result<()> {
    let setup = connect_as(&config.dsn, "deploy-migration")
        .await
        .context("open the schema-setup connection")?;
    for schema in 0..config.schemas {
        let name = naming::schema_name(schema);
        setup
            .batch_execute(&format!("create schema if not exists {name}"))
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
        "kronika-demo: workload schema setup created {}/{total} tables across {} schemas",
        progress.load(Ordering::Relaxed),
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
    let client = connect_as(dsn, "schema-loader")
        .await
        .context("open a DDL connection")?;
    for table in chunk {
        let (name, ddl) = table_ddl(*table);
        // Use the simple-query protocol across PgBouncer transaction pooling.
        if let Err(error) = client.batch_execute(&ddl).await {
            eprintln!("kronika-demo: create table {name} failed: {error:?}");
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
