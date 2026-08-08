//! Reading the configured log files into sections.
//!
//! Each file is followed from where the previous process stopped; the offsets
//! are written back to `<out>/log.offsets` after every read, so a restart
//! resumes instead of re-reading or skipping. A file that cannot be read is one
//! warning line and no rows.

mod buffering;
mod settings;

use std::path::PathBuf;
use std::time::Instant;

use kronika_source_log::pgbouncer::PgBouncerLog;
use kronika_source_log::postgres::{Events, Format, LinePrefix, PgLog};
use kronika_source_log::{Offsets, pgbouncer};

use crate::config::Config;
use crate::logging::{
    LogLevel, field, log_collection_failure, log_collection_finish, log_collection_start, log_event,
};
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_log_sources;

/// The name each source's offset is stored under.
const POSTGRESQL: &str = "postgresql";
const PGBOUNCER: &str = "pgbouncer";

/// The type id the `PostgreSQL` log reports its collection under. The read
/// produces seven sections at once, and the errors are the one an operator
/// looks for first.
const PG_LOG_TYPE_ID: u32 = 2_001_001;
const PGBOUNCER_TYPE_ID: u32 = 2_100_001;

/// What one read of the configured logs produced.
#[derive(Debug, Default)]
pub(crate) struct LogRows {
    postgres: Events,
    pgbouncer: Vec<pgbouncer::Event>,
}

/// The configured log files and where each of them was left off.
#[derive(Debug)]
pub(crate) struct LogSources {
    offsets: Offsets,
    postgres: Option<PgLog>,
    pgbouncer: Option<PgBouncerLog>,
    dsn: Option<String>,
}

impl LogSources {
    /// Open the configured log files, resuming from `<out>/log.offsets`.
    ///
    /// # Errors
    ///
    /// Returns the error of reading the offsets file. A log file that is not
    /// there yet is not an error: it is opened on the read that finds it.
    pub(crate) fn open(config: &Config) -> anyhow::Result<Self> {
        let offsets = Offsets::load(&config.out_dir)?;
        let postgres = config
            .pg_log
            .clone()
            .map(|path| open_postgres(path, &offsets));
        let pgbouncer = config
            .pgbouncer_log
            .clone()
            .map(|path| PgBouncerLog::new(path, offsets.get(PGBOUNCER)));
        Ok(Self {
            offsets,
            postgres,
            pgbouncer,
            dsn: config.pg_dsn.clone(),
        })
    }

    /// Ask the server for its `log_line_prefix`, if a `stderr` log is being
    /// read without one.
    ///
    /// The prefix is what names the database and the user on a `stderr`
    /// record, so a server that is not up yet costs those two fields until it
    /// is, and nothing else.
    pub(crate) async fn refresh_prefix(&mut self) {
        let Some(dsn) = self.dsn.as_deref() else {
            return;
        };
        let Some(log) = self.postgres.as_mut() else {
            return;
        };
        if log.format() != Format::Stderr || log.has_prefix() {
            return;
        }
        match settings::log_line_prefix(dsn).await {
            Ok(setting) => {
                log_event(
                    LogLevel::Info,
                    "pg_log_line_prefix",
                    &[field("setting", setting.as_str())],
                );
                log.set_prefix(LinePrefix::parse(&setting));
            }
            Err(error) => log_event(
                LogLevel::Warn,
                "collection_degraded",
                &[
                    field("collection", "pg_log_line_prefix"),
                    field("reason", format!("{error:#}")),
                ],
            ),
        }
    }

    /// Read what the configured logs have written since the last tick.
    pub(crate) fn collect(&mut self, due: &DueSet, now: i64) -> LogRows {
        let mut rows = LogRows::default();
        if !due.has(SourceKind::Logs) {
            return rows;
        }
        if let Some(log) = self.postgres.as_mut() {
            let started = Instant::now();
            let source = log.format().as_str();
            log_collection_start(PG_LOG_TYPE_ID, source);
            match log.read(now) {
                Ok(events) => {
                    log_collection_finish(PG_LOG_TYPE_ID, source, events.rows(), started.elapsed());
                    rows.postgres = events;
                }
                Err(error) => {
                    log_collection_failure(PG_LOG_TYPE_ID, source, &error, started.elapsed());
                }
            }
            self.offsets.set(POSTGRESQL, log.position());
        }
        if let Some(log) = self.pgbouncer.as_mut() {
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
                    rows.pgbouncer = events;
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
            self.offsets.set(PGBOUNCER, log.position());
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

fn open_postgres(path: PathBuf, offsets: &Offsets) -> PgLog {
    let log = PgLog::new(path, offsets.get(POSTGRESQL), None);
    log_event(
        LogLevel::Info,
        "pg_log_source",
        &[
            field("path", log.path().display()),
            field("format", log.format().as_str()),
        ],
    );
    log
}
