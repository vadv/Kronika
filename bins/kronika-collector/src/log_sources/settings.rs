//! Asking a server which log it writes and who it is.
//!
//! Both `PostgreSQL` and `PgBouncer` know where their own log goes, so the
//! collector asks them rather than having the operator declare it a second
//! time. What comes back also carries the identity that ends up in every row.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr as _;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use kronika_source_pg::Session;
use kronika_source_pg::query::{self, QueryStats};
use tokio_postgres::config::Host;
use tokio_postgres::types::Type;
use tokio_postgres::{Config, NoTls};

use crate::pg_sources::{ConnectionObservation, PgObservation, QueryObservation, QueryOutcome};

const DEFAULT_PORT: u16 = 5432;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

const INITIAL_POSTGRES_FACTS_QUERY: &str = "/* kronika: */ SELECT control.system_identifier, \
    current_setting('log_line_prefix') AS log_line_prefix, \
    current_setting('data_directory') AS data_directory, \
    pg_current_logfile() AS current_logfile \
    FROM pg_control_system() AS control";

const CACHED_POSTGRES_FACTS_QUERY: &str = "/* kronika: */ SELECT NULL::bigint AS system_identifier, \
    current_setting('log_line_prefix') AS log_line_prefix, \
    current_setting('data_directory') AS data_directory, \
    pg_current_logfile() AS current_logfile";

/// A parsed connection and the only identity safe to put in a log line.
pub(super) struct ConnectionTarget {
    config: Config,
    label: String,
    source_index: usize,
    system_identifier: OnceLock<u64>,
}

impl ConnectionTarget {
    /// Parse one configured connection without retaining its original text.
    pub(super) fn parse(raw: &str, source_index: usize) -> Result<Self, InvalidConnection> {
        let config = Config::from_str(raw).map_err(|_error| InvalidConnection)?;
        validate_endpoints(&config)?;
        let label = connection_label(&config);
        Ok(Self {
            config,
            label,
            source_index,
            system_identifier: OnceLock::new(),
        })
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) const fn source_index(&self) -> usize {
        self.source_index
    }

    fn postgres_facts_query(&self) -> &'static str {
        if self.system_identifier.get().is_some() {
            CACHED_POSTGRES_FACTS_QUERY
        } else {
            INITIAL_POSTGRES_FACTS_QUERY
        }
    }

    fn remember_system_identifier(&self, identifier: u64) -> u64 {
        *self.system_identifier.get_or_init(|| identifier)
    }
}

impl fmt::Debug for ConnectionTarget {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ConnectionTarget")
            .field("label", &self.label)
            .finish_non_exhaustive()
    }
}

/// Deliberately carries neither parser details nor the rejected input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct InvalidConnection;

fn validate_endpoints(config: &Config) -> Result<(), InvalidConnection> {
    let hosts = config.get_hosts().len();
    let hostaddrs = config.get_hostaddrs().len();
    let endpoints = hosts.max(hostaddrs);
    let ports = config.get_ports().len();
    if endpoints == 0
        || (hosts != 0 && hostaddrs != 0 && hosts != hostaddrs)
        || !matches!(ports, 0 | 1) && ports != endpoints
    {
        return Err(InvalidConnection);
    }
    Ok(())
}

