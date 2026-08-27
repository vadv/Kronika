//! Tool names, descriptions, and input schemas returned by `tools/list`.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::route::{MAX_HEATMAP_TOP, MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup};

use super::filter::FilterInput;

pub(crate) const OVERVIEW_TOOL: &str = "kronika_overview";
pub(crate) const GET_CONTEXT_TOOL: &str = "kronika_get_context";
pub(crate) const GET_INSTANCE_TOOL: &str = "kronika_get_instance";
pub(crate) const FIND_POSTGRESQL_TABLES_TOOL: &str = "kronika_find_postgresql_tables";
pub(crate) const FIND_POSTGRESQL_INDEXES_TOOL: &str = "kronika_find_postgresql_indexes";
pub(crate) const FIND_POSTGRESQL_ACTIVITY_TOOL: &str = "kronika_find_postgresql_activity";
pub(crate) const FIND_POSTGRESQL_LOCKS_TOOL: &str = "kronika_find_postgresql_locks";
pub(crate) const FIND_POSTGRESQL_VACUUM_TOOL: &str = "kronika_find_postgresql_vacuum";
pub(crate) const FIND_POSTGRESQL_DATABASES_TOOL: &str = "kronika_find_postgresql_databases";
pub(crate) const FIND_POSTGRESQL_STATEMENTS_TOOL: &str = "kronika_find_postgresql_statements";
pub(crate) const FIND_POSTGRESQL_PLANS_TOOL: &str = "kronika_find_postgresql_plans";
pub(crate) const FIND_PROCESSES_TOOL: &str = "kronika_find_processes";
pub(crate) const GET_ROW_DETAIL_TOOL: &str = "kronika_get_row_detail";
pub(crate) const FIND_EVENTS_TOOL: &str = "kronika_find_events";

/// Ranks identities in one recorded logical section over an explicit time
/// window.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct OverviewInput {
    /// Recorded logical section. `kronika_get_context` lists recorded names
    /// with each layout's fields, classes, and units.
    pub(crate) section: String,
    /// One to four distinct numeric fields. All must be cumulative counters
    /// or all gauges. Kronika sums the listed fields before ranking;
    /// repeated names are rejected. Use fields with compatible units.
    #[schemars(length(min = 1, max = 4))]
    pub(crate) fields: Vec<String>,
    /// Inclusive window start, Unix microseconds.
    pub(crate) from: i64,
    /// Inclusive window end, Unix microseconds. An inverted window is
    /// rejected.
    pub(crate) to: i64,
    /// Number of ranked identities to return, from 1 through 500.
    #[schemars(range(min = 1, max = MAX_HEATMAP_TOP))]
    pub(crate) top: u32,
}

/// Takes no parameters. Braced, not a unit struct: schemars renders a unit
/// struct as `{"type": "null"}`, and tool arguments are always an object.
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "schemars needs the braces to render an object schema, not type: null"
)]
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetContextInput {}

/// Takes no parameters.
#[expect(
    clippy::empty_structs_with_brackets,
    reason = "schemars needs the braces to render an object schema, not type: null"
)]
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetInstanceInput {}

/// Output identity for table and index tools. `object` keeps one table or
/// index identity. Other values aggregate matching objects; each metric uses
/// its own reducer.
#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum GroupInput {
    /// One row per recorded table or index.
    Object,
    /// One aggregate row per database.
    Database,
    /// One aggregate row per schema.
    Schema,
    /// One aggregate row per tablespace.
    Tablespace,
}

impl From<GroupInput> for RelationGroup {
    fn from(value: GroupInput) -> Self {
        match value {
            GroupInput::Object => Self::Object,
            GroupInput::Database => Self::Database,
            GroupInput::Schema => Self::Schema,
            GroupInput::Tablespace => Self::Tablespace,
        }
    }
}

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum DirectionInput {
    Asc,
    Desc,
}

impl From<DirectionInput> for Order {
    fn from(value: DirectionInput) -> Self {
        match value {
            DirectionInput::Asc => Self::Asc,
            DirectionInput::Desc => Self::Desc,
        }
    }
}

