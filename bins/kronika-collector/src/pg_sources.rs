//! Asking a `PostgreSQL` server for its statistics.
//!
//! One server is collected: the sections carry no column saying which instance
//! a row came from, so two servers would write into each other's rows. The
//! first entry of `KRONIKA_PG_DSNS` is the one that is asked.
//!
//! A read happens before the window is built, because it is the only source
//! that has to wait on the network. A source that fails is one log line and no
//! rows for that section; the rest of the read continues.

mod buffering;

use std::collections::BTreeMap;
use std::time::Instant;

use kronika_source_pg::activity::ActivityRead;
use kronika_source_pg::archiver::ArchiverRow;
use kronika_source_pg::database::{DatabaseRow, DatabaseVersion};
use kronika_source_pg::databases;
use kronika_source_pg::extension;
use kronika_source_pg::io::{IoRow, IoVersion};
use kronika_source_pg::prepared_xacts::PreparedXactsRow;
use kronika_source_pg::progress_vacuum::ProgressVacuumRow;
use kronika_source_pg::settings::SettingsRow;
use kronika_source_pg::statements::{StatementsRow, StatementsVersion, statements_version};
use kronika_source_pg::store_plans::{self, Flavour, OsscRow, VadvRow};
use kronika_source_pg::user_indexes::{UserIndexesRow, UserIndexesVersion};
use kronika_source_pg::user_tables::{UserTablesRow, UserTablesVersion};
use kronika_source_pg::wal::WalSnapshot;
use kronika_source_pg::{
    MAX_AGE, Pool, activity, archiver, database, io, prepared_xacts, progress_vacuum, settings,
    statements, user_indexes, user_tables, wal,
};
use tokio_postgres::Client;

use crate::config::Config;
use crate::logging::{
    LogLevel, field, log_collection_failure, log_collection_finish, log_collection_start, log_event,
};
use crate::scheduler::{DueSet, SourceKind};

pub(crate) use buffering::push_pg_sources;

/// Section ids, for the collection log lines.
const SETTINGS: u32 = 1_019_001;
const ARCHIVER: u32 = 1_008_001;
const WAL: u32 = 1_009_001;
const PREPARED_XACTS: u32 = 1_007_001;
const DATABASE: u32 = 1_005_001;
const IO: u32 = 1_006_001;
const ACTIVITY: u32 = 1_001_001;
const PROGRESS_VACUUM: u32 = 1_012_001;
const STATEMENTS: u32 = 1_002_001;
const STORE_PLANS: u32 = 1_003_001;
const USER_TABLES: u32 = 1_013_001;
const USER_INDEXES: u32 = 1_014_001;

/// What one read of the server produced.
#[derive(Debug, Default)]
pub(crate) struct PgRows {
    /// The running configuration, written once per segment.
    pub(crate) settings: Vec<SettingsRow>,
    pub(crate) archiver: Option<ArchiverRow>,
    pub(crate) wal: Option<WalSnapshot>,
    pub(crate) prepared_xacts: Vec<PreparedXactsRow>,
    pub(crate) database: Option<(DatabaseVersion, Vec<DatabaseRow>)>,
    pub(crate) io: Option<(IoVersion, Vec<IoRow>)>,
    pub(crate) activity: Option<ActivityRead>,
    pub(crate) progress_vacuum: Vec<ProgressVacuumRow>,
    pub(crate) statements: Option<(StatementsVersion, Vec<StatementsRow>)>,
    pub(crate) store_plans_ossc: Vec<OsscRow>,
    pub(crate) store_plans_vadv: Vec<VadvRow>,
    pub(crate) user_tables: Option<(UserTablesVersion, Vec<UserTablesRow>)>,
    pub(crate) user_indexes: Option<(UserIndexesVersion, Vec<UserIndexesRow>)>,
}

