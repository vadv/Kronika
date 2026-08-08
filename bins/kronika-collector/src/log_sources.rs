//! Reading the configured log files into sections.
//!
//! A source is named one of two ways: a DSN, and then the server itself says
//! which file it writes and who it is, or a path or glob, and then the file is
//! read for what it holds and nothing is known about the writer. Both may be
//! given; a file reached both ways is followed once.
//!
//! Each file is followed from where the previous process stopped; the offsets
//! are keyed by path in `<out>/log.offsets`, so a restart resumes instead of
//! re-reading or skipping. A file that cannot be read is one warning line
//! every rescan and no rows.

mod buffering;
mod paths;
mod settings;

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::postgres::{Events, LinePrefix, PgLog};
use kronika_source_log::{Offsets, pgbouncer};

use crate::config::Config;
use crate::logging::{
    LogLevel, field, log_collection_failure, log_collection_finish, log_collection_start, log_event,
};
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_log_sources;

/// How often a server is asked again and the globs are expanded again. One
/// interval covers every way a source can be missing: the server is down, the
/// file is not there yet, a new instance appeared.
const RESCAN: Duration = Duration::from_mins(5);

/// The type id each source reports its collection under. A `PostgreSQL` read
/// produces seven sections at once, and the errors are the one an operator
/// looks for first.
const PG_LOG_TYPE_ID: u32 = 2_001_001;
const PGBOUNCER_TYPE_ID: u32 = 2_100_001;

/// What one read of one `PostgreSQL` log produced.
#[derive(Debug)]
pub(crate) struct PostgresBatch {
    pub(crate) system_identifier: Option<u64>,
    pub(crate) source_file: String,
    pub(crate) events: Events,
}

/// What one read of one `PgBouncer` log produced.
#[derive(Debug)]
pub(crate) struct PgBouncerBatch {
    pub(crate) source_file: String,
    pub(crate) events: Vec<pgbouncer::Event>,
}

/// What one read of every configured log produced.
#[derive(Debug, Default)]
pub(crate) struct LogRows {
    pub(crate) postgres: Vec<PostgresBatch>,
    pub(crate) pgbouncer: Vec<PgBouncerBatch>,
}

/// One followed `PostgreSQL` log.
#[derive(Debug)]
struct PostgresSource {
    log: PgLog,
    system_identifier: Option<u64>,
}

/// What a rescan decided one `PostgreSQL` file should be read as.
#[derive(Debug, Default)]
struct PostgresFacts {
    system_identifier: Option<u64>,
    line_prefix: Option<String>,
}

/// The configured logs and where each of them was left off.
#[derive(Debug)]
pub(crate) struct LogSources {
    offsets: Offsets,
    pg_dsns: Vec<String>,
    pg_logs: Vec<String>,
    pgbouncer_dsns: Vec<String>,
    pgbouncer_logs: Vec<String>,
    postgres: Vec<PostgresSource>,
    pgbouncer: Vec<PgBouncerLog>,
    next_scan: Option<Instant>,
}

impl LogSources {
    /// Take the configuration and resume from `<out>/log.offsets`.
    ///
    /// # Errors
    ///
    /// Returns the error of reading the offsets file. Nothing is opened here:
    /// the first rescan finds what exists.
    pub(crate) fn open(config: &Config) -> anyhow::Result<Self> {
        Ok(Self {
            offsets: Offsets::load(&config.out_dir)?,
            pg_dsns: config.pg_dsns.clone(),
            pg_logs: config.pg_logs.clone(),
            pgbouncer_dsns: config.pgbouncer_dsns.clone(),
            pgbouncer_logs: config.pgbouncer_logs.clone(),
            postgres: Vec::new(),
            pgbouncer: Vec::new(),
            next_scan: None,
        })
    }

    /// Ask every configured server what it writes, expand every glob, and
    /// bring the followed set in line with what came back.
    pub(crate) async fn rescan(&mut self) {
        let now = Instant::now();
        if self.next_scan.is_some_and(|due| now < due) {
            return;
        }
        self.next_scan = Some(now + RESCAN);
        self.rescan_postgres().await;
        self.rescan_pgbouncer().await;
    }

    async fn rescan_postgres(&mut self) {
        let mut wanted: BTreeMap<PathBuf, PostgresFacts> = BTreeMap::new();
        for dsn in &self.pg_dsns {
            match settings::postgres(dsn).await {
                Ok(server) => {
                    let Some(path) = server.log_path else {
                        log_source_absent(dsn, "logging_collector is off, so there is no log file");
                        continue;
                    };
                    let path = PathBuf::from(path);
                    if !path.is_file() {
                        log_source_unreadable(&path, dsn);
                        continue;
                    }
                    wanted.insert(
                        path,
                        PostgresFacts {
                            system_identifier: Some(server.system_identifier),
                            line_prefix: Some(server.line_prefix),
                        },
                    );
                }
                Err(error) => log_source_unreachable("postgresql", dsn, &error),
            }
        }
        for entry in &self.pg_logs {
            for path in paths::expand(entry) {
                wanted.entry(path).or_default();
            }
        }
        self.postgres
            .retain(|source| wanted.contains_key(source.log.path()));
        for (path, facts) in wanted {
            let prefix = facts.line_prefix.as_deref().map(LinePrefix::parse);
            if let Some(existing) = self
                .postgres
                .iter_mut()
                .find(|source| source.log.path() == path)
            {
                existing.system_identifier = facts.system_identifier;
                if let Some(prefix) = prefix {
                    existing.log.set_prefix(prefix);
                }
                continue;
            }
            let position = self.offsets.get(&key(&path));
            let log = PgLog::new(path, position, prefix);
            log_source_opened("postgresql", log.path(), log.format().as_str());
            self.postgres.push(PostgresSource {
                log,
                system_identifier: facts.system_identifier,
            });
        }
    }