/// Sorts matching rows before applying `limit`; omit for stable identity order.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct SortInput {
    /// Sort token documented by the enclosing tool. Unknown tokens are
    /// rejected; a plain tool retains identity order for a known column no
    /// selected physical layout exposes.
    pub(crate) field: String,
    /// `asc` puts the lowest non-null value first; `desc` puts the highest.
    /// Nulls remain last.
    pub(crate) direction: DirectionInput,
}

/// Input for `kronika_find_postgresql_tables`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct TablesInput {
    /// Required output identity: `object`, `database`, `schema`, or
    /// `tablespace`.
    pub(crate) group: GroupInput,
    /// AND-only predicates. Text fields (`eq` or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `tablespace`. Quantity fields
    /// (`gt` or `lt`): `size` (bytes), `table_count` and `xid_age` (count),
    /// `buffer_hit` (percentage points), `seq_scan_rate`, `change_rate`, and
    /// `autovacuum_rate` (count/s), `autovacuum_mean` (ms). Empty or omitted
    /// matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field available for the selected group, such as `seq_scan`,
    /// `n_live_tup`, `dead_pct`, `buffer_hit_pct`,
    /// `displayed_storage_bytes`, or `xid_age`. Invalid fields are rejected.
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_indexes`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct IndexesInput {
    /// Required output identity: `object`, `database`, `schema`, or
    /// `tablespace`.
    pub(crate) group: GroupInput,
    /// AND-only predicates. Text fields (`eq` or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `index_name`, `access_method`,
    /// `definition`, `tablespace`. Quantity fields (`gt` or `lt`): `size`
    /// (bytes), `index_count` (count), `buffer_hit` (percentage points), and
    /// `scan_rate` (count/s). Empty or omitted matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned field available for the selected group, such as `idx_scan`,
    /// `idx_tup_read`, `tuples_per_scan`, `main_fork_bytes`,
    /// `buffer_hit_pct`, or `state_severity`. Invalid fields are rejected.
    /// Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_activity`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ActivityInput {
    /// AND-only predicates. Text fields (`eq` or `contains`): `text`,
    /// `database`, `role`, `application`, `client_addr`, `backend_type`,
    /// `state`, `wait_event_type`, `wait_event`. Identifier fields (`eq`):
    /// `pid`, `query_id`. Quantity fields (`gt` or `lt`): `backend_xid_age`
    /// and `backend_xmin_age` (count). Empty or omitted matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned field, such as `pid`, `datname`, `state`,
    /// `backend_xid_age`, `backend_xmin_age`, or `query_start`. Filter aliases
    /// are not sort aliases; unknown names are rejected.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_locks`. Every returned row includes
/// `blocked_by`, a list of direct blocker PIDs; it is not filterable.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct LocksInput {
    /// AND-only predicates. Text fields (`eq` or `contains`): `text`,
    /// `database`, `role`, `state`, `lock_type`, `lock_mode`, `table_name`.
    /// Identifier field (`eq`): `pid`. Empty or omitted matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned scalar field, such as `pid`, `datname`, `state`,
    /// `lock_locktype`, `lock_mode`, `lock_relname`, or `waitstart`. Filter
    /// aliases are not sort aliases; unknown names are rejected.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_vacuum`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct VacuumInput {
    /// AND-only predicates. Text fields (`eq` or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `phase`, `is_autovacuum`; use the
    /// string `"true"` or `"false"` for `is_autovacuum`. Identifier field
    /// (`eq`): `pid`. Quantity fields (`gt` or `lt`): `heap_blks_total`,
    /// `heap_blks_scanned`, `heap_blks_vacuumed` (count). Empty or omitted
    /// matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned field, such as `pid`, `datname`, `schemaname`,
    /// `relname`, `phase`, `heap_blks_scanned`, or `heap_blks_vacuumed`.
    /// Filter aliases are not sort aliases. An unavailable name leaves rows in
    /// identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_databases`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct DatabasesInput {
    /// AND-only predicates. Text fields (`eq` or `contains`): `text`,
    /// `database`. Identifier field (`eq`): `datid`. Quantity fields (`gt` or
    /// `lt`): `numbackends` (count); `xact_commit`, `xact_rollback`, and
    /// `deadlocks` compare counter delta per microsecond; `temp_bytes` compares
    /// byte delta per microsecond. Returned versions of the latter four are
    /// per-second rates. Empty or omitted matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned field, such as `datid`, `datname`, `numbackends`,
    /// `xact_commit`, `deadlocks`, `temp_bytes`, or `active_time`. Cumulative
    /// fields sort by interval rate. Filter aliases are not sort aliases;
    /// unknown names are rejected.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_statements`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct StatementsInput {
    /// AND-only predicates. Text: `text`, `database`, `role`; identifier:
    /// `query_id`; count/s: `call_rate`, `row_rate`, `plan_rate`; ms/s:
    /// `exec_time_rate`, `planning_time_rate`, `shared_read_time_rate`,
    /// `shared_write_time_rate`, `local_read_time_rate`,
    /// `local_write_time_rate`, `temp_read_time_rate`,
    /// `temp_write_time_rate`; ms: `mean_exec`, `min_exec_since_reset`,
    /// `max_exec_since_reset`, `mean_exec_since_reset`,
    /// `stddev_exec_since_reset`; unitless: `rows_per_call`, `exec_cv`;
    /// percentage points: `planning_share`, `buffer_hit`; bytes/s:
    /// `shared_buffer_hit_rate`, `shared_buffer_read_rate`,
    /// `shared_buffer_dirty_rate`, `shared_buffer_write_rate`,
    /// `local_buffer_hit_rate`, `local_buffer_read_rate`,
    /// `local_buffer_dirty_rate`, `local_buffer_write_rate`,
    /// `temp_buffer_read_rate`, `temp_buffer_write_rate`, `wal_rate`; bytes:
    /// `buffer_per_call`, `wal_per_call`. Use `eq`/`contains` for text, `eq`
    /// for the identifier, and `gt`/`lt` for quantities. Empty or omitted
    /// matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned field, such as `calls`, `total_exec_time`, `rows`,
    /// `shared_blks_read`, or `wal_bytes`, or one of the seven returned
    /// `derived_*` names (`derived_hit_fraction` and
    /// `derived_plan_time_fraction` rank by their 0-100 renderings — the
    /// same order). Unknown names are rejected; omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_postgresql_plans`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct PlansInput {
    /// AND-only predicates. Text: `text`, `database`, `role`; identifiers:
    /// `query_id`, `plan_id`; count: `calls`; count/s: `call_rate`, `row_rate`,
    /// `slow_call_rate`; ms/s: `exec_time_rate`, `planning_time_rate`,
    /// `shared_read_time_rate`, `shared_write_time_rate`,
    /// `local_read_time_rate`, `local_write_time_rate`, `temp_read_time_rate`,
    /// `temp_write_time_rate`; ms: `mean_exec`, `min_exec_since_reset`,
    /// `max_exec_since_reset`, `mean_exec_since_reset`,
    /// `stddev_exec_since_reset`; unitless: `rows_per_call`, `exec_cv`;
    /// percentage points: `planning_share`, `buffer_hit`; bytes/s:
    /// `shared_buffer_hit_rate`, `shared_buffer_read_rate`,
    /// `shared_buffer_dirty_rate`, `shared_buffer_write_rate`,
    /// `local_buffer_hit_rate`, `local_buffer_read_rate`,
    /// `local_buffer_dirty_rate`, `local_buffer_write_rate`,
    /// `temp_buffer_read_rate`, `temp_buffer_write_rate`; bytes:
    /// `buffer_per_call`. Use `eq`/`contains` for text, `eq` for identifiers,
    /// and `gt`/`lt` for quantities. Empty or omitted matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned field, such as `calls`, `total_time`, `rows`, or
    /// `shared_blks_read`, or one of the seven returned `derived_*` names.
    /// `calls` sorts by its interval rate although the returned field is an
    /// exact cumulative count. Unknown names are rejected; omit for
    /// identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Input for `kronika_find_processes`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct ProcessesInput {
    /// AND-only predicates. Text: `text`, `user`, `effective_user`, `command`,
    /// `state`; identifiers: `user_id`, `effective_user_id`, `pid`,
    /// `parent_pid`; bytes: `rss`, `vsz`, `swap`; count: `threads`; unitless
    /// CPU cores: `cpu_cores`, `user_cpu_cores`, `system_cpu_cores`; bytes/s:
    /// `disk_read_rate`, `disk_write_rate`, `logical_read_rate`,
    /// `logical_write_rate`; count/s: `read_syscall_rate`,
    /// `write_syscall_rate`, `major_fault_rate`, `minor_fault_rate`,
    /// `context_switch_rate`, `voluntary_context_switch_rate`,
    /// `involuntary_context_switch_rate`; ms/s: `run_delay`,
    /// `block_io_delay`. Use `eq`/`contains` for text, `eq` for identifiers,
    /// and `gt`/`lt` for quantities. Empty or omitted matches all rows.
    #[serde(default)]
    pub(crate) filters: Vec<FilterInput>,
    /// Physical returned field, such as `pid`, `comm`, `rmem_kb`, `vmem_kb`,
    /// `num_threads`, `utime`, `read_bytes`, or `rundelay_ns`. Filter aliases
    /// (`rss`, `vsz`, `threads`, and rate names) and virtual fields are not
    /// sort aliases; unknown names are rejected. Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Exact physical locator. Every numeric field accepts a JSON integer or a
/// decimal string; copy decimal strings from a find result without conversion.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct RowDetailInput {
    /// Recorded logical section. Plain timestamped and event sections are
    /// accepted. `pg_stat_user_tables` and `pg_stat_user_indexes` are rejected
    /// because their find results aggregate physical rows.
    pub(crate) section: String,
    /// Signed 64-bit recorded segment ID, as a JSON integer or decimal string.
    pub(crate) segment_id: serde_json::Value,
    /// Signed 64-bit row timestamp in Unix microseconds, as a JSON integer or
    /// decimal string.
    pub(crate) at: serde_json::Value,
    /// Unsigned 32-bit physical layout ID, as a JSON integer or decimal string.
    pub(crate) type_id: serde_json::Value,
    /// Unsigned 64-bit physical row position within the section, as a JSON
    /// integer or decimal string.
    pub(crate) row_ordinal: serde_json::Value,
}

