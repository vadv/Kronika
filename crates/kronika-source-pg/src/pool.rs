//! One connection to a `PostgreSQL` server, replaced before it gets old.
//!
//! A connection is opened on first use and dropped once it reaches its age
//! limit, so a session that survived a failover or a pooler restart cannot go
//! on being used. The check happens when a collection tick asks for the
//! client; there is no background timer, because the collector already wakes
//! on a schedule and a timer would keep it awake between ticks.

use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use tokio_postgres::{Client, Config, NoTls};

/// How long a connection may live before it is replaced.
///
/// An hour is both the default and the ceiling: a longer session accumulates
/// plan cache and backend memory on a server that is not ours to spend.
pub const MAX_AGE: Duration = Duration::from_hours(1);

/// A connection that reopens itself once it is old enough.
#[derive(Debug)]
pub struct Pool {
    config: Config,
    max_age: Duration,
    open: Option<Open>,
}

#[derive(Debug)]
struct Open {
    client: Client,
    driver: tokio::task::JoinHandle<()>,
    opened: Instant,
}

impl Pool {
    /// A pool for `dsn`, replacing a connection after `max_age`.
    ///
    /// `max_age` above [`MAX_AGE`] is brought down to it: the ceiling is the
    /// point of the setting.
    ///
    /// # Errors
    ///
    /// Returns the parse error when `dsn` is neither a keyword string nor a
    /// connection URL.
    pub fn new(dsn: &str, max_age: Duration) -> Result<Self> {
        let config: Config = dsn.parse().context("parse the PostgreSQL DSN")?;
        Ok(Self::from_config(config, max_age))
    }

    /// A pool for an already parsed connection configuration.
    #[must_use]
    pub fn from_config(config: Config, max_age: Duration) -> Self {
        Self {
            config,
            max_age: max_age.min(MAX_AGE),
            open: None,
        }
    }

    /// The same server and credentials, connecting to `dbname` instead.
    ///
    /// This is how a per-database section reaches every database: the operator
    /// configures one DSN and the collector varies only the database name.
    #[must_use]
    pub fn on_database(&self, dbname: &str) -> Self {
        let mut config = self.config.clone();
        config.dbname(dbname);
        Self::from_config(config, self.max_age)
    }

    /// The connection to use now, opening or replacing it as needed.
    ///
    /// # Errors
    ///
    /// Returns the connection error. The pool keeps no failed connection, so
    /// the next call tries again from the start.
    ///
    /// # Panics
    ///
    /// Never: the connection is opened on the line above the unwrap.
    pub async fn client(&mut self) -> Result<&Client> {
        if self.expired() {
            self.close();
        }
        if self.open.is_none() {
            self.open = Some(self.connect().await?);
        }
        Ok(&self
            .open
            .as_ref()
            .expect("the connection was just opened")
            .client)
    }

    /// How long the current connection has been open, or `None` when there is
    /// none.
    #[must_use]
    pub fn age(&self) -> Option<Duration> {
        self.open.as_ref().map(|open| open.opened.elapsed())
    }

    /// Drop the connection. The next [`client`](Self::client) opens a new one.
    pub fn close(&mut self) {
        if let Some(open) = self.open.take() {
            open.driver.abort();
        }
    }

    fn expired(&self) -> bool {
        self.age().is_some_and(|age| age >= self.max_age)
    }

    async fn connect(&self) -> Result<Open> {
        let (client, connection) = self
            .config
            .connect(NoTls)
            .await
            .context("connect to PostgreSQL")?;
        // The connection drives the protocol; it ends when the client drops.
        let driver = tokio::spawn(async move {
            let _ended = connection.await;
        });
        Ok(Open {
            client,
            driver,
            opened: Instant::now(),
        })
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

#[cfg(test)]
mod tests;
