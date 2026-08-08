//! One reusable connection to a `PostgreSQL` server.
//!
//! A healthy connection remains open across collection cycles. It is replaced
//! only after the driver reports it closed or the caller closes it following a
//! connection, protocol, or deadline failure.

use std::error::Error;
use std::fmt;
use std::time::Duration;

use anyhow::{Context as _, Result};
use tokio_postgres::{Config, NoTls};

use crate::Session;

/// Maximum time allowed for opening a `PostgreSQL` connection.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Failure to open a `PostgreSQL` connection.
#[derive(Debug)]
pub enum ConnectError {
    /// The explicit connection deadline elapsed.
    Timeout,
    /// `PostgreSQL` or its transport rejected the connection.
    PostgreSql(tokio_postgres::Error),
}

impl ConnectError {
    /// Whether this failure is the explicit connection deadline.
    #[must_use]
    pub const fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Timeout => write!(
                f,
                "connect to PostgreSQL timed out after {} seconds",
                CONNECT_TIMEOUT.as_secs()
            ),
            Self::PostgreSql(error) => fmt::Display::fmt(error, f),
        }
    }
}

impl Error for ConnectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Timeout => None,
            Self::PostgreSql(error) => Some(error),
        }
    }
}

/// A connection opened lazily and reused while healthy.
#[derive(Debug)]
pub struct Pool {
    config: Config,
    open: Option<Open>,
    next_generation: u64,
}

#[derive(Debug)]
struct Open {
    client: tokio_postgres::Client,
    driver: tokio::task::JoinHandle<()>,
    generation: u64,
}

impl Pool {
    /// Parse a DSN without opening a connection.
    ///
    /// # Errors
    ///
    /// Returns the parse error when `dsn` is neither a keyword string nor a
    /// connection URL.
    pub fn new(dsn: &str) -> Result<Self> {
        let config: Config = dsn.parse().context("parse the PostgreSQL DSN")?;
        Ok(Self::from_config(config))
    }

    /// Build a lazy pool from an already parsed configuration.
    #[must_use]
    pub const fn from_config(config: Config) -> Self {
        Self {
            config,
            open: None,
            next_generation: 1,
        }
    }

    /// The same server and credentials, connecting to `dbname` instead.
    #[must_use]
    pub fn on_database(&self, dbname: &str) -> Self {
        let mut config = self.config.clone();
        config.dbname(dbname);
        Self::from_config(config)
    }

    /// Safe database label used by per-query logs.
    #[must_use]
    pub fn database_label(&self) -> &str {
        self.config.get_dbname().unwrap_or("postgresql")
    }

    /// Return the healthy current session, connecting when necessary.
    ///
    /// # Errors
    ///
    /// Returns a finite-deadline timeout or the `PostgreSQL` connection error.
    pub async fn session(&mut self) -> std::result::Result<Session<'_>, ConnectError> {
        if let Some(open) = self.open.take() {
            if !open.client.is_closed() {
                let open = self.open.insert(open);
                return Ok(Session::new(&open.client, open.generation));
            }
            open.driver.abort();
        }
        let connected = self.connect().await?;
        let open = self.open.insert(connected);
        Ok(Session::new(&open.client, open.generation))
    }

    /// Return the existing healthy session only when its generation matches.
    ///
    /// This never reconnects. A closed connection is discarded, while a
    /// healthy connection from another generation remains available for the
    /// next collection pass.
    #[must_use]
    pub fn session_for_generation(&mut self, expected: u64) -> Option<Session<'_>> {
        let closed = self
            .open
            .as_ref()
            .is_some_and(|open| open.client.is_closed());
        if closed {
            self.close();
            return None;
        }
        let open = self.open.as_ref()?;
        (open.generation == expected).then(|| Session::new(&open.client, open.generation))
    }

    /// Generation of the current healthy connection.
    #[must_use]
    pub fn generation(&self) -> Option<u64> {
        self.open
            .as_ref()
            .filter(|open| !open.client.is_closed())
            .map(|open| open.generation)
    }

    /// Drop the connection. The next [`session`](Self::session) opens a new one.
    pub fn close(&mut self) {
        if let Some(open) = self.open.take() {
            open.driver.abort();
        }
    }

    async fn connect(&mut self) -> std::result::Result<Open, ConnectError> {
        let connecting = self.config.connect(NoTls);
        let (client, connection) = tokio::time::timeout(CONNECT_TIMEOUT, connecting)
            .await
            .map_err(|_elapsed| ConnectError::Timeout)?
            .map_err(ConnectError::PostgreSql)?;
        let generation = self.next_generation;
        self.next_generation = self.next_generation.saturating_add(1);
        let driver = tokio::spawn(async move {
            let _ended = connection.await;
        });
        Ok(Open {
            client,
            driver,
            generation,
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