/// Reads selected recorded event sections over an inclusive time window.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EventsInput {
    /// Recorded sections to read: `pg_log_errors`, `pg_log_checkpoints`,
    /// `pg_log_autovacuum`, `pg_log_slow_queries`, `pg_log_lock_waits`,
    /// `pg_log_temp_files`, `pg_log_lifecycle`, `pgbouncer_events`. Omit or
    /// use null for all eight; an empty array reads none. Use each source
    /// once: duplicates duplicate its rows.
    #[serde(default)]
    pub(crate) sources: Option<Vec<String>>,
    /// Inclusive start of the window, Unix microseconds.
    pub(crate) from: i64,
    /// Inclusive end, Unix microseconds. `to` must be at least `from`, and
    /// `to - from` must not exceed 3,600,000,000 microseconds (one hour).
    pub(crate) to: i64,
    /// Maximum combined rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

pub(crate) fn tools() -> Vec<Tool> {
    entry_tools()
        .into_iter()
        .chain(relation_tools())
        .chain(postgresql_plain_tools())
        .chain(ratio_tools())
        .chain(tail_tools())
        .collect()
}

/// `kronika_overview`, `kronika_get_context`, and `kronika_get_instance`,
/// kept separate to satisfy the line-count lint.
fn entry_tools() -> [Tool; 3] {
    [
        Tool::new(
            OVERVIEW_TOOL,
            "Ranks stored values in one logical section over the inclusive \
             Unix-microsecond `[from, to]` window; it does not query the live \
             host or database. Counter `entities[].total` values are \
             whole-window deltas; gauge totals are whole-window maxima. The \
             requested fields are summed before ranking and must share one \
             unit. `entities` is \
             descending by total with nulls last, and `entity_count` is the \
             decimal-string number of identities found. For counters, \
             `totals_total` and `others_total` sum all and omitted identity \
             deltas; for gauges they are the maxima across all and omitted \
             identities. Null means no usable value. Each entity carries an \
             `identity` object naming the section's identity columns. \
             Fields a finder accepts as filters keep the finder's spelling \
             — `query_id`, `plan_id`, `pid`, and pg_stat_database's \
             `datid` — and their values pass to a `find_*` filter \
             verbatim. The other identity fields (`dbid`, `userid`, \
             `toplevel`, and the table/index OIDs) have no finder filter: \
             rank tables or indexes with the finder's own sort instead. \
             `kronika_get_context` lists each section's fields, classes, and \
             units, and the recorded time range this window must fall into. \
             Across this catalog, tool failures set `isError=true` and \
             contain text only, without `structuredContent`.",
            schema_object::<OverviewInput>(),
        ),
        Tool::new(
            GET_CONTEXT_TOOL,
            "Lists physical section layouts found across all stored segments; \
             it does not inspect the live host or database. Top-level \
             `recorded_from`/`recorded_to` are the store's first and last \
             recorded timestamps, decimal-string Unix microseconds — an \
             empty answer elsewhere may just mean the window fell outside \
             them. Each `sections` item contains `logical_name`, \
             `physical_name`, decimal-string `type_id`, `rows`, `bytes`, its \
             `identity` column names, and `fields` — every column's `name`, \
             `class`, and `unit`. A `cumulative` column is a monotonic \
             counter that `find_*` rows return as a per-second rate under \
             the raw column name and `kronika_overview` ranks as a \
             whole-window delta; a `gauge` is an instantaneous value. \
             `implementation` and `source_family` may be null. Rows and \
             bytes are summed separately for each physical `type_id` across \
             segments, so one logical name may appear more than once. \
             Store-internal `dict.*` layouts are omitted.",
            schema_object::<GetContextInput>(),
        ),
        Tool::new(
            GET_INSTANCE_TOOL,
            "Returns stored host facts and PostgreSQL server settings; it \
             does not query the live host or database. `host` is the newest \
             recorded instance_metadata row — hostname, kernel version, \
             environment (0 machine or VM, 1 container), \
             `clock_ticks_per_sec` (converts jiffies/s rates), \
             `page_size_bytes` (converts page counts), boot id and Unix-\
             microsecond boot time, whether PostgreSQL collection was \
             configured, its cadence in seconds, and its effective CPU \
             count — or null when never recorded. `postgresql_settings` are \
             the newest recorded pg_settings rows, one per parameter per \
             monitored session (`datname`/`usename` name the session): \
             `name`, `setting`, and `unit` (null for unitless parameters) \
             are the strings PostgreSQL reports, alongside `source`, \
             `context`, `vartype`, `boot_val`, `reset_val`, and \
             `pending_restart`; empty when never recorded. The two \
             parameters whose values may hold secrets (primary_conninfo, \
             ssl_passphrase_command) are never recorded. `host_as_of` and `settings_as_of` \
             are each part's snapshot anchor as decimal-string Unix \
             microseconds, null for a part never recorded; \
             `settings_has_more` means settings rows were omitted at the \
             5,000-row cap. Rows carry decimal-string locator fields \
             accepted unchanged by kronika_get_row_detail.",
            schema_object::<GetInstanceInput>(),
        ),
    ]
}