/// The configured server, its per-database connections, and the last
/// configuration snapshot a new segment needs.
#[derive(Debug)]
pub(crate) struct PgSources {
    server: Option<Pool>,
    databases: BTreeMap<String, Pool>,
    settings: Vec<SettingsRow>,
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
        match Pool::new(dsn, MAX_AGE) {
            Ok(server) => Self {
                server: Some(server),
                databases: BTreeMap::new(),
                settings: Vec::new(),
            },
            Err(error) => {
                log_event(
                    LogLevel::Warn,
                    "pg_metrics_configuration_invalid",
                    &[field("error", format!("{error:#}"))],
                );
                Self::disabled()
            }
        }
    }

    const fn disabled() -> Self {
        Self {
            server: None,
            databases: BTreeMap::new(),
            settings: Vec::new(),
        }
    }

    /// The configuration snapshot a segment opening now should carry.
    pub(crate) fn last_settings(&self) -> &[SettingsRow] {
        &self.settings
    }

    /// Read the due sections. A server that cannot be reached produces no rows
    /// and one log line.
    pub(crate) async fn collect(&mut self, due: &DueSet) -> PgRows {
        let instance = due.has(SourceKind::Pg);
        let relations = due.has(SourceKind::PgRelations);
        if !instance && !relations {
            return PgRows::default();
        }
        let Some(server) = self.server.as_mut() else {
            return PgRows::default();
        };
        let major = match server_major(server).await {
            Ok(major) => major,
            Err(error) => {
                log_event(
                    LogLevel::Warn,
                    "pg_server_unreachable",
                    &[field("error", format!("{error:#}"))],
                );
                return PgRows::default();
            }
        };
        let mut rows = PgRows::default();
        if instance {
            collect_instance_wide(server, major, &mut rows).await;
            if !rows.settings.is_empty() {
                self.settings.clone_from(&rows.settings);
            }
        }
        if relations {
            self.collect_relations(major, &mut rows).await;
        }
        rows
    }

    /// Visit every database for the sections that only exist inside one.
    async fn collect_relations(&mut self, major: u32, rows: &mut PgRows) {
        let Some(server) = self.server.as_mut() else {
            return;
        };
        let found = match server.client().await {
            Ok(client) => match databases::enumerate(client).await {
                Ok(found) => found,
                Err(error) => {
                    log_event(
                        LogLevel::Warn,
                        "pg_databases_unreadable",
                        &[field("error", format!("{error:#}"))],
                    );
                    return;
                }
            },
            Err(error) => {
                log_event(
                    LogLevel::Warn,
                    "pg_server_unreachable",
                    &[field("error", format!("{error:#}"))],
                );
                return;
            }
        };
        databases::refresh(&mut self.databases, &found, server);
        let mut tables = Vec::new();
        let mut indexes = Vec::new();
        for database in &found {
            let Some(pool) = self.databases.get_mut(&database.name) else {
                continue;
            };
            let client = match pool.client().await {
                Ok(client) => client,
                Err(error) => {
                    log_database_unreachable(&database.name, &error);
                    continue;
                }
            };
            let started = Instant::now();
            log_collection_start(USER_TABLES, &database.name);
            match user_tables::collect_user_tables(client, database, major).await {
                Ok((_version, mut collected)) => {
                    log_collection_finish(
                        USER_TABLES,
                        &database.name,
                        collected.len(),
                        started.elapsed(),
                    );
                    tables.append(&mut collected);
                }
                Err(error) => {
                    log_collection_failure(USER_TABLES, &database.name, &error, started.elapsed());
                }
            }
            let started = Instant::now();
            log_collection_start(USER_INDEXES, &database.name);
            match user_indexes::collect_user_indexes(client, database, major).await {
                Ok((_version, mut collected)) => {
                    log_collection_finish(
                        USER_INDEXES,
                        &database.name,
                        collected.len(),
                        started.elapsed(),
                    );
                    indexes.append(&mut collected);
                }
                Err(error) => {
                    log_collection_failure(USER_INDEXES, &database.name, &error, started.elapsed());
                }
            }
        }
        if !tables.is_empty() {
            rows.user_tables = Some((user_tables::user_tables_version(major), tables));
        }
        if !indexes.is_empty() {
            rows.user_indexes = Some((user_indexes::user_indexes_version(major), indexes));
        }
    }
}

/// The server major version, which selects most of the layouts.
async fn server_major(server: &mut Pool) -> anyhow::Result<u32> {
    let client = server.client().await?;
    let row = client
        .query_one(
            "/* kronika: */ SELECT current_setting('server_version_num')::int4 AS num",
            &[],
        )
        .await?;
    let num: i32 = row.get("num");
    Ok(u32::try_from(num).unwrap_or(0) / 10_000)
}