fn connection_label(config: &Config) -> String {
    let user = config.get_user();
    let ports = config.get_ports();
    if config.get_hosts().is_empty() {
        return config
            .get_hostaddrs()
            .iter()
            .enumerate()
            .map(|(index, host)| endpoint(user, &ip_label(*host), port_at(ports, index)))
            .collect::<Vec<_>>()
            .join(",");
    }
    config
        .get_hosts()
        .iter()
        .enumerate()
        .map(|(index, host)| {
            let host = match host {
                Host::Tcp(host) => tcp_label(host),
                #[cfg(unix)]
                Host::Unix(path) => format!("unix:{}", path.display()),
            };
            endpoint(user, &host, port_at(ports, index))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn port_at(ports: &[u16], index: usize) -> u16 {
    match ports {
        [] => DEFAULT_PORT,
        [port] => *port,
        many => many.get(index).copied().unwrap_or(DEFAULT_PORT),
    }
}

fn endpoint(user: Option<&str>, host: &str, port: u16) -> String {
    user.map_or_else(
        || format!("{host}:{port}"),
        |user| format!("{user}@{host}:{port}"),
    )
}

fn tcp_label(host: &str) -> String {
    if host.starts_with('[') && host.ends_with(']') {
        return host.to_owned();
    }
    if host.contains(':') {
        format!("[{host}]")
    } else {
        host.to_owned()
    }
}

fn ip_label(host: IpAddr) -> String {
    match host {
        IpAddr::V4(host) => host.to_string(),
        IpAddr::V6(host) => format!("[{host}]"),
    }
}

/// What one `PostgreSQL` server said about itself.
#[derive(Debug)]
pub(super) struct PostgresServer {
    /// The file the server is writing right now, or `None` when
    /// `logging_collector` is off and there is no file to follow.
    pub(super) log_path: Option<String>,
    /// The layout of a `stderr` line's prefix.
    pub(super) line_prefix: String,
    /// Generated at `initdb`, so it survives restarts, renames and moves.
    pub(super) system_identifier: u64,
}

/// What one `PgBouncer` said about itself.
#[derive(Debug)]
pub(super) struct PgBouncerServer {
    /// Empty when `logfile` is unset, which means the pooler writes to stderr
    /// and there is no file to follow.
    pub(super) log_path: Option<String>,
}

/// Ask `PostgreSQL` for its log file, its line layout and its identity.
///
/// # Errors
///
/// Returns an error when the connection cannot be made or a query fails.
pub(super) async fn postgres(
    target: &ConnectionTarget,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<PostgresServer> {
    let connect_started = Instant::now();
    let connected = tokio::time::timeout(CONNECT_TIMEOUT, target.config.connect(NoTls)).await;
    let (client, connection) = match connected {
        Ok(Ok(connected)) => connected,
        Ok(Err(error)) => {
            observe(PgObservation::Connection(ConnectionObservation {
                database: target.label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: false,
                closed: false,
            }));
            return Err(error).context("connect to PostgreSQL");
        }
        Err(elapsed) => {
            observe(PgObservation::Connection(ConnectionObservation {
                database: target.label().to_owned(),
                elapsed: connect_started.elapsed(),
                timeout: true,
                closed: false,
            }));
            return Err(elapsed).context("connect to PostgreSQL timed out");
        }
    };
    // The connection drives the protocol and ends when the client is dropped.
    let driver = tokio::spawn(connection);
    let mut stats = QueryStats::default();
    let query_started = Instant::now();
    let read = tokio::time::timeout(QUERY_TIMEOUT, async {
        let facts = query::read_one(
            Session::new(&client, 0),
            target.postgres_facts_query(),
            std::iter::empty::<(String, Type)>(),
            0,
            &mut stats,
            |row| PostgresFacts {
                system_identifier: row.get("system_identifier"),
                line_prefix: row.get("log_line_prefix"),
                data_directory: row.get("data_directory"),
                logfile: row.get("current_logfile"),
            },
        )
        .await
        .context("read PostgreSQL log facts")?;
        let identifier = facts
            .system_identifier
            .map(|identifier| {
                #[expect(
                    clippy::cast_sign_loss,
                    reason = "the server reports the identifier as its signed bit pattern"
                )]
                let identifier = identifier as u64;
                target.remember_system_identifier(identifier)
            })
            .or_else(|| target.system_identifier.get().copied())
            .context("PostgreSQL log facts omitted system_identifier before it was cached")?;
        Ok::<_, anyhow::Error>(PostgresServer {
            log_path: facts
                .logfile
                .map(|name| absolute(&facts.data_directory, &name)),
            line_prefix: facts.line_prefix,
            system_identifier: identifier,
        })
    })
    .await;
    drop(client);
    driver.abort();
    match read {
        Ok(Ok(server)) => {
            observe(PgObservation::Query(QueryObservation {
                query_name: "postgres_log_facts",
                database: target.label().to_owned(),
                elapsed: query_started.elapsed(),
                stats,
                outcome: QueryOutcome::Success,
            }));
            Ok(server)
        }
        Ok(Err(error)) => {
            observe(PgObservation::Query(QueryObservation {
                query_name: "postgres_log_facts",
                database: target.label().to_owned(),
                elapsed: query_started.elapsed(),
                stats,
                outcome: QueryOutcome::Error,
            }));
            Err(error)
        }
        Err(elapsed) => {
            observe(PgObservation::Query(QueryObservation {
                query_name: "postgres_log_facts",
                database: target.label().to_owned(),
                elapsed: query_started.elapsed(),
                stats,
                outcome: QueryOutcome::Timeout,
            }));
            Err(elapsed).context("read PostgreSQL log facts timed out")
        }
    }
}

struct PostgresFacts {
    system_identifier: Option<i64>,
    line_prefix: String,
    data_directory: String,
    logfile: Option<String>,
}

/// Ask `PgBouncer` for its log file and where it listens.
///
/// `SHOW CONFIG` needs the account to be in `stats_users` or `admin_users`;
/// no administrative right beyond that.
///
/// # Errors
///
/// Returns an error when the connection cannot be made or the query fails.
pub(super) async fn pgbouncer(target: &ConnectionTarget) -> Result<PgBouncerServer> {
    let (client, connection) = tokio::time::timeout(CONNECT_TIMEOUT, target.config.connect(NoTls))
        .await
        .context("connect to PgBouncer timed out")?
        .context("connect to PgBouncer")?;
    let driver = tokio::spawn(connection);
    let read = tokio::time::timeout(QUERY_TIMEOUT, async {
        // The admin console speaks the protocol but not the extended query
        // path, so this is a simple query and the rows come back as text.
        let rows = client
            .simple_query("SHOW CONFIG")
            .await
            .context("read SHOW CONFIG from PgBouncer")?;
        let mut settings = Settings::default();
        for row in &rows {
            if let tokio_postgres::SimpleQueryMessage::Row(row) = row {
                settings.take(row.get("key"), row.get("value"));
            }
        }
        Ok::<_, anyhow::Error>(settings.finish())
    })
    .await
    .context("read SHOW CONFIG from PgBouncer timed out");
    drop(client);
    driver.abort();
    read?
}

/// The one `SHOW CONFIG` key worth keeping.
#[derive(Debug, Default)]
struct Settings {
    logfile: Option<String>,
}

impl Settings {
    fn take(&mut self, key: Option<&str>, value: Option<&str>) {
        let (Some(key), Some(value)) = (key, value) else {
            return;
        };
        if key == "logfile" {
            self.logfile = Some(value.to_owned());
        }
    }

    fn finish(self) -> PgBouncerServer {
        PgBouncerServer {
            log_path: self.logfile.filter(|path| !path.trim().is_empty()),
        }
    }
}

/// `pg_current_logfile()` reports relative to the data directory.
fn absolute(data_directory: &str, name: &str) -> String {
    if name.starts_with('/') {
        return name.to_owned();
    }
    format!("{}/{name}", data_directory.trim_end_matches('/'))
}

#[cfg(test)]
mod tests;