/// The table and index tools, kept separate to satisfy the line-count lint.
fn relation_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_TABLES_TOOL,
            "Finds stored PostgreSQL table statistics at the snapshot anchor: \
             the maximum timestamp of the greatest recorded segment carrying \
             the section, with each \
             compatible layout contributing its latest observation at or \
             before that time. It does not query PostgreSQL. Filters are \
             ANDed during aggregation and before sorting and `limit`; omitted \
             filters match all. `group` selects table, database, schema, or \
             tablespace identity, and metrics use field-specific reducers. \
             Object identity contains `datid`, `datname`, `schemaname`, \
             `relid`, and `relname`; database identity contains \
             `datid`/`datname`; schema adds `schemaname`; tablespace identity \
             uses `tablespace_oid`. \
             Rate fields such as `seq_scan`, tuple changes, vacuum counts, and \
             block reads/hits are per second; `*_bytes` are bytes, `*_pct` are \
             percentage points, `*_mean_ms` are milliseconds, and timestamps \
             are Unix microseconds. Exact 64-bit values use decimal strings; \
             unavailable metrics are null. Returns `{rows, has_more, as_of}`: \
             `as_of` is the anchor as a decimal-string Unix-microsecond \
             timestamp, null alongside empty rows when no segment records \
             the section; `has_more` means matches were omitted; no \
             continuation cursor is returned. Aggregated relation rows have no physical locator and \
             cannot be passed to `kronika_get_row_detail`.",
            schema_object::<TablesInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_INDEXES_TOOL,
            "Finds stored PostgreSQL index statistics at the snapshot anchor: \
             the maximum timestamp of the greatest recorded segment carrying \
             the section, with each \
             compatible layout contributing its latest observation at or \
             before that time. It does not query PostgreSQL. Filters are \
             ANDed during aggregation and before sorting and `limit`; omitted \
             filters match all. `group` selects index, database, schema, or \
             tablespace identity, and metrics use field-specific reducers. \
             Object identity contains `datid`, `datname`, `schemaname`, \
             `relid`, `relname`, `indexrelid`, and `indexrelname`; database \
             identity contains `datid`/`datname`; schema adds `schemaname`; \
             tablespace identity uses `tablespace_oid`. \
             `idx_scan`, `idx_tup_read`, `idx_tup_fetch`, `idx_blks_read`, and \
             `idx_blks_hit` are per-second rates; `tuples_per_scan` and \
             `fetches_per_scan` are unitless. `*_bytes` are bytes, `*_pct` are \
             percentage points, and timestamps are Unix microseconds. Exact \
             64-bit values use decimal strings; unavailable metrics are null. \
             Returns `{rows, has_more, as_of}` with the same meanings as the \
             tables tool. Aggregated relation rows have \
             no physical locator and cannot be passed to \
             `kronika_get_row_detail`.",
            schema_object::<IndexesInput>(),
        ),
    ]
}

