//! Sequential `PostgreSQL` statistics collection.
//!
//! Every database keeps one healthy connection across collection cycles. A
//! failed or timed-out connection is closed and reopened on a later cycle.
//! Queries on every connection are awaited one at a time; no pipeline or
//! concurrent query task is created.

mod buffering;
pub(crate) mod telemetry;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::Context as _;
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
use crate::logging::{LogLevel, field, log_event};
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_pg_batch;

const SERVER_PROBE_SQL: &str = concat!(
    "/* kronika:",
    env!("CARGO_PKG_VERSION"),
    " bins/kronika-collector/src/pg_sources.rs */ ",
    "SELECT current_setting('server_version_num'), session_user::text, ",
    "current_database()::text, pg_catalog.pg_has_role('pg_read_all_stats', 'member')"
);
const QUERY_TIMEOUT: Duration = Duration::from_secs(30);
const DISCOVERY_INTERVAL: Duration = Duration::from_mins(5);

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
    pub(crate) connection: String,
    pub(crate) database: String,
    pub(crate) elapsed: Duration,
    pub(crate) stats: query::QueryStats,
    pub(crate) outcome: QueryOutcome,
    pub(crate) error: Option<String>,
}

/// One failed connection attempt.
#[derive(Debug)]
pub(crate) struct ConnectionObservation {
    pub(crate) connection: String,
    pub(crate) database: String,
    pub(crate) elapsed: Duration,
    pub(crate) timeout: bool,
    pub(crate) closed: bool,
    pub(crate) error: String,
}

