//! Asking a server which log it writes and who it is.
//!
//! Both `PostgreSQL` and `PgBouncer` know where their own log goes, so the
//! collector asks them rather than having the operator declare it a second
//! time. What comes back also carries the identity that ends up in every row.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr as _;
use std::time::{Duration, Instant};

use anyhow::{Context as _, Result};
use futures_util::TryStreamExt as _;
use kronika_source_pg::Session;
use kronika_source_pg::query::QueryStats;
use tokio_postgres::config::Host;
use tokio_postgres::{Config, NoTls, SimpleQueryMessage};

use crate::pg_sources::{ConnectionObservation, PgObservation, QueryObservation, QueryOutcome};

const DEFAULT_PORT: u16 = 5432;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

const POSTGRES_LOG_FACTS_QUERY: &str = "/* kronika: */ SELECT \
    current_setting('log_line_prefix') AS line_prefix, \
    current_setting('data_directory') AS data_directory, \
    pg_current_logfile() AS log_path";
const POSTGRES_SYSTEM_IDENTIFIER_QUERY: &str =
    "/* kronika: */ SELECT system_identifier::text AS system_identifier FROM pg_control_system()";

/// A parsed connection and the only identity safe to put in a log line.
pub(super) struct ConnectionTarget {
    config: Config,
    label: String,
    source_index: usize,
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
        })
    }

    pub(super) fn label(&self) -> &str {
        &self.label
    }

    pub(super) const fn source_index(&self) -> usize {
        self.source_index
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
    pub(super) system_identifier: Option<u64>,
    /// The identity query failed and should be retried on the next rescan.
    pub(super) identity_unavailable: bool,
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
/// Returns an error when the connection or the refresh query fails. A failed
/// first identity query leaves the identity empty so the next rescan retries it.
pub(super) async fn postgres(
    target: &ConnectionTarget,
    cached_system_identifier: Option<u64>,
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
    let mut facts_stats = QueryStats::default();
    let facts_started = Instant::now();
    let facts = tokio::time::timeout(
        QUERY_TIMEOUT,
        read_log_facts(Session::new(&client, 0), &mut facts_stats),
    )
    .await;
    let (line_prefix, data_directory, logfile) = match facts {
        Ok(Ok(facts)) => {
            observe_query(
                observe,
                target,
                "postgres_log_facts",
                facts_started,
                facts_stats,
                QueryOutcome::Success,
            );
            facts
        }
        Ok(Err(error)) => {
            observe_query(
                observe,
                target,
                "postgres_log_facts",
                facts_started,
                facts_stats,
                QueryOutcome::Error,
            );
            drop(client);
            driver.abort();
            return Err(error);
        }
        Err(elapsed) => {
            observe_query(
                observe,
                target,
                "postgres_log_facts",
                facts_started,
                facts_stats,
                QueryOutcome::Timeout,
            );
            drop(client);
            driver.abort();
            return Err(elapsed).context("read PostgreSQL log facts timed out");
        }
    };
    let (system_identifier, identity_unavailable) = match cached_system_identifier {
        Some(identifier) => (Some(identifier), false),
        None => {
            let mut stats = QueryStats::default();
            let started = Instant::now();
            let identity = tokio::time::timeout(
                QUERY_TIMEOUT,
                read_system_identifier(Session::new(&client, 0), &mut stats),
            )
            .await;
            match identity {
                Ok(Ok(identifier)) => {
                    observe_query(
                        observe,
                        target,
                        "postgres_system_identifier",
                        started,
                        stats,
                        QueryOutcome::Success,
                    );
                    (Some(identifier), false)
                }
                Ok(Err(_error)) => {
                    observe_query(
                        observe,
                        target,
                        "postgres_system_identifier",
                        started,
                        stats,
                        QueryOutcome::Error,
                    );
                    (None, true)
                }
                Err(_elapsed) => {
                    observe_query(
                        observe,
                        target,
                        "postgres_system_identifier",
                        started,
                        stats,
                        QueryOutcome::Timeout,
                    );
                    (None, true)
                }
            }
        }
    };
    drop(client);
    driver.abort();
    Ok(PostgresServer {
        log_path: logfile.map(|name| absolute(&data_directory, &name)),
        line_prefix,
        system_identifier,
        identity_unavailable,
    })
}

fn observe_query(
    observe: &mut (dyn FnMut(PgObservation) + Send),
    target: &ConnectionTarget,
    query_name: &'static str,
    started: Instant,
    stats: QueryStats,
    outcome: QueryOutcome,
) {
    observe(PgObservation::Query(QueryObservation {
        query_name,
        database: target.label().to_owned(),
        elapsed: started.elapsed(),
        stats,
        outcome,
    }));
}

async fn read_log_facts(
    session: Session<'_>,
    stats: &mut QueryStats,
) -> Result<(String, String, Option<String>)> {
    let stream = session
        .simple_stream(POSTGRES_LOG_FACTS_QUERY, stats)
        .await
        .context("read PostgreSQL log settings")?;
    let mut stream = std::pin::pin!(stream);
    let mut row = None;
    while let Some(message) = stream.try_next().await? {
        if let SimpleQueryMessage::Row(found) = message {
            row = Some(found);
        }
    }
    let row = row.context("PostgreSQL log settings returned no row")?;
    let line_prefix = row
        .get("line_prefix")
        .context("PostgreSQL log settings omitted line_prefix")?
        .to_owned();
    let data_directory = row
        .get("data_directory")
        .context("PostgreSQL log settings omitted data_directory")?
        .to_owned();
    let log_path = row.get("log_path").map(str::to_owned);
    Ok((line_prefix, data_directory, log_path))
}

async fn read_system_identifier(session: Session<'_>, stats: &mut QueryStats) -> Result<u64> {
    let stream = session
        .simple_stream(POSTGRES_SYSTEM_IDENTIFIER_QUERY, stats)
        .await
        .context("read system_identifier from pg_control_system()")?;
    let mut stream = std::pin::pin!(stream);
    let mut row = None;
    while let Some(message) = stream.try_next().await? {
        if let SimpleQueryMessage::Row(found) = message {
            row = Some(found);
        }
    }
    let row = row.context("pg_control_system() returned no row")?;
    let identifier = row
        .get("system_identifier")
        .context("pg_control_system() omitted system_identifier")?
        .parse::<i64>()
        .context("parse system_identifier from pg_control_system()")?;
    #[expect(
        clippy::cast_sign_loss,
        reason = "the server reports the identifier as its signed bit pattern"
    )]
    Ok(identifier as u64)
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
    let (client, connection) = target
        .config
        .connect(NoTls)
        .await
        .context("connect to PgBouncer")?;
    let driver = tokio::spawn(connection);
    let read = async {
        // The admin console speaks the protocol but not the extended query
        // path, so this is a simple query and the rows come back as text.
        let rows = client
            .simple_query("SHOW CONFIG")
            .await
            .context("read SHOW CONFIG from PgBouncer")?;
        let mut settings = Settings::default();
        for row in &rows {
            if let SimpleQueryMessage::Row(row) = row {
                settings.take(row.get("key"), row.get("value"));
            }
        }
        Ok::<_, anyhow::Error>(settings.finish())
    }
    .await;
    drop(client);
    driver.abort();
    read
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