/// The process, row-detail, and event tools, kept separate to satisfy the
/// line-count lint.
fn tail_tools() -> [Tool; 3] {
    [
        Tool::new(
            FIND_PROCESSES_TOOL,
            "Finds stored Linux process observations at the snapshot anchor: \
             the maximum timestamp of the greatest recorded segment carrying \
             the section, with each \
             compatible layout contributing its latest observation at or \
             before that time. It does not query the OS. Filters are ANDed \
             before sorting and `limit`; omitted filters match all. Returned \
             `rmem_kb`, `vmem_kb`, and `vswap_kb` are KiB; `utime` and `stime` \
             are jiffies/s; `rundelay_ns` is ns/s; `blkdelay_ticks` is \
             jiffies/s; fault, context-switch, and syscall counters are \
             count/s; I/O counters are bytes/s. Rates are null without a \
             usable predecessor or across a PID start-time change. \
             Unrecorded `/proc/PID/io` fields are null. Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. Each row \
             has decimal-string `segment_id`, `type_id`, `row_ordinal`, and \
             `at` accepted unchanged by `kronika_get_row_detail`.",
            schema_object::<ProcessesInput>(),
        ),
        Tool::new(
            GET_ROW_DETAIL_TOOL,
            "Returns the rendered row addressed by exact physical locator \
             (`section`, `segment_id`, `at`, `type_id`, `row_ordinal`) from any \
             stored segment. Locator fields accept JSON integers or decimal \
             strings; copy the strings from a `find_*` row to avoid precision \
             loss. `pg_store_plans.calls` remains the exact stored count and \
             adds `calls_per_second`; other cumulative columns are rendered as \
             interval rates rather than stored counter values. Missing, \
             unavailable, or underivable values are null. Plain timestamped \
             sections, including event sections, are supported; relation-grouped \
             `pg_stat_user_tables` and `pg_stat_user_indexes` are rejected. A \
             missing or mismatched locator returns an error. Recognized event \
             codes receive the same `<field>_label` siblings as \
             `kronika_find_events`; the find-only `source` field is not part of \
             the stored row and is not returned here.",
            schema_object::<RowDetailInput>(),
        ),
        Tool::new(
            FIND_EVENTS_TOOL,
            "Reads stored event rows from selected PostgreSQL and PgBouncer \
             sources in inclusive Unix-microsecond `[from, to]`; it performs \
             no live query and has no field predicates. It keeps the \
             earliest `limit` matches per source regardless of stored order, \
             sorts the merged list by `at`, and truncates it to `limit` from \
             the newest end — the oldest rows survive. Equal timestamps \
             preserve requested source order; within a source they follow \
             the physical locator. Returns `{rows, has_more}`; `has_more` \
             means rows were omitted, including matches inside segments the \
             scan could skip, and no continuation cursor is returned. Each row includes `source` and \
             decimal-string `segment_id`, `type_id`, `row_ordinal`, and `at` \
             accepted unchanged by `kronika_get_row_detail`. Recognized \
             Kronika event codes keep their numeric field and add a \
             `<field>_label` sibling. Code numbers are not severity ranks; \
             unknown codes have no label sibling.",
            schema_object::<EventsInput>(),
        ),
    ]
}

