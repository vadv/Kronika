//! Sequential `PostgreSQL` statistics collection.
//!
//! Every database keeps one healthy connection across collection cycles. A
//! failed or timed-out connection is closed and reopened on a later cycle.
//! Queries on every connection are awaited one at a time; no pipeline or
//! concurrent query task is created.

mod buffering;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use kronika_registry::pg_stat_statements_info::PgStatStatementsInfo;
use kronika_registry::pg_store_plans_info::PgStorePlansInfo;
use kronika_source_pg::activity::{self, ActivityRow, ActivityVersion};
use kronika_source_pg::archiver::ArchiverRow;
use kronika_source_pg::bgwriter::{self, BgwriterSnapshot};
use kronika_source_pg::checkpointer::{self, CheckpointerSnapshot};
use kronika_source_pg::database::{self, DatabaseRow, DatabaseVersion};
use kronika_source_pg::databases;
use kronika_source_pg::extension;
use kronika_source_pg::io::{self, IoRow, IoVersion};
use kronika_source_pg::locks::{self, LockRow, LocksVersion};
use kronika_source_pg::prepared_xacts::{self, PreparedXactsRow};
use kronika_source_pg::progress_vacuum::{self, ProgressVacuumRow};
use kronika_source_pg::query::{self, BatchError, BatchWrite};
use kronika_source_pg::settings::{self, SettingsRow};
use kronika_source_pg::statements::{self, StatementsRow, StatementsVersion};
use kronika_source_pg::statements_info;
use kronika_source_pg::store_plans::{self, Flavour, OsscRow, VadvRow};
use kronika_source_pg::store_plans_info;
use kronika_source_pg::user_indexes::{self, UserIndexesRow, UserIndexesVersion};
use kronika_source_pg::user_tables::{self, UserTablesRow, UserTablesVersion};
use kronika_source_pg::wal::{self, WalSnapshot};
use kronika_source_pg::{Pool, Session};

use crate::config::Config;
use crate::logging::{LogLevel, duration_ms, field, log_event};
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_pg_batch;

const SERVER_MAJOR_SQL: &str = "/* kronika: */ SELECT current_setting('server_version_num')::int4";
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);

/// A low-cardinality measurement hook for the canonical collector telemetry.
#[derive(Debug)]
pub(crate) enum PgObservation {
    Query(QueryObservation),
    Connection(ConnectionObservation),
}

/// One completed or interrupted SQL statement.
#[derive(Debug)]
pub(crate) struct QueryObservation {
    pub(crate) query_name: &'static str,
    pub(crate) database: String,
    pub(crate) elapsed: Duration,
    pub(crate) stats: query::QueryStats,
    pub(crate) outcome: QueryOutcome,
}

/// One failed connection attempt.
#[derive(Debug)]
pub(crate) struct ConnectionObservation {
    pub(crate) database: String,
    pub(crate) elapsed: Duration,
    pub(crate) timeout: bool,
    pub(crate) closed: bool,
}

/// Stable query completion classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryOutcome {
    Success,
    Error,
    Timeout,
    SinkError,
}

/// Emit the per-query diagnostic event while leaving interval aggregation to
/// the collector's canonical telemetry owner.
pub(crate) fn log_pg_observation(observation: PgObservation) {
    match observation {
        PgObservation::Query(observation) => {
            let fetch_elapsed = observation.stats.fetch_elapsed(observation.elapsed);
            let level = if fetch_elapsed > Duration::from_millis(500)
                || observation.outcome != QueryOutcome::Success
            {
                LogLevel::Warn
            } else {
                LogLevel::Debug
            };
            let outcome = match observation.outcome {
                QueryOutcome::Success => "success",
                QueryOutcome::Error => "error",
                QueryOutcome::Timeout => "timeout",
                QueryOutcome::SinkError => "sink_error",
            };
            log_event(
                level,
                "pg_query",
                &[
                    field("query_name", observation.query_name),
                    field("database", observation.database),
                    field("outcome", outcome),
                    field("elapsed_ms", duration_ms(observation.elapsed)),
                    field("fetch_ms", duration_ms(fetch_elapsed)),
                    field("rows", observation.stats.rows),
                    field(
                        "application_payload_from_postgres_bytes",
                        observation.stats.application_payload_from_postgres_bytes,
                    ),
                    field(
                        "application_payload_to_postgres_bytes",
                        observation.stats.application_payload_to_postgres_bytes,
                    ),
                    field("batches", observation.stats.batches),
                    field("encode_ms", duration_ms(observation.stats.encode_elapsed)),
                    field("append_ms", duration_ms(observation.stats.append_elapsed)),
                    field("encoded_bytes", observation.stats.encoded_bytes),
                    field("wal_bytes_appended", observation.stats.wal_bytes_appended),
                ],
            );
        }
        PgObservation::Connection(observation) => log_event(
            LogLevel::Warn,
            "pg_connection_failure",
            &[
                field("database", observation.database),
                field("elapsed_ms", duration_ms(observation.elapsed)),
                field("timeout", observation.timeout),
                field("closed", observation.closed),
            ],
        ),
    }
}