/// Stable query completion classification.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum QueryOutcome {
    Success,
    Error,
    Timeout,
    SinkError,
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
        self.finish(QueryOutcome::Success, None);
    }

    fn error(self, error: &(dyn std::fmt::Display + '_)) {
        self.finish(QueryOutcome::Error, Some(error.to_string()));
    }

    fn timeout(self) {
        self.finish(
            QueryOutcome::Timeout,
            Some(format!(
                "query timed out after {} seconds",
                QUERY_TIMEOUT.as_secs()
            )),
        );
    }

    fn sink_error(self) {
        self.finish(
            QueryOutcome::SinkError,
            Some("write query batch to the journal failed".to_owned()),
        );
    }

    fn finish(self, outcome: QueryOutcome, error: Option<String>) {
        (self.observe)(PgObservation::Query(QueryObservation {
            query_name: self.query_name,
            connection: self.database.clone(),
            database: self.database,
            elapsed: self.started.elapsed(),
            stats: self.stats,
            outcome,
            error,
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

#[derive(Debug, Clone)]
struct GenerationProbe {
    generation: u64,
    major: u32,
    user: String,
    database: String,
    full_visibility: bool,
}

#[derive(Debug, Clone, Default)]
struct DatabaseCapabilities {
    statements: Option<statements::StatementsCapability>,
    store_plans: Option<store_plans::StorePlansCapability>,
    statements_info: Option<extension::ExtensionSchema>,
    store_plans_info: Option<extension::ExtensionSchema>,
}

/// The primary connection, persistent per-database connections, and caches
/// tied to the primary connection generation.
#[derive(Debug)]
pub(crate) struct PgSources {
    server: Option<Pool>,
    server_database: Option<String>,
    databases: BTreeMap<String, Pool>,
    discovered: Vec<databases::Database>,
    capabilities: BTreeMap<String, DatabaseCapabilities>,
    last_discovery: Option<Instant>,
    settings: Option<CachedSettings>,
    probe: Option<GenerationProbe>,
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
                server_database: None,
                databases: BTreeMap::new(),
                discovered: Vec::new(),
                capabilities: BTreeMap::new(),
                last_discovery: None,
                settings: None,
                probe: None,
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
            server_database: None,
            databases: BTreeMap::new(),
            discovered: Vec::new(),
            capabilities: BTreeMap::new(),
            last_discovery: None,
            settings: None,
            probe: None,
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
        let Some(probe) = self.read_probe(observe).await else {
            return Ok(());
        };
        let now = Instant::now();
        if discovery_due(self.last_discovery, now, due.forced()) {
            self.discover(&probe, now, observe).await;
        }
        if instance {
            let current = self.settings.as_ref().map(|cached| cached.rows.as_ref());
            let settings_result = match self.server.as_mut() {
                Some(server) => {
                    read_settings(server, probe.generation, current, observe, &mut admit).await
                }
                None => Ok(None),
            };
            match settings_result {
                Ok(Some(rows)) => {
                    self.settings = Some(CachedSettings {
                        generation: probe.generation,
                        rows,
                    });
                }
                Ok(None) => {}
                Err(error) => return Err(error),
            }
        }
        let cached_settings = self.last_settings();
        if instance {
            match self
                .collect_instance(&probe, observe, cached_settings.as_ref(), &mut admit)
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
            self.collect_relations(&probe, observe, cached_settings.as_ref(), &mut admit)
                .await?;
        }
        Ok(())
    }

    async fn read_probe(
        &mut self,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) -> Option<GenerationProbe> {
        let cached = self.probe.clone();
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
        if let Some(cached) = cached
            && cached.generation == generation
        {
            return Some(cached);
        }
        let result = {
            let mut measured = measure(observe, "server_probe", &database);
            let result = tokio::time::timeout(
                QUERY_TIMEOUT,
                read_generation_probe(session, generation, measured.stats_mut()),
            )
            .await;
            finish_query(measured, result)
        };
        match result {
            Ok(probe) => {
                self.settings = None;
                self.capabilities.clear();
                self.discovered.clear();
                self.last_discovery = None;
                self.server_database = Some(probe.database.clone());
                self.probe = Some(probe.clone());
                Some(probe)
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
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<bool, E> {
        let Some(server) = self.server.as_mut() else {
            return Ok(false);
        };
        let database = probe.database.clone();
        let major = probe.major;
        let generation = probe.generation;

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
                other => match finish_failed(measured, other) {
                    QueryCompletion::SourceFailed => Some(()),
                    QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => None,
                    QueryCompletion::Complete => unreachable!("success handled above"),
                },
            }
        };
        if archiver.is_none() {
            self.clear_primary_connection();
            return Ok(false);
        }

        if !collect_bgwriter(
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
        if !collect_checkpointer(
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
                    other => matches!(
                        finish_failed(measured, other),
                        QueryCompletion::SourceFailed
                    ),
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
        if probe.full_visibility {
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
                || !collect_locks(
                    server,
                    generation,
                    major,
                    observe,
                    &database,
                    cached_settings,
                    admit,
                )
                .await?
                || !collect_progress(
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
        } else {
            log_event(
                LogLevel::Warn,
                "pg_stats_visibility_required",
                &[
                    field("database", &database),
                    field("reason", "pg_read_all_stats_required"),
                ],
            );
        }
        let _ = server;
        self.collect_extensions_dynamic(probe, observe, cached_settings, admit)
            .await?;
        Ok(true)
    }

    async fn collect_extensions_dynamic<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        if let Some((database, capability)) =
            selected_statements(&self.capabilities, self.server_database.as_deref())
        {
            let info = self
                .capabilities
                .get(&database)
                .and_then(|entry| entry.statements_info.clone());
            let is_current = self.server_database.as_deref() == Some(&database);
            let completion = {
                let Some(pool) = self.database_pool_mut(&database) else {
                    self.invalidate_statements(&database);
                    return Ok(());
                };
                let session = if is_current {
                    session_for_generation(pool, probe.generation, observe)
                } else {
                    open_session(pool, observe).await
                };
                let session = match session {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        self.invalidate_statements(&database);
                        return Ok(());
                    }
                    Err(QueryFailure::Source) => return Ok(()),
                };
                let version = capability.version;
                let mut measured = measure(observe, "pg_stat_statements", &database);
                let result = statements::collect_statements(
                    session,
                    &capability,
                    measured.stats_mut(),
                    |batch| admit(PgBatch::Statements(version, batch.rows), settings.cloned()),
                )
                .await;
                finish_batched_kind(pool, measured, result)?
            };
            match completion {
                QueryCompletion::Complete | QueryCompletion::SourceFailed => {}
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
                    self.invalidate_statements(&database);
                    return Ok(());
                }
            }
            if let Some(schema) = info {
                let is_current = self.server_database.as_deref() == Some(&database);
                let completion = {
                    let Some(pool) = self.database_pool_mut(&database) else {
                        self.invalidate_statements(&database);
                        return Ok(());
                    };
                    let session = if is_current {
                        session_for_generation(pool, probe.generation, observe)
                    } else {
                        open_session(pool, observe).await
                    };
                    let session = match session {
                        Ok(session) => session,
                        Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                            pool.close();
                            self.invalidate_statements(&database);
                            return Ok(());
                        }
                        Err(QueryFailure::Source) => return Ok(()),
                    };
                    let mut measured = measure(observe, "pg_stat_statements_info", &database);
                    let result = tokio::time::timeout(
                        QUERY_TIMEOUT,
                        statements_info::collect(
                            session,
                            &schema.qualify("pg_stat_statements_info"),
                            measured.stats_mut(),
                        ),
                    )
                    .await;
                    match result {
                        Ok(Ok(row)) => {
                            measured = deliver(
                                measured,
                                admit,
                                PgBatch::StatementsInfo(row),
                                settings.cloned(),
                            )?;
                            measured.success();
                            QueryCompletion::Complete
                        }
                        other => finish_failed(measured, other),
                    }
                };
                if matches!(
                    completion,
                    QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut
                ) {
                    self.invalidate_statements(&database);
                }
            }
        }

        if let Some((database, capability)) =
            selected_store_plans(&self.capabilities, self.server_database.as_deref())
        {
            let info = self
                .capabilities
                .get(&database)
                .and_then(|entry| entry.store_plans_info.clone());
            let is_current = self.server_database.as_deref() == Some(&database);
            let completion = {
                let Some(pool) = self.database_pool_mut(&database) else {
                    self.invalidate_store_plans(&database);
                    return Ok(());
                };
                let session = if is_current {
                    session_for_generation(pool, probe.generation, observe)
                } else {
                    open_session(pool, observe).await
                };
                let session = match session {
                    Ok(session) => session,
                    Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                        pool.close();
                        self.invalidate_store_plans(&database);
                        return Ok(());
                    }
                    Err(QueryFailure::Source) => return Ok(()),
                };
                let mut measured = measure(observe, "pg_store_plans", &database);
                let result = match capability.flavour {
                    Flavour::OsscCompatible => {
                        store_plans::collect_ossc(
                            session,
                            &capability,
                            measured.stats_mut(),
                            |batch| admit(PgBatch::StorePlansOssc(batch.rows), settings.cloned()),
                        )
                        .await
                    }
                    Flavour::Vadv => {
                        store_plans::collect_vadv(
                            session,
                            &capability,
                            measured.stats_mut(),
                            |batch| admit(PgBatch::StorePlansVadv(batch.rows), settings.cloned()),
                        )
                        .await
                    }
                };
                finish_batched_kind(pool, measured, result)?
            };
            match completion {
                QueryCompletion::Complete | QueryCompletion::SourceFailed => {}
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut => {
                    self.invalidate_store_plans(&database);
                    return Ok(());
                }
            }
            if let Some(schema) = info {
                let is_current = self.server_database.as_deref() == Some(&database);
                let completion = {
                    let Some(pool) = self.database_pool_mut(&database) else {
                        self.invalidate_store_plans(&database);
                        return Ok(());
                    };
                    let session = if is_current {
                        session_for_generation(pool, probe.generation, observe)
                    } else {
                        open_session(pool, observe).await
                    };
                    let session = match session {
                        Ok(session) => session,
                        Err(QueryFailure::Timeout | QueryFailure::Connection) => {
                            pool.close();
                            self.invalidate_store_plans(&database);
                            return Ok(());
                        }
                        Err(QueryFailure::Source) => return Ok(()),
                    };
                    let mut measured = measure(observe, "pg_store_plans_info", &database);
                    let result = tokio::time::timeout(
                        QUERY_TIMEOUT,
                        store_plans_info::collect(
                            session,
                            &schema.qualify("pg_store_plans_info"),
                            measured.stats_mut(),
                        ),
                    )
                    .await;
                    match result {
                        Ok(Ok(row)) => {
                            measured = deliver(
                                measured,
                                admit,
                                PgBatch::StorePlansInfo(row),
                                settings.cloned(),
                            )?;
                            measured.success();
                            QueryCompletion::Complete
                        }
                        other => finish_failed(measured, other),
                    }
                };
                if matches!(
                    completion,
                    QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut
                ) {
                    self.invalidate_store_plans(&database);
                }
            }
        }
        Ok(())
    }

    async fn collect_relations<E>(
        &mut self,
        probe: &GenerationProbe,
        observe: &mut (dyn FnMut(PgObservation) + Send),
        cached_settings: Option<&Arc<[SettingsRow]>>,
        admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
    ) -> Result<(), E> {
        let found = self.discovered.clone();
        for database in &found {
            let Some(pool) = self.database_pool_mut(&database.name) else {
                continue;
            };
            let result = collect_relation_database(
                pool,
                database,
                probe.major,
                observe,
                cached_settings,
                admit,
            )
            .await;
            match result {
                Err(error) => {
                    return Err(error);
                }
                Ok(RelationResult::Complete) => {}
                Ok(RelationResult::SourceFailed) => {}
                Ok(RelationResult::ConnectionFailed) => pool.close(),
                Ok(RelationResult::TimedOut) => {
                    pool.close();
                }
            }
        }
        Ok(())
    }

    /// Refresh the connectable database list and one capability map entry per
    /// database. A failed inventory never leaves a stale capability behind.
    async fn discover(
        &mut self,
        probe: &GenerationProbe,
        now: Instant,
        observe: &mut (dyn FnMut(PgObservation) + Send),
    ) {
        self.last_discovery = Some(now);
        let found = {
            let Some(server) = self.server.as_mut() else {
                return;
            };
            let session = match session_for_generation(server, probe.generation, observe) {
                Ok(session) => session,
                Err(_failure) => {
                    self.clear_primary_connection();
                    return;
                }
            };
            let mut measured = measure(observe, "databases", &probe.database);
            let result = tokio::time::timeout(
                QUERY_TIMEOUT,
                databases::enumerate(session, measured.stats_mut()),
            )
            .await;
            match finish_query(measured, result) {
                Ok(found) => found,
                Err(QueryFailure::Timeout) => {
                    self.clear_primary_connection();
                    return;
                }
                Err(QueryFailure::Source | QueryFailure::Connection) => {
                    self.discovered.clear();
                    self.capabilities.clear();
                    self.databases.clear();
                    return;
                }
            }
        };
        self.server_database = found
            .iter()
            .find(|database| database.is_current)
            .map(|database| database.name.clone());
        if let Some(server) = self.server.as_ref() {
            databases::refresh(&mut self.databases, &found, server);
        }

        let mut capabilities = BTreeMap::new();
        for database in &found {
            let Some(pool) = self.database_pool_mut(&database.name) else {
                continue;
            };
            let session = if database.is_current {
                session_for_generation(pool, probe.generation, observe)
            } else {
                open_session(pool, observe).await
            };
            let session = match session {
                Ok(session) => session,
                Err(QueryFailure::Timeout) => {
                    pool.close();
                    continue;
                }
                Err(QueryFailure::Source | QueryFailure::Connection) => continue,
            };
            let mut measured = measure(observe, "extension_inventory", &database.name);
            let result = tokio::time::timeout(
                QUERY_TIMEOUT,
                extension::inventory(session, measured.stats_mut()),
            )
            .await;
            match finish_query(measured, result) {
                Ok(inventory) => {
                    capabilities.insert(
                        database.name.clone(),
                        capabilities_from_inventory(&inventory),
                    );
                }
                Err(QueryFailure::Timeout) => pool.close(),
                Err(QueryFailure::Source | QueryFailure::Connection) => {}
            }
        }
        self.discovered = found;
        self.capabilities = capabilities;
    }

    fn database_pool_mut(&mut self, database: &str) -> Option<&mut Pool> {
        if self.server_database.as_deref() == Some(database) {
            self.server.as_mut()
        } else {
            self.databases.get_mut(database)
        }
    }

    fn invalidate_statements(&mut self, database: &str) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            capability.statements = None;
            capability.statements_info = None;
        }
    }

    fn invalidate_store_plans(&mut self, database: &str) {
        if let Some(capability) = self.capabilities.get_mut(database) {
            capability.store_plans = None;
            capability.store_plans_info = None;
        }
    }

    fn clear_primary_connection(&mut self) {
        if let Some(server) = self.server.as_mut() {
            server.close();
        }
        self.settings = None;
        self.probe = None;
        self.server_database = None;
        self.capabilities.clear();
        self.discovered.clear();
        self.last_discovery = None;
    }

    fn clear_primary_cache_if_closed(&mut self) {
        if self.server.as_ref().and_then(Pool::generation).is_none() {
            self.settings = None;
            self.probe = None;
        }
    }
}

async fn read_generation_probe(
    session: Session<'_>,
    generation: u64,
    stats: &mut query::QueryStats,
) -> anyhow::Result<GenerationProbe> {
    let mut rows = query::read_simple_rows(session, SERVER_PROBE_SQL, stats, |row| {
        let version = row
            .get(0)
            .context("server probe omitted server_version_num")?;
        let user = row.get(1).context("server probe omitted session_user")?;
        let database = row
            .get(2)
            .context("server probe omitted current_database")?;
        let visibility = row
            .get(3)
            .context("server probe omitted pg_read_all_stats membership")?;
        let version = version
            .parse::<u32>()
            .context("parse server_version_num from server probe")?;
        let full_visibility = match visibility {
            "t" => true,
            "f" => false,
            other => anyhow::bail!("server probe returned {other:?} for visibility"),
        };
        Ok(GenerationProbe {
            generation,
            major: version / 10_000,
            user: user.to_owned(),
            database: database.to_owned(),
            full_visibility,
        })
    })
    .await?;
    anyhow::ensure!(rows.len() == 1, "server probe returned {} rows", rows.len());
    Ok(rows.remove(0))
}

fn discovery_due(last: Option<Instant>, now: Instant, forced: bool) -> bool {
    forced || last.is_none_or(|last| now.saturating_duration_since(last) >= DISCOVERY_INTERVAL)
}

fn capabilities_from_inventory(inventory: &[extension::InventoryEntry]) -> DatabaseCapabilities {
    let statements_entry = inventory
        .iter()
        .find(|entry| entry.name == statements::EXTENSION);
    let store_plans_entry = inventory
        .iter()
        .find(|entry| entry.name == store_plans::EXTENSION);
    DatabaseCapabilities {
        statements: statements_entry.and_then(statements::capability),
        store_plans: store_plans_entry.and_then(store_plans::capability),
        statements_info: statements_entry
            .filter(|entry| entry.schema_usable && entry.statements_info)
            .map(|entry| entry.schema.clone()),
        store_plans_info: store_plans_entry
            .filter(|entry| entry.schema_usable && entry.store_plans_info)
            .map(|entry| entry.schema.clone()),
    }
}

fn selected_capability<T: Clone>(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
    get: impl Fn(&DatabaseCapabilities) -> Option<&T>,
) -> Option<(String, T)> {
    if let Some(current) = current_database
        && let Some(capability) = capabilities.get(current).and_then(&get)
    {
        return Some((current.to_owned(), capability.clone()));
    }
    capabilities.iter().find_map(|(database, capabilities)| {
        get(capabilities).map(|capability| (database.clone(), capability.clone()))
    })
}

fn selected_statements(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
) -> Option<(String, statements::StatementsCapability)> {
    selected_capability(capabilities, current_database, |entry| {
        entry.statements.as_ref()
    })
}

fn selected_store_plans(
    capabilities: &BTreeMap<String, DatabaseCapabilities>,
    current_database: Option<&str>,
) -> Option<(String, store_plans::StorePlansCapability)> {
    selected_capability(capabilities, current_database, |entry| {
        entry.store_plans.as_ref()
    })
}

fn settings_equal_ignoring_ts(left: &[SettingsRow], right: &[SettingsRow]) -> bool {
    left.len() == right.len()
        && left.iter().zip(right).all(|(left, right)| {
            let mut normalized = left.clone();
            normalized.ts = right.ts;
            &normalized == right
        })
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
    cached: Option<&[SettingsRow]>,
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
            if cached.is_none_or(|cached| !settings_equal_ignoring_ts(cached, &rows)) {
                measured = deliver(measured, admit, PgBatch::Settings(Arc::clone(&rows)), None)?;
            }
            measured.success();
            Ok(Some(rows))
        }
        other => {
            if matches!(
                finish_failed(measured, other),
                QueryCompletion::ConnectionFailed | QueryCompletion::TimedOut
            ) {
                server.close();
            }
            Ok(None)
        }
    }
}