/// Plain `PostgreSQL` tools, kept separate to satisfy the line-count lint.
fn postgresql_plain_tools() -> [Tool; 4] {
    [
        Tool::new(
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            "Finds stored PostgreSQL backend activity, state, waits, query \
             text, and transaction-age fields at the snapshot anchor: the \
             maximum timestamp of the greatest recorded segment carrying the \
             section. Each compatible \
             layout contributes its latest observation at or before that time, \
             so layouts may contribute different `at` values. It does not \
             query PostgreSQL. Filters are ANDed before sorting and `limit`; \
             omitted filters match all. XID ages are counts; `backend_start`, \
             `xact_start`, `query_start`, and `state_change` are Unix \
             microseconds. Null denotes no transaction, query, or wait where \
             applicable, a null recording, or a field absent from the physical \
             layout. Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. Each row has decimal-string `segment_id`, \
             `type_id`, `row_ordinal`, and `at` accepted unchanged by \
             `kronika_get_row_detail`.",
            schema_object::<ActivityInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_LOCKS_TOOL,
            "Finds stored PostgreSQL backends in a direct lock-wait graph at \
             the snapshot anchor: the maximum timestamp of the greatest \
             recorded segment carrying the section. Each compatible layout contributes its latest \
             observation at or before that time; it does not query PostgreSQL. \
             Filters are ANDed before sorting and `limit`; omitted filters \
             match all. `blocked_by` is always a list of direct blocker PIDs; \
             an empty list marks a root or blocker-only row, and PID 0 denotes \
             a prepared-transaction holder. Timestamp fields are Unix \
             microseconds. Null denotes an inapplicable, null, or unavailable \
             field. The wait graph is recorded only while contention \
             exists, so the anchor can predate the present and the rows can \
             describe contention that has since ended. Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. Each row has \
             decimal-string `segment_id`, `type_id`, `row_ordinal`, and `at` \
             accepted unchanged by `kronika_get_row_detail`.",
            schema_object::<LocksInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_VACUUM_TOOL,
            "Finds PostgreSQL backends recorded as running `VACUUM` at the \
             snapshot anchor: the maximum timestamp of the greatest recorded \
             segment carrying the section. Each compatible layout contributes its latest \
             observation at or before that time; it does not query PostgreSQL. \
             Filters are ANDed before sorting and `limit`; omitted filters \
             match all. Heap, index, and dead-item fields are counts; \
             `dead_tuple_bytes` and `max_dead_tuple_bytes` are bytes; \
             `delay_time` is milliseconds. Version-specific unavailable \
             fields are null. The section is recorded only while a vacuum \
             runs: empty rows with null `as_of` mean none was ever recorded, \
             and the anchor can point at the last vacuum, not the present. \
             Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. Each row has decimal-string `segment_id`, `type_id`, \
             `row_ordinal`, and `at` accepted unchanged by \
             `kronika_get_row_detail`.",
            schema_object::<VacuumInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_DATABASES_TOOL,
            "Finds stored per-database PostgreSQL statistics at the snapshot \
             anchor: the maximum timestamp of the greatest recorded segment \
             carrying the section. \
             Each compatible layout contributes its latest observation at or \
             before that time; it does not query PostgreSQL. Filters are ANDed \
             before sorting and `limit`; omitted filters match all. \
             `numbackends` is a recorded count. Cumulative count fields are \
             returned as count/s, `temp_bytes` as bytes/s, and cumulative time \
             fields as ms/s. `stats_reset` and `checksum_last_failure` are Unix \
             microseconds. Interval rates are null without a usable predecessor \
             or after counter rollback; absent or null fields are null. Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. \
             Each row has decimal-string locator fields accepted unchanged \
             by `kronika_get_row_detail`.",
            schema_object::<DatabasesInput>(),
        ),
    ]
}