struct QueryMeasurement<'a> {
    observe: &'a mut (dyn FnMut(PgObservation) + Send),
    query_name: &'static str,
    database: String,
    started: Instant,
    stats: query::QueryStats,
}

impl QueryMeasurement<'_> {
    const fn stats_mut(&mut self) -> &mut query::QueryStats {
        &mut self.stats
    }

    fn success(self) {
        self.finish(QueryOutcome::Success);
    }

    fn error(self) {
        self.finish(QueryOutcome::Error);
    }

    fn timeout(self) {
        self.finish(QueryOutcome::Timeout);
    }

    fn sink_error(self) {
        self.finish(QueryOutcome::SinkError);
    }

    fn finish(self, outcome: QueryOutcome) {
        (self.observe)(PgObservation::Query(QueryObservation {
            query_name: self.query_name,
            database: self.database,
            elapsed: self.started.elapsed(),
            stats: self.stats,
            outcome,
        }));
    }
}

fn measure<'a>(
    observe: &'a mut (dyn FnMut(PgObservation) + Send),
    query_name: &'static str,
    database: &str,
) -> QueryMeasurement<'a> {
    QueryMeasurement {
        observe,
        query_name,
        database: database.to_owned(),
        started: Instant::now(),
        stats: query::QueryStats::default(),
    }
}

/// One bounded `PostgreSQL` batch retained until it reaches the WAL.
#[derive(Debug)]
pub(crate) enum PgBatch {
    Settings(Arc<[SettingsRow]>),
    Archiver(ArchiverRow),
    Bgwriter(BgwriterSnapshot),
    Checkpointer(CheckpointerSnapshot),
    Wal(WalSnapshot),
    PreparedXacts(Vec<PreparedXactsRow>),
    Database(DatabaseVersion, Vec<DatabaseRow>),
    Io(IoVersion, Vec<IoRow>),
    Activity(ActivityVersion, Vec<ActivityRow>),
    Locks(LocksVersion, Vec<LockRow>),
    ProgressVacuum(Vec<ProgressVacuumRow>),
    Statements(StatementsVersion, Vec<StatementsRow>),
    StatementsInfo(PgStatStatementsInfo),
    StorePlansOssc(Vec<OsscRow>),
    StorePlansVadv(Vec<VadvRow>),
    StorePlansInfo(PgStorePlansInfo),
    UserTables(UserTablesVersion, Vec<UserTablesRow>),
    UserIndexes(UserIndexesVersion, Vec<UserIndexesRow>),
}

#[derive(Debug)]
struct CachedSettings {
    generation: u64,
    rows: Arc<[SettingsRow]>,
}

/// The primary connection, persistent per-database connections, and caches
/// tied to the primary connection generation.
#[derive(Debug)]
pub(crate) struct PgSources {
    server: Option<Pool>,
    databases: BTreeMap<String, Pool>,
    settings: Option<CachedSettings>,
    server_major: Option<(u64, u32)>,
}

