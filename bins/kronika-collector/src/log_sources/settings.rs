//! Asking a server which log it writes and who it is.
//!
//! Both `PostgreSQL` and `PgBouncer` know where their own log goes, so the
//! collector asks them rather than having the operator declare it a second
//! time. What comes back also carries the identity that ends up in every row.

use std::fmt;
use std::net::IpAddr;
use std::str::FromStr as _;

use anyhow::{Context as _, Result};
use tokio_postgres::config::Host;
use tokio_postgres::{Config, NoTls};

const DEFAULT_PORT: u16 = 5432;

/// A parsed connection and the only identity safe to put in a log line.
pub(super) struct ConnectionTarget {
    config: Config,
    label: String,
}

impl ConnectionTarget {
    /// Parse one configured connection without retaining its original text.
    pub(super) fn parse(raw: &str) -> Result<Self, InvalidConnection> {
        let config = Config::from_str(raw).map_err(|_error| InvalidConnection)?;
        validate_endpoints(&config)?;
        let label = connection_label(&config);
        Ok(Self { config, label })
    }

    pub(super) fn label(&self) -> &str {
        &self.label
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
pub(super) async fn postgres(target: &ConnectionTarget) -> Result<PostgresServer> {
    let (client, connection) = target
        .config
        .connect(NoTls)
        .await
        .context("connect to PostgreSQL")?;
    // The connection drives the protocol and ends when the client is dropped.
    let driver = tokio::spawn(connection);
    let read = async {
        let identity = client
            .query_one("SELECT system_identifier FROM pg_control_system()", &[])
            .await
            .context("read system_identifier from pg_control_system()")?;
        let prefix = client
            .query_one(
                "SELECT setting FROM pg_settings WHERE name = 'log_line_prefix'",
                &[],
            )
            .await
            .context("read log_line_prefix from pg_settings")?;
        let current = client
            .query_one(
                "SELECT current_setting('data_directory'), pg_current_logfile()",
                &[],
            )
            .await
            .context("read the current log file")?;
        let data_directory: String = current.get(0);
        let logfile: Option<String> = current.get(1);
        let identifier: i64 = identity.get(0);
        Ok::<_, anyhow::Error>(PostgresServer {
            log_path: logfile.map(|name| absolute(&data_directory, &name)),
            line_prefix: prefix.get(0),
            #[expect(
                clippy::cast_sign_loss,
                reason = "the server reports the identifier as its signed bit pattern"
            )]
            system_identifier: identifier as u64,
        })
    }
    .await;
    drop(client);
    driver.abort();
    read
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
            if let tokio_postgres::SimpleQueryMessage::Row(row) = row {
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