    async fn rescan_pgbouncer(&mut self) {
        let mut wanted: Vec<PathBuf> = Vec::new();
        for dsn in &self.pgbouncer_dsns {
            match settings::pgbouncer(dsn).await {
                Ok(server) => {
                    let Some(path) = server.log_path else {
                        log_source_absent(dsn, "logfile is unset, so the pooler writes to stderr");
                        continue;
                    };
                    let path = PathBuf::from(path);
                    if path.is_file() {
                        wanted.push(path);
                    } else {
                        log_source_unreadable(&path, dsn);
                    }
                }
                Err(error) => log_source_unreachable("pgbouncer", dsn, &error),
            }
        }
        for entry in &self.pgbouncer_logs {
            wanted.extend(paths::expand(entry));
        }
        wanted.sort();
        wanted.dedup();
        self.pgbouncer
            .retain(|log| wanted.contains(&log.path().to_path_buf()));
        for path in wanted {
            if self.pgbouncer.iter().any(|log| log.path() == path) {
                continue;
            }
            let position = self.offsets.get(&key(&path));
            let log = PgBouncerLog::new(path, position);
            log_source_opened("pgbouncer", log.path(), "pgbouncer");
            self.pgbouncer.push(log);
        }
    }

    /// Read what the followed logs have written since the last tick.
    pub(crate) fn collect(&mut self, due: &DueSet, now: i64) -> LogRows {
        let mut rows = LogRows::default();
        if !due.has(SourceKind::Logs) {
            return rows;
        }
        for source in &mut self.postgres {
            let started = Instant::now();
            let format = source.log.format().as_str();
            log_collection_start(PG_LOG_TYPE_ID, format);
            match source.log.read(now) {
                Ok(events) => {
                    log_collection_finish(PG_LOG_TYPE_ID, format, events.rows(), started.elapsed());
                    rows.postgres.push(PostgresBatch {
                        system_identifier: source.system_identifier,
                        source_file: source.log.path().display().to_string(),
                        events,
                    });
                }
                Err(error) => {
                    log_collection_failure(PG_LOG_TYPE_ID, format, &error, started.elapsed());
                }
            }
            self.offsets
                .set(&key(source.log.path()), source.log.position());
        }
        for log in &mut self.pgbouncer {
            let started = Instant::now();
            log_collection_start(PGBOUNCER_TYPE_ID, "pgbouncer");
            match log.read() {
                Ok(events) => {
                    log_collection_finish(
                        PGBOUNCER_TYPE_ID,
                        "pgbouncer",
                        events.len(),
                        started.elapsed(),
                    );
                    rows.pgbouncer.push(PgBouncerBatch {
                        source_file: log.path().display().to_string(),
                        events,
                    });
                }
                Err(error) => {
                    log_collection_failure(
                        PGBOUNCER_TYPE_ID,
                        "pgbouncer",
                        &error,
                        started.elapsed(),
                    );
                }
            }
            self.offsets.set(&key(log.path()), log.position());
        }
        self.save_offsets();
        rows
    }

    /// A restart re-reads from the last saved offset, so a failed save costs
    /// the lines between it and the restart.
    fn save_offsets(&self) {
        if let Err(error) = self.offsets.save() {
            log_event(
                LogLevel::Warn,
                "log_offsets_save_failure",
                &[field("error", format!("{error:#}"))],
            );
        }
    }
}

/// Offsets are keyed by the path, so a file keeps its place across restarts
/// and a file that stops existing stops being written out.
fn key(path: &std::path::Path) -> String {
    path.display().to_string()
}

fn log_source_opened(kind: &str, path: &std::path::Path, format: &str) {
    log_event(
        LogLevel::Info,
        "log_source_opened",
        &[
            field("kind", kind),
            field("path", path.display()),
            field("format", format),
        ],
    );
}

fn log_source_unreachable(kind: &str, dsn: &str, error: &anyhow::Error) {
    log_event(
        LogLevel::Warn,
        "log_source_unreachable",
        &[
            field("kind", kind),
            field("dsn", redact(dsn)),
            field("error", format!("{error:#}")),
        ],
    );
}

fn log_source_absent(dsn: &str, reason: &str) {
    log_event(
        LogLevel::Warn,
        "log_source_absent",
        &[field("dsn", redact(dsn)), field("reason", reason)],
    );
}

fn log_source_unreadable(path: &std::path::Path, dsn: &str) {
    log_event(
        LogLevel::Warn,
        "log_source_unreadable",
        &[
            field("path", path.display()),
            field("dsn", redact(dsn)),
            field(
                "hint",
                "mount the directory here and name the file in KRONIKA_PG_LOGS",
            ),
        ],
    );
}

/// A DSN may carry a password, and this line goes to an operator's log.
fn redact(dsn: &str) -> String {
    let mut out = String::with_capacity(dsn.len());
    let mut hidden = false;
    for part in dsn.split(' ') {
        if !out.is_empty() {
            out.push(' ');
        }
        if let Some(name) = part.strip_prefix("password=") {
            hidden = !name.is_empty();
            out.push_str("password=***");
        } else {
            out.push_str(part);
        }
    }
    if hidden || !dsn.contains('@') {
        return out;
    }
    // A URL keeps its credentials in front of the host.
    match (out.find("://"), out.rfind('@')) {
        (Some(scheme), Some(at)) if scheme + 3 < at => {
            let mut redacted = out.clone();
            redacted.replace_range(scheme + 3..at, "***");
            redacted
        }
        _plain => out,
    }
}

#[cfg(test)]
mod tests;