impl PgSources {
    /// Take the first configured DSN, or nothing when none is configured.
    pub(crate) fn open(config: &Config) -> Self {
        let Some(dsn) = config.pg_dsns.first() else {
            return Self::disabled();
        };
        if config.pg_dsns.len() > 1 {
            log_event(
                LogLevel::Warn,
                "pg_metrics_single_server",
                &[
                    field("configured", config.pg_dsns.len()),
                    field(
                        "reason",
                        "a metric row does not name its server, so only the first DSN is collected",
                    ),
                ],
            );
        }
        match Pool::new(dsn) {
            Ok(server) => Self {
                server: Some(server),
                databases: BTreeMap::new(),
                settings: None,
                server_major: None,
            },
            Err(_error) => {
                log_event(
                    LogLevel::Warn,
                    "pg_metrics_configuration_invalid",
                    &[
                        field("source_index", 0_usize),
                        field("reason", "invalid_connection_configuration"),
                    ],
                );
                Self::disabled()
            }
        }
    }

    const fn disabled() -> Self {
        Self {
            server: None,
            databases: BTreeMap::new(),
            settings: None,
            server_major: None,
        }
    }

    /// Configuration rows safe to attach to a newly opened segment.
    pub(crate) fn last_settings(&self) -> Option<Arc<[SettingsRow]>> {
        let generation = self.server.as_ref().and_then(Pool::generation)?;
        cached_settings_for_generation(self.settings.as_ref(), generation)
    }

    /// Read due sections and synchronously admit each bounded batch before the
    /// query stream fetches another row.
    pub(crate) async fn collect<E>(
        &mut self,
        due: &DueSet,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        mut admit: impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        let instance = due.has(SourceKind::Pg);
        let relations = due.has(SourceKind::PgRelations);
        if !instance && !relations {
            return Ok(());
        }
        let Some((major, generation)) = self.read_server_major(observe).await else {
            return Ok(());
        };
        if self.settings.as_ref().map(|cached| cached.generation) != Some(generation) {
            self.settings = None;
            let settings_result = match self.server.as_mut() {
                Some(server) => read_settings(server, generation, observe, &mut admit).await,
                None => Ok(None),
            };
            match settings_result {
                Ok(Some(rows)) => {
                    self.settings = Some(CachedSettings { generation, rows });
                }
                Ok(None) => {
                    self.clear_primary_connection();
                    return Ok(());
                }
                Err(error) => {
                    self.clear_primary_cache_if_closed();
                    return Err(error);
                }
            }
        }
        let cached_settings = self.last_settings();
        if instance {
            match self
                .collect_instance(
                    major,
                    generation,
                    observe,
                    cached_settings.as_ref(),
                    &mut admit,
                )
                .await
            {
                Ok(true) => {}
                Ok(false) => {
                    self.clear_primary_connection();
                    return Ok(());
                }
                Err(error) => {
                    self.clear_primary_cache_if_closed();
                    return Err(error);
                }
            }
        }
        if relations {
            self.collect_relations(
                major,
                generation,
                observe,
                cached_settings.as_ref(),
                &mut admit,
            )
            .await?;
        }
        Ok(())
    }