/// Read the sections one connection to one database can answer for the whole
/// instance.
async fn collect_instance_wide(server: &mut Pool, major: u32, rows: &mut PgRows) {
    let client = match server.client().await {
        Ok(client) => client,
        Err(error) => {
            log_event(
                LogLevel::Warn,
                "pg_server_unreachable",
                &[field("error", format!("{error:#}"))],
            );
            return;
        }
    };
    rows.settings = read(SETTINGS, settings::collect(client), Vec::len)
        .await
        .unwrap_or_default();
    rows.archiver = read(ARCHIVER, archiver::collect_archiver(client), |_row| 1).await;
    rows.wal = read(WAL, wal::collect_wal(client, major), |found| {
        usize::from(found.is_some())
    })
    .await
    .flatten();
    rows.prepared_xacts = read(
        PREPARED_XACTS,
        prepared_xacts::collect_prepared_xacts(client),
        Vec::len,
    )
    .await
    .unwrap_or_default();
    rows.database = read(
        DATABASE,
        database::collect_database(client, major),
        |(_version, collected)| collected.len(),
    )
    .await;
    rows.io = read(IO, io::collect_io(client, major), |found| {
        found.as_ref().map_or(0, |(_version, rows)| rows.len())
    })
    .await
    .flatten();
    rows.activity = read(
        ACTIVITY,
        activity::collect_activity(client, major),
        |read| read.rows.len(),
    )
    .await;
    rows.progress_vacuum = read(
        PROGRESS_VACUUM,
        progress_vacuum::collect_progress_vacuum(client, major),
        Vec::len,
    )
    .await
    .unwrap_or_default();
    collect_extensions(client, rows).await;
}

/// The two extensions, each collected only where it is installed.
async fn collect_extensions(client: &Client, rows: &mut PgRows) {
    match extension::installed(client, statements::EXTENSION).await {
        Ok(Some(version)) => {
            if let Some(layout) = statements_version(version)
                && let Some(collected) = read(
                    STATEMENTS,
                    statements::collect_statements(client, layout),
                    Vec::len,
                )
                .await
            {
                rows.statements = Some((layout, collected));
            }
        }
        Ok(None) => {}
        Err(error) => log_extension_unreadable(statements::EXTENSION, &error),
    }
    match extension::installed(client, store_plans::EXTENSION).await {
        Ok(Some(version)) => match store_plans::flavour(version) {
            Some(Flavour::Ossc) => {
                rows.store_plans_ossc = read(
                    STORE_PLANS,
                    store_plans::collect_ossc(
                        client,
                        store_plans::TOP_N,
                        store_plans::PLAN_TEXT_BUDGET,
                    ),
                    Vec::len,
                )
                .await
                .unwrap_or_default();
            }
            Some(Flavour::Vadv) => {
                rows.store_plans_vadv = read(
                    STORE_PLANS,
                    store_plans::collect_vadv(
                        client,
                        store_plans::TOP_N,
                        store_plans::PLAN_TEXT_BUDGET,
                    ),
                    Vec::len,
                )
                .await
                .unwrap_or_default();
            }
            None => {}
        },
        Ok(None) => {}
        Err(error) => log_extension_unreadable(store_plans::EXTENSION, &error),
    }
}

/// Run one section's read, logging its start, what it produced, and its
/// failure. `count` says how many rows the read is worth in the log line.
async fn read<T, E: std::fmt::Display>(
    type_id: u32,
    collecting: impl Future<Output = Result<T, E>>,
    count: impl FnOnce(&T) -> usize,
) -> Option<T> {
    let started = Instant::now();
    log_collection_start(type_id, "postgresql");
    match collecting.await {
        Ok(collected) => {
            log_collection_finish(type_id, "postgresql", count(&collected), started.elapsed());
            Some(collected)
        }
        Err(error) => {
            log_collection_failure(type_id, "postgresql", &error, started.elapsed());
            None
        }
    }
}

fn log_database_unreachable(name: &str, error: &(dyn std::fmt::Display + '_)) {
    log_event(
        LogLevel::Warn,
        "pg_database_unreachable",
        &[
            field("database", name),
            field("error", format!("{error:#}")),
        ],
    );
}

fn log_extension_unreadable(name: &str, error: &(dyn std::fmt::Display + '_)) {
    log_event(
        LogLevel::Warn,
        "pg_extension_unreadable",
        &[
            field("extension", name),
            field("error", format!("{error:#}")),
        ],
    );
}
