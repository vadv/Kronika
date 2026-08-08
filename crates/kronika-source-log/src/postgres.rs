//! Reading a `PostgreSQL` log, whichever of its three shapes it has.

mod csvlog;
mod events;
mod jsonlog;
mod normalize;
mod prefix;
mod stderr;

pub use events::{
    AutovacuumEvent, AutovacuumKind, CheckpointEvent, CheckpointPhase, ErrorGroup, Events,
    LifecycleEvent, LifecycleKind, LockWait, LockWaitKind, SlowQuery, TempFile,
};
pub use normalize::{ErrorCategory, classify_error, normalize_error, normalize_sql};
pub use prefix::LinePrefix;

use std::io;
use std::path::{Path, PathBuf};

use crate::tail::{Position, Record, Tail};
use crate::text::{MAX_TEXT_BYTES, truncate};

/// The shape a `PostgreSQL` log file is written in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    /// `log_destination = csvlog`, a fixed 23-column CSV plus whatever newer
    /// releases appended.
    Csvlog,
    /// `log_destination = jsonlog`, one JSON object per record (PG 15+).
    Jsonlog,
    /// `log_destination = stderr`, laid out by `log_line_prefix`.
    Stderr,
}

impl Format {
    /// The format `PostgreSQL` writes to a file with this name.
    ///
    /// `logging_collector` names the file after the destination it writes, so
    /// the extension is the whole rule.
    #[must_use]
    pub fn of(path: &Path) -> Self {
        match path.extension().and_then(|ext| ext.to_str()) {
            Some("csv") => Self::Csvlog,
            Some("json") => Self::Jsonlog,
            _ => Self::Stderr,
        }
    }

    /// The name used in log lines and documentation.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Csvlog => "csvlog",
            Self::Jsonlog => "jsonlog",
            Self::Stderr => "stderr",
        }
    }

    const fn continues(self) -> crate::tail::Continues {
        match self {
            Self::Csvlog => csvlog::continues,
            Self::Jsonlog => jsonlog::continues,
            Self::Stderr => stderr::continues,
        }
    }
}

/// The severity `PostgreSQL` gave a record.
///
/// `DEBUG`, `INFO` and `NOTICE` records carry nothing this collector records,
/// so they have no variant and their lines are dropped.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Severity {
    /// `LOG`: the severity checkpoints, autovacuum and lifecycle records use.
    Log,
    /// `WARNING`.
    Warning,
    /// `ERROR`.
    Error,
    /// `FATAL`.
    Fatal,
    /// `PANIC`.
    Panic,
}

impl Severity {
    /// The code stored in `pg_log_errors.severity`.
    #[must_use]
    pub const fn code(self) -> u8 {
        match self {
            Self::Error => 0,
            Self::Fatal => 1,
            Self::Panic => 2,
            Self::Warning => 3,
            Self::Log => 4,
        }
    }

    fn parse(value: &str) -> Option<Self> {
        match value {
            "LOG" => Some(Self::Log),
            "WARNING" => Some(Self::Warning),
            "ERROR" => Some(Self::Error),
            "FATAL" => Some(Self::Fatal),
            "PANIC" => Some(Self::Panic),
            _ => None,
        }
    }
}

/// One log record, with the fields the three formats agree on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PgRecord {
    /// When the record was written, unix microseconds.
    pub ts: i64,
    /// Severity.
    pub severity: Severity,
    /// `SQLSTATE`, when the format carries one.
    pub sqlstate: Option<String>,
    /// The message itself.
    pub message: String,
    /// `DETAIL`.
    pub detail: Option<String>,
    /// `HINT`.
    pub hint: Option<String>,
    /// `CONTEXT`.
    pub context: Option<String>,
    /// The statement the record was raised under.
    pub statement: Option<String>,
    /// Database name, when the format or `log_line_prefix` carries one.
    pub database: Option<String>,
    /// User name, when the format or `log_line_prefix` carries one.
    pub username: Option<String>,
}

impl PgRecord {
    fn new(ts: i64, severity: Severity, message: &str) -> Self {
        Self {
            ts,
            severity,
            sqlstate: None,
            message: truncate(message.trim(), MAX_TEXT_BYTES).to_owned(),
            detail: None,
            hint: None,
            context: None,
            statement: None,
            database: None,
            username: None,
        }
    }
}

/// A followed `PostgreSQL` log file.
#[derive(Debug)]
pub struct PgLog {
    tail: Tail,
    format: Format,
    prefix: Option<LinePrefix>,
}

impl PgLog {
    /// Follow `path`, resuming from `position`.
    ///
    /// `prefix` is the server's `log_line_prefix`; without it a `stderr` record
    /// still yields its time, severity and message, but not the database and
    /// user the prefix would have named.
    #[must_use]
    pub fn new(path: PathBuf, position: Position, prefix: Option<LinePrefix>) -> Self {
        Self {
            format: Format::of(&path),
            tail: Tail::new(path, position),
            prefix,
        }
    }

    /// The format this file is being read as.
    #[must_use]
    pub const fn format(&self) -> Format {
        self.format
    }

    /// The path being followed.
    #[must_use]
    pub fn path(&self) -> &Path {
        self.tail.path()
    }

    /// The offset to resume from after a restart.
    #[must_use]
    pub const fn position(&self) -> Position {
        self.tail.position()
    }

    /// Read and classify everything written since the last call.
    ///
    /// `now` stands in for the timestamp of a record whose format or
    /// `log_line_prefix` does not carry one.
    ///
    /// # Errors
    ///
    /// Returns the operating system's error for reading the file.
    pub fn read(&mut self, now: i64) -> io::Result<Events> {
        let records = self.tail.read(self.format.continues())?;
        let mut events = Events::default();
        for record in &records {
            if let Some(parsed) = self.parse(record, now) {
                events.add(&parsed);
            }
        }
        events.finish();
        Ok(events)
    }

    fn parse(&self, record: &Record, now: i64) -> Option<PgRecord> {
        match self.format {
            Format::Csvlog => csvlog::parse(&record.joined(), now),
            Format::Jsonlog => jsonlog::parse(record.first(), now),
            Format::Stderr => stderr::parse(record, self.prefix.as_ref(), now),
        }
    }
}