async fn collect_bgwriter<E>(
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
    let mut measured = measure(observe, "pg_stat_bgwriter", database);
    let result = tokio::time::timeout(
        QUERY_TIMEOUT,
        bgwriter::collect_bgwriter(session, major, measured.stats_mut()),
    )
    .await;
    match result {
        Ok(Ok(row)) => {
            measured = deliver(measured, admit, PgBatch::Bgwriter(row), settings.cloned())?;
            measured.success();
            Ok(true)
        }
        other => Ok(matches!(
            finish_failed(measured, other),
            QueryCompletion::SourceFailed
        )),
    }
}

async fn collect_checkpointer<E>(
    pool: &mut Pool,
    generation: u64,
    major: u32,
    observe: &mut (dyn FnMut(PgObservation) + Send),
    database: &str,
    settings: Option<&Arc<[SettingsRow]>>,
    admit: &mut impl FnMut(PgBatch, Option<Arc<[SettingsRow]>>) -> Result<BatchWrite, E>,
) -> Result<bool, E> {
    let Some(_version) = checkpointer::checkpointer_version(major) else {
        return Ok(true);
    };
    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(_failure) => return Ok(false),
    };
    let mut measured = measure(observe, "pg_stat_checkpointer", database);
    let result = tokio::time::timeout(
        QUERY_TIMEOUT,
        checkpointer::collect_checkpointer(session, major, measured.stats_mut()),
    )
    .await;
    match result {
        Ok(Ok(Some(row))) => {
            measured = deliver(
                measured,
                admit,
                PgBatch::Checkpointer(row),
                settings.cloned(),
            )?;
            measured.success();
            Ok(true)
        }
        Ok(Ok(None)) => {
            measured.success();
            Ok(true)
        }
        other => Ok(matches!(
            finish_failed(measured, other),
            QueryCompletion::SourceFailed
        )),
    }
}

