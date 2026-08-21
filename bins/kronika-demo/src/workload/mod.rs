//! Optional `PostgreSQL` workload the demo drives alongside the collector.
//!
//! Disabled unless `KRONIKA_DEMO_WORKLOAD_DSN` is set, which leaves
//! `kronika-demo` exactly as it behaves without this feature. Every other
//! `KRONIKA_DEMO_WORKLOAD_*` variable has a default sized for a demo
//! container: thousands of tables across several schemas, a steady mix of
//! reads and writes, and real lock-wait chains, so the dashboards a fresh
//! `docker run` produces are already populated.

mod dml;
mod locks;
mod naming;
mod schema;

use anyhow::{Context, Result};
use std::fmt;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::runtime::{Builder, Runtime};
use tokio_postgres::{Client, NoTls};

/// Validated workload configuration.
#[derive(Clone)]
pub(crate) struct WorkloadConfig {
    /// Where the workload connects, normally through `PgBouncer`.
    pub(crate) dsn: String,
    /// How many schemas to create.
    pub(crate) schemas: u32,
    /// How many tables to create in each schema.
    pub(crate) tables_per_schema: u32,
    /// How many connections run `CREATE TABLE` concurrently during setup.
    pub(crate) ddl_concurrency: u32,
    /// How many long-lived sessions run steady-state DML.
    pub(crate) sessions: u32,
    /// How many independent lock chains run in each round.
    pub(crate) lock_chains: u32,
    /// How many transactions make up one lock chain.
    pub(crate) lock_chain_depth: u32,
    /// How long each link in a chain holds its lock, milliseconds.
    pub(crate) lock_hold_ms: u64,
    /// Pause between lock-chain rounds, seconds.
    pub(crate) lock_round_interval_s: u64,
}

impl fmt::Debug for WorkloadConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("WorkloadConfig")
            .field("dsn", &"[redacted]")
            .field("schemas", &self.schemas)
            .field("tables_per_schema", &self.tables_per_schema)
            .field("ddl_concurrency", &self.ddl_concurrency)
            .field("sessions", &self.sessions)
            .field("lock_chains", &self.lock_chains)
            .field("lock_chain_depth", &self.lock_chain_depth)
            .field("lock_hold_ms", &self.lock_hold_ms)
            .field("lock_round_interval_s", &self.lock_round_interval_s)
            .finish()
    }
}

fn env_u32(key: &str, default: u32) -> Result<u32> {
    match std::env::var(key) {
        Ok(raw) => raw
            .trim()
            .parse()
            .with_context(|| format!("{key}={raw:?} is not a u32")),
        Err(_unset) => Ok(default),
    }
}

fn env_u64(key: &str, default: u64) -> Result<u64> {
    match std::env::var(key) {
        Ok(raw) => raw
            .trim()
            .parse()
            .with_context(|| format!("{key}={raw:?} is not a u64")),
        Err(_unset) => Ok(default),
    }
}

impl WorkloadConfig {
    /// Read the workload configuration from the environment.
    ///
    /// Returns `Ok(None)` when `KRONIKA_DEMO_WORKLOAD_DSN` is unset.
    ///
    /// # Errors
    ///
    /// Returns an error when a set variable does not parse.
    pub(crate) fn from_env() -> Result<Option<Self>> {
        let Ok(dsn) = std::env::var("KRONIKA_DEMO_WORKLOAD_DSN") else {
            return Ok(None);
        };
        let config = Self {
            dsn,
            schemas: env_u32("KRONIKA_DEMO_WORKLOAD_SCHEMAS", 4)?,
            tables_per_schema: env_u32("KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA", 40)?,
            ddl_concurrency: env_u32("KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY", 4)?,
            sessions: env_u32("KRONIKA_DEMO_WORKLOAD_SESSIONS", 4)?,
            lock_chains: env_u32("KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS", 2)?,
            lock_chain_depth: env_u32("KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH", 3)?,
            lock_hold_ms: env_u64("KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS", 1_500)?,
            lock_round_interval_s: env_u64("KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S", 30)?,
        };
        config.validate()?;
        Ok(Some(config))
    }

    fn validate(&self) -> Result<()> {
        for (key, value) in [
            ("KRONIKA_DEMO_WORKLOAD_SCHEMAS", self.schemas),
            (
                "KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA",
                self.tables_per_schema,
            ),
            (
                "KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY",
                self.ddl_concurrency,
            ),
            ("KRONIKA_DEMO_WORKLOAD_SESSIONS", self.sessions),
            ("KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS", self.lock_chains),
            (
                "KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH",
                self.lock_chain_depth,
            ),
        ] {
            anyhow::ensure!(value > 0, "{key} must be greater than zero");
        }
        anyhow::ensure!(
            self.lock_hold_ms > 0,
            "KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS must be greater than zero"
        );
        anyhow::ensure!(
            self.lock_round_interval_s > 0,
            "KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S must be greater than zero"
        );
        Ok(())
    }
}

/// Connect to `dsn` and drive its connection on a background task.
///
/// # Errors
///
/// Returns an error when the connection cannot be established.
pub(crate) async fn connect(dsn: &str) -> Result<Client> {
    let (client, connection) = tokio_postgres::connect(dsn, NoTls)
        .await
        .context("connect to the demo PostgreSQL workload")?;
    tokio::spawn(async move {
        if let Err(error) = connection.await {
            eprintln!("kronika-demo: workload connection ended: {error}");
        }
    });
    Ok(client)
}

/// A running workload: the Tokio runtime driving it and the flag that stops
/// it.
pub(crate) struct Workload {
    runtime: Runtime,
    stop: Arc<AtomicBool>,
}

/// How long `Workload::stop` waits for tasks to notice the stop flag and
/// return before dropping whatever is still running.
const SHUTDOWN_GRACE: Duration = Duration::from_secs(5);

impl Workload {
    /// Start the workload on its own multi-thread runtime.
    ///
    /// # Errors
    ///
    /// Returns an error when the runtime cannot be created.
    pub(crate) fn start(config: WorkloadConfig) -> Result<Self> {
        let runtime = Builder::new_multi_thread()
            .enable_all()
            .thread_name("kronika-demo-workload")
            .build()
            .context("build the workload runtime")?;
        let stop = Arc::new(AtomicBool::new(false));
        let task_stop = Arc::clone(&stop);
        runtime.spawn(async move {
            if let Err(error) = run(config, task_stop).await {
                eprintln!("kronika-demo: workload stopped early: {error:#}");
            }
        });
        Ok(Self { runtime, stop })
    }

    /// Signal every task to finish its current operation and stop, then wait
    /// up to `SHUTDOWN_GRACE` before dropping anything still running.
    pub(crate) fn stop(self) {
        self.stop.store(true, Ordering::SeqCst);
        self.runtime.shutdown_timeout(SHUTDOWN_GRACE);
    }
}

async fn run(config: WorkloadConfig, stop: Arc<AtomicBool>) -> Result<()> {
    schema::create_all(&config).await?;

    let mut tasks = Vec::new();
    for session in 0..config.sessions {
        let config = config.clone();
        let stop = Arc::clone(&stop);
        tasks.push(tokio::spawn(async move {
            dml::run_session(session, &config, &stop).await;
        }));
    }
    tasks.push(tokio::spawn({
        let config = config.clone();
        let stop = Arc::clone(&stop);
        async move { locks::run_rounds(&config, &stop).await }
    }));
    for task in tasks {
        let _joined = task.await;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