    async fn read_server_major(
        &mut self,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> Option<(u32, u64)> {
        let cached = self.server_major;
        let server = self.server.as_mut()?;
        let database = server.database_label().to_owned();
        let session = match open_session(server, observe).await {
            Ok(session) => session,
            Err(_failure) => {
                self.clear_primary_connection();
                return None;
            }
        };
        let generation = session.generation();
        if let Some((cached_generation, major)) = cached
            && cached_generation == generation
        {
            return Some((major, generation));
        }
        let result = {
            let mut measured = measure(observe, "server_major", &database);
            let result = tokio::time::timeout(
                QUERY_TIMEOUT,
                query::read_simple_i32(session, SERVER_MAJOR_SQL, measured.stats_mut()),
            )
            .await;
            finish_query(measured, result)
        };
        match result {
            Ok(num) => {
                let major = u32::try_from(num).unwrap_or(0) / 10_000;
                self.server_major = Some((generation, major));
                Some((major, generation))
            }
            Err(_failure) => {
                self.clear_primary_connection();
                None
            }
        }
    }

    #[allow(
        clippy::too_many_lines,
        reason = "the sequential pass keeps each query and failure boundary visible"
    )]
    async fn collect_instance<E>(
        &mut self,
        major: u32,
        generation: u64,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let Some(server) = self.server.as_mut() else {
            return Ok(false);
        };
        let database = server.database_label().to_owned();

        let archiver = {
            let session = match session_for_generation(server, generation, observe) {
                Ok(session) => session,
                Err(_failure) => return Ok(false),
            };
            let mut measured = measure(observe, "pg_stat_archiver", &database);
            let result = tokio::time::timeout(
                QUERY_TIMEOUT,
                kronika_source_pg::archiver::collect_archiver(session, measured.stats_mut()),
            )
            .await;
            match result {
                Ok(Ok(row)) => {
                    measured = deliver(
                        measured,
                        admit,
                        PgBatch::Archiver(row),
                        cached_settings.cloned(),
                    )?;
                    measured.success();
                    Some(())
                }
                other => {
                    finish_failed(measured, other);
                    None
                }
            }
        };
        if archiver.is_none() {
            self.clear_primary_connection();
            return Ok(false);
        }

        if wal::wal_version(major).is_some() {
            let result = {
                let session = match session_for_generation(server, generation, observe) {
                    Ok(session) => session,
                    Err(_failure) => return Ok(false),
                };
                let mut measured = measure(observe, "pg_stat_wal", &database);
                let result = tokio::time::timeout(
                    QUERY_TIMEOUT,
                    wal::collect_wal(session, major, measured.stats_mut()),
                )
                .await;
                match result {
                    Ok(Ok(Some(row))) => {
                        measured =
                            deliver(measured, admit, PgBatch::Wal(row), cached_settings.cloned())?;
                        measured.success();
                        true
                    }
                    Ok(Ok(None)) => {
                        measured.success();
                        true
                    }
                    other => {
                        finish_failed(measured, other);
                        false
                    }
                }
            };
            if !result {
                self.clear_primary_connection();
                return Ok(false);
            }
        }

        if !collect_prepared(
            server,
            generation,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_database(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if io::io_version(major).is_some()
            && !collect_io(
                server,
                generation,
                major,
                observe,
                &database,
                cached_settings,
                admit,
            )
            .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_activity(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_progress(
            server,
            generation,
            major,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        if !collect_extensions(
            server,
            generation,
            observe,
            &database,
            cached_settings,
            admit,
        )
        .await?
        {
            self.clear_primary_connection();
            return Ok(false);
        }
        Ok(true)
    }

    async fn collect_relations<E>(
        &mut self,
        major: u32,
        generation: u64,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        let found = {
            let Some(server) = self.server.as_mut() else {
                return Ok(());
            };
            let database = server.database_label().to_owned();
            let session = match session_for_generation(server, generation, observe) {
                Ok(session) => session,
                Err(_failure) => {
                    self.clear_primary_connection();
                    return Ok(());
                }
            };
            let mut measured = measure(observe, "databases", &database);
            let result = tokio::time::timeout(
                QUERY_TIMEOUT,
                databases::enumerate(session, measured.stats_mut()),
            )
            .await;
            match finish_query(measured, result) {
                Ok(found) => found,
                Err(_failure) => {
                    self.clear_primary_connection();
                    return Ok(());
                }
            }
        };
        let Some(server) = self.server.as_ref() else {
            return Ok(());
        };
        databases::refresh(&mut self.databases, &found, server);

        for database in &found {
            let Some(pool) = self.databases.get_mut(&database.name) else {
                continue;
            };
            let result =
                collect_relation_database(pool, database, major, observe, cached_settings, admit)
                    .await;
            match result {
                Err(error) => {
                    return Err(error);
                }
                Ok(RelationResult::Complete) => {}
                Ok(RelationResult::Failed) => pool.close(),
                Ok(RelationResult::TimedOut) => {
                    pool.close();
                    break;
                }
            }
        }
        Ok(())
    }

    fn clear_primary_connection(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.close();
        }
        self.settings = None;
        self.server_major = None;
    }

    fn clear_primary_cache_if_closed(&mut self) {
        if self.server.as_ref().and_then(Pool::generation).is_none() {
            self.settings = None;
            self.server_major = None;
        }
    }
}

fn cached_settings_for_generation(
    settings: Option<&CachedSettings>,
    generation: u64,
) -> Option<Arc<[SettingsRow]>> {
    settings
        .filter(|cached| cached.generation == generation)
        .map(|cached| Arc::clone(&cached.rows))
}

async fn read_settings<E>(
    server: &mut Pool,
    generation: u64,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<Option<Arc<[SettingsRow]>>, E> {
    let database = server.database_label().to_owned();
    let session = match session_for_generation(server, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(None),
    };
    let mut measured = measure(observe, "pg_settings", &database);
    let result = tokio::time::timeout(
        QUERY_TIMEOUT,
        settings::collect(session, measured.stats_mut()),
    )
    .await;
    match result {
        Ok(Ok(rows)) => {
            let rows: Arc<[SettingsRow]> = Arc::from(rows);
            measured = deliver(measured, admit, PgBatch::Settings(Arc::clone(&rows)), None)?;
            measured.success();
            Ok(Some(rows))
        }
        other => {
            finish_failed(measured, other);
            Ok(None)
        }
    }
}

async fn collect_prepared<E>(
    pool: &mut Pool,
    generation: u64,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_prepared_xacts", database);
    let result = prepared_xacts::collect_prepared_xacts(session, measured.stats_mut(), |batch| {
        admit(PgBatch::PreparedXacts(batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_database<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database_label: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let version = database::database_version(major);
    let mut measured = measure(observe, "pg_stat_database", database_label);
    let result = database::collect_database(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Database(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_io<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let Some(version) = io::io_version(major) else {
        return Ok(true);
    };
    let mut measured = measure(observe, "pg_stat_io", database);
    let result = io::collect_io(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Io(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_activity<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let version = activity::activity_version(major);
    let mut measured = measure(observe, "pg_stat_activity", database);
    let result = activity::collect_activity(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Activity(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
}

async fn collect_progress<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_stat_progress_vacuum", database);
    let result =
        progress_vacuum::collect_progress_vacuum(session, major, measured.stats_mut(), |batch| {
            admit(PgBatch::ProgressVacuum(batch.rows), settings.cloned())
        })
        .await;
    finish_batched(pool, measured, result)
}

async fn collect_extensions<E>(
    pool: &mut Pool,
    generation: u64,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let statements_version = {
        let session = match session_for_generation(pool, generation, observe) {
            Ok(session) => session,
            Err(_failure) => return Ok(false),
        };
        let mut measured = measure(observe, "pg_stat_statements_extension", database);
        let result = tokio::time::timeout(
            QUERY_TIMEOUT,
            extension::installed(session, statements::EXTENSION, measured.stats_mut()),
        )
        .await;
        match finish_query(measured, result) {
            Ok(version) => version.and_then(statements::statements_version),
            Err(_failure) => return Ok(false),
        }
    };
    if let Some(version) = statements_version {
        let session = match session_for_generation(pool, generation, observe) {
            Ok(session) => session,
            Err(_failure) => return Ok(false),
        };
        let mut measured = measure(observe, "pg_stat_statements", database);
        let result =
            statements::collect_statements(session, version, measured.stats_mut(), |batch| {
                admit(PgBatch::Statements(version, batch.rows), settings.cloned())
            })
            .await;
        if !finish_batched(pool, measured, result)? {
            return Ok(false);
        }
    }

    let plans_flavour = {
        let session = match session_for_generation(pool, generation, observe) {
            Ok(session) => session,
            Err(_failure) => return Ok(false),
        };
        let mut measured = measure(observe, "pg_store_plans_extension", database);
        let result = tokio::time::timeout(
            QUERY_TIMEOUT,
            extension::installed(session, store_plans::EXTENSION, measured.stats_mut()),
        )
        .await;
        match finish_query(measured, result) {
            Ok(version) => version.and_then(store_plans::flavour),
            Err(_failure) => return Ok(false),
        }
    };
    match plans_flavour {
        Some(Flavour::Ossc) => {
            let session = match session_for_generation(pool, generation, observe) {
                Ok(session) => session,
                Err(_failure) => return Ok(false),
            };
            let mut measured = measure(observe, "pg_store_plans_ossc", database);
            let result = store_plans::collect_ossc(
                session,
                store_plans::TOP_N,
                store_plans::PLAN_TEXT_BUDGET,
                measured.stats_mut(),
                |batch| admit(PgBatch::StorePlansOssc(batch.rows), settings.cloned()),
            )
            .await;
            finish_batched(pool, measured, result)
        }
        Some(Flavour::Vadv) => {
            let session = match session_for_generation(pool, generation, observe) {
                Ok(session) => session,
                Err(_failure) => return Ok(false),
            };
            let mut measured = measure(observe, "pg_store_plans_vadv", database);
            let result = store_plans::collect_vadv(
                session,
                store_plans::TOP_N,
                store_plans::PLAN_TEXT_BUDGET,
                measured.stats_mut(),
                |batch| admit(PgBatch::StorePlansVadv(batch.rows), settings.cloned()),
            )
            .await;
            finish_batched(pool, measured, result)
        }
        None => Ok(true),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelationResult {
    Complete,
    Failed,
    TimedOut,
}

async fn collect_relation_database<E>(
    pool: &mut Pool,
    database: &databases::Database,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<RelationResult, E> {
    let session = match open_session(pool, observe).await {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Error) => return Ok(RelationResult::Failed),
    };
    let generation = session.generation();
    let table_version = user_tables::user_tables_version(major);
    let mut measured = measure(observe, "pg_stat_user_tables", &database.name);
    let result =
        user_tables::collect_user_tables(session, database, major, measured.stats_mut(), |batch| {
            admit(
                PgBatch::UserTables(table_version, batch.rows),
                settings.cloned(),
            )
        })
        .await;
    match finish_batched_kind(pool, measured, result)? {
        QueryCompletion::Complete => {}
        QueryCompletion::Failed => return Ok(RelationResult::Failed),
        QueryCompletion::TimedOut => return Ok(RelationResult::TimedOut),
    }

    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Error) => return Ok(RelationResult::Failed),
    };
    let index_version = user_indexes::user_indexes_version(major);
    let mut measured = measure(observe, "pg_stat_user_indexes", &database.name);
    let result = user_indexes::collect_user_indexes(
        session,
        database,
        major,
        measured.stats_mut(),
        |batch| {
            admit(
                PgBatch::UserIndexes(index_version, batch.rows),
                settings.cloned(),
            )
        },
    )
    .await;
    Ok(match finish_batched_kind(pool, measured, result)? {
        QueryCompletion::Complete => RelationResult::Complete,
        QueryCompletion::Failed => RelationResult::Failed,
        QueryCompletion::TimedOut => RelationResult::TimedOut,
    })
}

fn deliver<'a, E>(
    mut measured: QueryMeasurement<'a>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    batch: PgBatch,
    settings: Option<Arc<[SettingsRow]>>,
) -> Result<QueryMeasurement<'a>, E> {
    let started = Instant::now();
    match admit(batch, settings) {
        Ok(write) => {
            measured
                .stats_mut()
                .record_batch_write(started.elapsed(), write);
            Ok(measured)
        }
        Err(error) => {
            measured.stats_mut().record_failed_batch(started.elapsed());
            measured.sink_error();
            Err(error)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryFailure {
    Error,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryCompletion {
    Complete,
    Failed,
    TimedOut,
}

async fn open_session<'a>(
    pool: &'a mut Pool,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<Session<'a>, QueryFailure> {
    let database = pool.database_label().to_owned();
    let started = Instant::now();
    match pool.session().await {
        Ok(session) => Ok(session),
        Err(error) => {
            let timeout = error.is_timeout();
            observe(PgObservation::Connection(ConnectionObservation {
                database,
                elapsed: started.elapsed(),
                timeout,
                closed: false,
            }));
            if timeout {
                Err(QueryFailure::Timeout)
            } else {
                Err(QueryFailure::Error)
            }
        }
    }
}

fn session_for_generation<'a>(
    pool: &'a mut Pool,
    expected: u64,
    observe: &mut (dyn FnMut(PgObservation) + Send),
) -> Result<Session<'a>, QueryFailure> {
    let database = pool.database_label().to_owned();
    let Some(session) = pool.session_for_generation(expected) else {
        observe(PgObservation::Connection(ConnectionObservation {
            database,
            elapsed: Duration::ZERO,
            timeout: false,
            closed: true,
        }));
        return Err(QueryFailure::Error);
    };
    Ok(session)
}

fn finish_query<T, E>(
    measured: QueryMeasurement<'_>,
    result: Result<Result<T, E>, tokio::time::error::Elapsed>,
) -> Result<T, QueryFailure> {
    match result {
        Ok(Ok(value)) => {
            measured.success();
            Ok(value)
        }
        Ok(Err(_error)) => {
            measured.error();
            Err(QueryFailure::Error)
        }
        Err(_elapsed) => {
            measured.timeout();
            Err(QueryFailure::Timeout)
        }
    }
}

fn finish_failed<T, E>(
    measured: QueryMeasurement<'_>,
    result: Result<Result<T, E>, tokio::time::error::Elapsed>,
) {
    match result {
        Ok(Ok(_value)) => measured.success(),
        Ok(Err(_error)) => measured.error(),
        Err(_elapsed) => measured.timeout(),
    }
}

fn finish_batched<E>(
    pool: &mut Pool,
    measured: QueryMeasurement<'_>,
    result: Result<(), BatchError<E>>,
) -> Result<bool, E> {
    Ok(matches!(
        finish_batched_kind(pool, measured, result)?,
        QueryCompletion::Complete
    ))
}

fn finish_batched_kind<E>(
    pool: &mut Pool,
    measured: QueryMeasurement<'_>,
    result: Result<(), BatchError<E>>,
) -> Result<QueryCompletion, E> {
    match result {
        Ok(()) => {
            measured.success();
            Ok(QueryCompletion::Complete)
        }
        Err(BatchError::PostgreSql(_error)) => {
            measured.error();
            pool.close();
            Ok(QueryCompletion::Failed)
        }
        Err(BatchError::Sink(error)) => {
            measured.sink_error();
            // Dropping an unconsumed RowStream leaves a response in flight.
            // Closing prevents a later query from being written behind it.
            pool.close();
            Err(error)
        }
        Err(BatchError::Timeout) => {
            measured.timeout();
            pool.close();
            Ok(QueryCompletion::TimedOut)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::{
        CachedSettings, PgObservation, QueryFailure, QueryOutcome, cached_settings_for_generation,
        measure, session_for_generation,
    };
    use kronika_source_pg::Pool;

    #[test]
    fn settings_cache_is_scoped_to_one_connection_generation() {
        let cached = Some(CachedSettings {
            generation: 7,
            rows: Arc::from([]),
        });
        assert!(cached_settings_for_generation(cached.as_ref(), 7).is_some());
        assert!(cached_settings_for_generation(cached.as_ref(), 8).is_none());
    }

    #[test]
    fn timeout_observation_retains_partial_query_accounting() {
        let mut observations = Vec::new();
        let mut observe = |observation| observations.push(observation);
        let mut measured = measure(&mut observe, "probe", "postgres");
        measured.stats_mut().rows = 9;
        measured.timeout();

        let PgObservation::Query(observation) = observations.pop().expect("one observation") else {
            panic!("expected a query observation");
        };
        assert_eq!(observation.outcome, QueryOutcome::Timeout);
        assert_eq!(observation.stats.rows, 9);
    }

    #[test]
    fn unavailable_session_is_accounted_as_a_closed_connection() {
        let mut pool = Pool::new("host=127.0.0.1 dbname=metrics")
            .expect("syntactically valid connection settings");
        let mut observations = Vec::new();
        {
            let mut observe = |observation| observations.push(observation);
            assert!(matches!(
                session_for_generation(&mut pool, 1, &mut observe),
                Err(QueryFailure::Error)
            ));
        }

        let PgObservation::Connection(observation) =
            observations.pop().expect("one connection observation")
        else {
            panic!("expected a connection observation");
        };
        assert_eq!(observation.database, "metrics");
        assert!(observation.closed);
        assert!(!observation.timeout);
    }
}