async fn collect_locks<E>(
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
    let version = locks::locks_version(major);
    let mut measured = measure(observe, "pg_locks", database);
    let result = locks::collect_locks(session, major, measured.stats_mut(), |batch| {
        admit(PgBatch::Locks(version, batch.rows), settings.cloned())
    })
    .await;
    finish_batched(pool, measured, result)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelationResult {
    Complete,
    SourceFailed,
    ConnectionFailed,
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
        Err(QueryFailure::Source) => return Ok(RelationResult::SourceFailed),
        Err(QueryFailure::Connection) => return Ok(RelationResult::ConnectionFailed),
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
        QueryCompletion::SourceFailed => return Ok(RelationResult::SourceFailed),
        QueryCompletion::ConnectionFailed => return Ok(RelationResult::ConnectionFailed),
        QueryCompletion::TimedOut => return Ok(RelationResult::TimedOut),
    }

    let session = match session_for_generation(pool, generation, observe) {
        Ok(session) => session,
        Err(QueryFailure::Timeout) => return Ok(RelationResult::TimedOut),
        Err(QueryFailure::Source) => return Ok(RelationResult::SourceFailed),
        Err(QueryFailure::Connection) => return Ok(RelationResult::ConnectionFailed),
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
        QueryCompletion::SourceFailed => RelationResult::SourceFailed,
        QueryCompletion::ConnectionFailed => RelationResult::ConnectionFailed,
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
    Source,
    Connection,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QueryCompletion {
    Complete,
    SourceFailed,
    ConnectionFailed,
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
                connection: database.clone(),
                database,
                elapsed: started.elapsed(),
                timeout,
                closed: false,
                error: error.to_string(),
            }));
            if timeout {
                Err(QueryFailure::Timeout)
            } else {
                Err(QueryFailure::Connection)
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
            connection: database.clone(),
            database,
            elapsed: Duration::ZERO,
            timeout: false,
            closed: true,
            error: "connection closed before the query started".to_owned(),
        }));
        return Err(QueryFailure::Connection);
    };
    Ok(session)
}