/// The statement and plan tools, kept separate to satisfy the line-count
/// lint.
fn ratio_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_STATEMENTS_TOOL,
            "Finds stored `pg_stat_statements` rows at the snapshot anchor: the \
             maximum timestamp of the greatest recorded segment carrying the \
             section, with the latest \
             observation selected separately per compatible layout. It does \
             not query PostgreSQL. Filters are ANDed before sorting and \
             `limit`. Cumulative fields are returned as interval rates: \
             count/s, PostgreSQL blocks/s, bytes/s for `wal_bytes`, and ms/s \
             for time counters; min/max/mean/stddev gauges remain ms. Seven \
             added fields are `derived_mean_exec_ms_per_call`, \
             `derived_rows_per_call`, `derived_blocks_per_call` (shared plus \
             local hit/read blocks), `derived_hit_fraction` (shared \
             hit/(hit+read), 0..1), `derived_wal_per_call` (bytes/call), \
             `derived_plan_time_fraction` (plan/(plan+execution), 0..1), and \
             `derived_cv` (execution stddev/mean). A derived value is null for \
             a missing/null operand, zero denominator, or non-finite result; \
             rate operands are also null without a usable predecessor or after \
             rollback. Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. Locator fields are decimal strings accepted \
             by `kronika_get_row_detail`.",
            schema_object::<StatementsInput>(),
        ),
        Tool::new(
            FIND_POSTGRESQL_PLANS_TOOL,
            "Finds stored `pg_store_plans` rows at the snapshot anchor: the \
             maximum timestamp of the greatest recorded segment carrying the \
             section, with the latest \
             observation selected separately per compatible layout. It does \
             not query PostgreSQL. Filters are ANDed before sorting and \
             `limit`. `calls` is the exact cumulative count and \
             `calls_per_second` its interval rate; other cumulative fields are \
             returned as count/s, PostgreSQL blocks/s, or ms/s. Seven added \
             fields use the statement formulas: mean execution ms/call, \
             rows/call, shared-plus-local blocks/call, shared hit fraction \
             (0..1), WAL bytes/call, planning fraction (0..1), and execution \
             coefficient of variation. `derived_wal_per_call` is always null \
             because plan layouts have no WAL bytes. \
             `derived_plan_time_fraction` is non-null only for the vadv layout. \
             Other derived nulls mean a missing/null operand, zero denominator, \
             non-finite result, missing predecessor, or rollback. Returns `{rows, has_more, as_of}`: `as_of` is the \
             anchor as a decimal-string Unix-microsecond timestamp, null \
             alongside empty rows when no segment records the section; \
             `has_more` means matches were omitted, and no continuation \
             cursor is returned. \
             Locator fields are decimal strings accepted by \
             `kronika_get_row_detail`.",
            schema_object::<PlansInput>(),
        ),
    ]
}

/// Serializes a Schemars schema into rmcp's `Arc<JsonObject>` without binding
/// this module to Schemars schema types.
fn schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    let schema = schemars::schema_for!(T);
    let value = serde_json::to_value(schema).expect("schema serializes to JSON");
    let object = value.as_object().expect("schema is a JSON object").clone();
    Arc::new(object)
}

#[cfg(test)]
mod tests;