fn finish_query<T>(
    measured: QueryMeasurement<'_>,
    result: Result<anyhow::Result<T>, tokio::time::error::Elapsed>,
) -> Result<T, QueryFailure> {
    match result {
        Ok(Ok(value)) => {
            measured.success();
            Ok(value)
        }
        Ok(Err(error)) => {
            measured.error(&error);
            if postgres_connection_error(&error) {
                Err(QueryFailure::Connection)
            } else {
                Err(QueryFailure::Source)
            }
        }
        Err(_elapsed) => {
            measured.timeout();
            Err(QueryFailure::Timeout)
        }
    }
}

fn finish_failed<T>(
    measured: QueryMeasurement<'_>,
    result: Result<anyhow::Result<T>, tokio::time::error::Elapsed>,
) -> QueryCompletion {
    match result {
        Ok(Ok(_value)) => {
            measured.success();
            QueryCompletion::Complete
        }
        Ok(Err(error)) => {
            measured.error(&error);
            if postgres_connection_error(&error) {
                QueryCompletion::ConnectionFailed
            } else {
                QueryCompletion::SourceFailed
            }
        }
        Err(_elapsed) => {
            measured.timeout();
            QueryCompletion::TimedOut
        }
    }
}

fn finish_batched<E>(
    pool: &mut Pool,
    measured: QueryMeasurement<'_>,
    result: Result<(), BatchError<E>>,
) -> Result<bool, E> {
    Ok(matches!(
        finish_batched_kind(pool, measured, result)?,
        QueryCompletion::Complete | QueryCompletion::SourceFailed
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
        Err(BatchError::PostgreSql(error)) => {
            measured.error(&error);
            if postgres_stream_connection_error(&error) {
                pool.close();
                Ok(QueryCompletion::ConnectionFailed)
            } else {
                Ok(QueryCompletion::SourceFailed)
            }
        }
        Err(BatchError::Decode(error)) => {
            measured.error(&error);
            Ok(QueryCompletion::SourceFailed)
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

fn postgres_stream_connection_error(error: &tokio_postgres::Error) -> bool {
    error.is_closed() || error.as_db_error().is_none()
}

fn postgres_connection_error(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause
            .downcast_ref::<tokio_postgres::Error>()
            .is_some_and(tokio_postgres::Error::is_closed)
    })
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
                Err(QueryFailure::Connection)
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
