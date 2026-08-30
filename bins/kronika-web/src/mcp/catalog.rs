//! Tool names, descriptions, and input schemas returned by `tools/list`.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::api::events::{EventsRepresentation, EventsResult};
use crate::api::heatmap::{DEFAULT_TOP, HeatmapBatchResult, MAX_TOP};
use crate::api::snapshot::search::SEARCH_MAX_CLAUSES;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup};

use super::filter::FilterInput;
use super::instance::InstanceOutput;
use super::semantics::FinderOutput;
use super::time::TimeSpecInput;

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

/// Executes ordered stored-data rankings over one half-open time window.
#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverviewInput {
    /// Inclusive start and exclusive end accept JSON integer or canonical
    /// signed decimal-string i64 Unix microseconds, RFC 3339, `now`, or a
    /// fixed-duration `now-N` expression.
    pub(crate) from: TimeSpecInput,
    pub(crate) to: TimeSpecInput,
    /// Ordered nonempty recipes. Exact duplicates are returned in place.
    #[schemars(length(min = 1))]
    pub(crate) rankings: Vec<OverviewRankingInput>,
}

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct OverviewRankingInput {
    /// Recorded logical section. `kronika_get_context` lists valid names.
    #[schemars(length(min = 1, max = 128))]
    pub(crate) section: String,
    /// One to four distinct numeric fields. All must be cumulative counters
    /// or all gauges. Kronika sums the listed fields before ranking;
    /// repeated names are rejected. Use fields with compatible units.
    #[schemars(length(min = 1, max = 4))]
    pub(crate) fields: Vec<String>,
    /// Number of ranked identities to return, from 1 through 500. Defaults to
    /// 25 when omitted.
    #[serde(default = "default_overview_top")]
    #[schemars(range(min = 1, max = MAX_TOP))]
    pub(crate) top: u64,
}

const fn default_overview_top() -> u64 {
    DEFAULT_TOP as u64
}

/// Narrows the answer to one recorded section; omit it for all of them.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetContextInput {
    /// One recorded logical section name; omit for every recorded layout.
    #[serde(default)]
    pub(crate) section: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SettingsScopeInput {
    /// Return every row except those whose recorded source is exactly `default`.
    #[default]
    NonDefault,
    /// Return every recorded `PostgreSQL` setting row.
    All,
}

/// Selects the recorded `PostgreSQL` settings included with instance facts.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetInstanceInput {
    /// `non_default` omits only rows whose recorded `source` is exactly
    /// `default`; null, missing, and unknown sources remain. `all` returns
    /// defaults too. Omit for `non_default`.
    #[serde(default)]
    pub(crate) settings: SettingsScopeInput,
}

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
#[serde(deny_unknown_fields)]
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
#[serde(deny_unknown_fields)]
pub(crate) struct TablesInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// Required output identity: `object`, `database`, `schema`, or
    /// `tablespace`.
    pub(crate) group: GroupInput,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `tablespace`. Quantity fields
    /// (`gt` or `lt`): `size` (bytes), `table_count` and `xid_age` (count),
    /// `buffer_hit` (percentage points), `seq_scan_rate`, `change_rate`, and
    /// `autovacuum_rate` (count/s), `autovacuum_mean` (ms). Empty or omitted
    /// matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct IndexesInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// Required output identity: `object`, `database`, `schema`, or
    /// `tablespace`.
    pub(crate) group: GroupInput,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `index_name`, `access_method`,
    /// `definition`, `tablespace`. Quantity fields (`gt` or `lt`): `size`
    /// (bytes), `index_count` (count), `buffer_hit` (percentage points), and
    /// `scan_rate` (count/s). Empty or omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct ActivityInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `role`, `application`, `client_addr`, `backend_type`,
    /// `state`, `wait_event_type`, `wait_event`. Identifier fields (`eq` or `in`):
    /// `pid`, `query_id`. Quantity fields (`gt` or `lt`): `backend_xid_age`
    /// and `backend_xmin_age` (count). Empty or omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct LocksInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `role`, `state`, `lock_type`, `lock_mode`, `table_name`.
    /// Identifier field (`eq` or `in`): `pid`. Empty or omitted matches all
    /// rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct VacuumInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `schema`, `table_name`, `phase`, `is_autovacuum`; use the
    /// string `"true"` or `"false"` for `is_autovacuum`. Identifier field
    /// (`eq` or `in`): `pid`. Quantity fields (`gt` or `lt`): `heap_blks_total`,
    /// `heap_blks_scanned`, `heap_blks_vacuumed` (count). Empty or omitted
    /// matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct DatabasesInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`. Identifier field (`eq` or `in`): `datid`. Quantity fields
    /// (`gt` or `lt`): `numbackends` (count); `xact_commit`, `xact_rollback`, and
    /// `deadlocks` compare counter delta per microsecond; `temp_bytes` compares
    /// byte delta per microsecond. Returned versions of the latter four are
    /// per-second rates. Empty or omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct StatementsInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
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
    /// `buffer_per_call`, `wal_per_call`. Use `eq`/`in`/`contains` for text,
    /// `eq`/`in` for the identifier, and `gt`/`lt` for quantities. Empty or
    /// omitted matches all rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct PlansInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
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
    /// `buffer_per_call`. Use `eq`/`in`/`contains` for text, `eq`/`in` for
    /// identifiers, and `gt`/`lt` for quantities. Empty or omitted matches all
    /// rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
#[serde(deny_unknown_fields)]
pub(crate) struct ProcessesInput {
    /// Omit for the latest recorded observation, or select the latest usable
    /// observation no later than this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text: `text`, `user`, `effective_user`, `command`,
    /// `state`; identifiers: `user_id`, `effective_user_id`, `pid`,
    /// `parent_pid`; bytes: `rss`, `vsz`, `swap`; count: `threads`; unitless
    /// CPU cores: `cpu_cores`, `user_cpu_cores`, `system_cpu_cores`; bytes/s:
    /// `disk_read_rate`, `disk_write_rate`, `logical_read_rate`,
    /// `logical_write_rate`; count/s: `read_syscall_rate`,
    /// `write_syscall_rate`, `major_fault_rate`, `minor_fault_rate`,
    /// `context_switch_rate`, `voluntary_context_switch_rate`,
    /// `involuntary_context_switch_rate`; ms/s: `run_delay`,
    /// `block_io_delay`. Use `eq`/`in`/`contains` for text, `eq`/`in` for
    /// identifiers, and `gt`/`lt` for quantities. Empty or omitted matches all
    /// rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
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
    /// The identifying value copied verbatim from the find row's `row_key`
    /// field. Required for rows that carry one; a mismatch reports a stale
    /// locator instead of returning another row.
    #[serde(default)]
    pub(crate) row_key: Option<serde_json::Value>,
}

/// Reads selected recorded event sections over a half-open time window.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct EventsInput {
    /// Recorded sections to read: `pg_log_errors`, `pg_log_checkpoints`,
    /// `pg_log_autovacuum`, `pg_log_slow_queries`, `pg_log_lock_waits`,
    /// `pg_log_temp_files`, `pg_log_lifecycle`, `pgbouncer_events`. Omit or
    /// use null for every source valid for the representation; an empty array
    /// reads none. Repeats are removed after their first occurrence.
    #[serde(default)]
    pub(crate) sources: Option<Vec<String>>,
    /// Inclusive start: JSON integer or canonical signed decimal-string i64
    /// Unix microseconds, RFC 3339, `now`, or `now-N<unit>`.
    pub(crate) from: TimeSpecInput,
    /// Exclusive end in the same time grammar. `to` must be at least `from`, and
    /// `to - from` must not exceed 3,600,000,000 microseconds (one hour).
    pub(crate) to: TimeSpecInput,
    /// Server-grouped console entries (default) or raw stored occurrences.
    #[serde(default = "default_events_representation")]
    pub(crate) representation: EventsRepresentation,
    /// Maximum combined rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

const fn default_events_representation() -> EventsRepresentation {
    EventsRepresentation::Groups
}

pub(crate) fn tools() -> Vec<Tool> {
    entry_tools()
        .into_iter()
        .chain(std::iter::once(instance_tool()))
        .chain(relation_tools())
        .chain(postgresql_plain_tools())
        .chain(ratio_tools())
        .chain(tail_tools())
        .collect()
}

/// `kronika_overview` and `kronika_get_context`, kept separate to
/// satisfy the line-count lint.
fn entry_tools() -> [Tool; 2] {
    [overview_tool(), context_tool()]
}

fn overview_tool() -> Tool {
    Tool::new(
        OVERVIEW_TOOL,
        "Runs an ordered batch of rankings over stored data in the half-open \
         `[from,to)` window; it never queries the live host or database. \
         `from` and `to` accept a JSON integer or canonical signed \
         decimal-string i64 Unix-microsecond timestamp, RFC 3339 with `Z` or a numeric UTC offset, `now`, or \
         `now-N{us,ms,s,m,h,d,w}`. One clock anchor resolves the whole call. \
         Each recipe contains one logical section, one to four distinct \
         numeric fields of one class and exact unit, and optional `top` \
         (default 25, maximum 500). Fields are summed per row before ranking. \
         Results retain request order and duplicate recipes. Counter totals \
         are whole-window non-negative deltas; gauge totals are window \
         maxima. `as_of` is the latest usable metric observation included in \
         that ranking. Every entity includes its named identity and the full \
         automatic label set for compatible recorded layouts; an unavailable \
         label is null. Coverage always reports data/no_data state, store-wide \
         recorded bounds, nearest rows around the window, and in-window row \
         count. Timestamps, identifiers, and counts that may exceed safe JSON \
         integer precision are decimal strings. A request or encoded-result \
         budget refusal names `ranking_index`. `top` bounds returned identities; \
         it does not change pre-ranking scan state.",
        schema_object::<OverviewInput>(),
    )
    .with_output_schema::<HeatmapBatchResult>()
}

fn context_tool() -> Tool {
    Tool::new(
        GET_CONTEXT_TOOL,
        "Lists physical section layouts found across all stored segments; \
         it does not inspect the live host or database. Top-level \
         `recorded_from`/`recorded_to` are the store's first and last \
         recorded timestamps, decimal-string Unix microseconds accepted \
         unchanged by MCP `from`, `to`, and `at` inputs — an \
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
         Store-internal `dict.*` layouts are omitted. The full answer \
         is tens of kilobytes — read it once per session, or pass \
         `section` to keep one section's layouts only. `recorded_to` is \
         the newest recorded moment: the anchor of the present for a store \
         read offline.",
        schema_object::<GetContextInput>(),
    )
}

/// `kronika_get_instance`, split from the other entry tools to satisfy
/// the line-count lint.
fn instance_tool() -> Tool {
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
             microseconds, null for a part never recorded. `settings_scope` \
             states the applied selection, `settings_returned_count` is its \
             decimal-string row count, `settings_defaults_omitted` says \
             whether exact `source=default` rows were excluded, and \
             `settings_request_all` is the arguments object for requesting \
             all settings. Settings selection is complete or the call fails; \
             it is never cut to a row prefix. Settings rows carry `row_key` (the parameter \
             `name`) and decimal-string locator fields accepted unchanged by \
             kronika_get_row_detail; the single host-facts row is pinned by \
             its `at` alone.",
        schema_object::<GetInstanceInput>(),
    )
    .with_raw_output_schema(output_schema_object::<InstanceOutput>())
}

/// The table and index tools, kept separate to satisfy the line-count lint.
fn relation_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_TABLES_TOOL,
            "Finds stored PostgreSQL table statistics at optional `at`, or at \
             the latest recorded point when omitted, with each compatible \
             layout contributing its latest usable observation no later than \
             that point. It does not query PostgreSQL. Filters are \
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
             unavailable metrics are null. Returns `{rows, truncated, as_of}`. \
             `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted; no \
             continuation cursor is returned. Aggregated relation rows have no physical locator and \
             cannot be passed to `kronika_get_row_detail`.",
            schema_object::<TablesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_INDEXES_TOOL,
            "Finds stored PostgreSQL index statistics at optional `at`, or at \
             the latest recorded point when omitted, with each compatible \
             layout contributing its latest usable observation no later than \
             that point. It does not query PostgreSQL. Filters are \
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
             Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted; no continuation cursor is returned. Aggregated relation rows have \
             no physical locator and cannot be passed to \
             `kronika_get_row_detail`.",
            schema_object::<IndexesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
    ]
}

/// The process, row-detail, and event tools, kept separate to satisfy the
/// line-count lint.
fn tail_tools() -> [Tool; 3] {
    [
        Tool::new(
            FIND_PROCESSES_TOOL,
            "Finds stored Linux process observations at optional `at`, or at \
             the latest recorded point when omitted, with each compatible \
             layout contributing its latest usable observation no later than \
             that point. It does not query the OS. Filters are ANDed \
             before sorting and `limit`; omitted filters match all. Returned \
             `rmem_kb`, `vmem_kb`, and `vswap_kb` are KiB; `utime` and `stime` \
             are jiffies/s; `rundelay_ns` is ns/s; `blkdelay_ticks` is \
             jiffies/s; fault, context-switch, and syscall counters are \
             count/s; I/O counters are bytes/s. Rates are null without a \
             usable predecessor or across a PID start-time change. \
             Unrecorded `/proc/PID/io` fields are null. Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. Each row \
             has decimal-string `segment_id`, `type_id`, `row_ordinal`, and \
             `at` accepted unchanged by `kronika_get_row_detail`.",
            schema_object::<ProcessesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            GET_ROW_DETAIL_TOOL,
            "Returns the rendered row addressed by exact physical locator \
             (`section`, `segment_id`, `at`, `type_id`, `row_ordinal`) from any \
             stored segment. Locator fields accept JSON integers or decimal \
             strings; copy the strings from a `find_*` row to avoid precision \
             loss. `row_key` is the row's identifying value, copied verbatim \
             from the find row: `queryid` (pg_stat_statements), `planid` \
             (pg_store_plans), `pid` (pg_stat_activity, \
             pg_stat_progress_vacuum, pg_locks, pg_log_lock_waits, \
             os_process), `datid` (pg_stat_database), `name` (pg_settings), \
             `pattern` (pg_log_errors, pg_log_slow_queries), `phase` \
             (pg_log_checkpoints), `relation` (pg_log_autovacuum), \
             `size_bytes` (pg_log_temp_files), `kind` (pg_log_lifecycle), \
             `text` (pgbouncer_events). Rows keep their ordinal only while a \
             segment stays active; after finalization the same ordinal can \
             hold another row, and the `row_key` check answers that with an \
             error. The check pins the object, not its binding: columns like \
             `dbid`, `userid`, or `toplevel` are reported in the row — \
             confirm them there. A row whose identifying column is null \
             carries no `row_key`; omit it then. Find rows without a \
             `row_key` come from sections keeping one row per timestamp, \
             pinned by `at` alone. `pg_store_plans.calls` remains the exact stored count and \
             adds `calls_per_second`; other cumulative columns are rendered as \
             interval rates rather than stored counter values. Missing, \
             unavailable, or underivable values are null. Plain timestamped \
             sections, including event sections, are supported; relation-grouped \
             `pg_stat_user_tables` and `pg_stat_user_indexes` are rejected. A \
             missing or mismatched locator returns an error. Recognized event \
             codes receive the same `<field>_label` siblings as \
             `kronika_find_events`; the find-only `source` field is not part of \
             the stored row and is not returned here. Text values under 4 KiB \
             arrive as plain strings; from 4 KiB up they arrive as \
             `{stored_text, full_len, truncated, sha256}`, where `truncated` \
             marks the cut at the storage's own text ceiling (1 MiB by \
             default). A marker such as \
             `<truncated>` inside the text itself is the source's cut — \
             pg_store_plans.max_plan_length trimmed the plan before Kronika \
             saw it — and leaves `truncated` false.",
            schema_object::<RowDetailInput>(),
        ),
        Tool::new(
            FIND_EVENTS_TOOL,
            "Reads stored PostgreSQL and PgBouncer events in half-open \
             `[from,to)` without a live query. `groups` (default) returns the \
             same ordered server-grouped entries as the web Events console; \
             `occurrences` returns raw stored rows, labels, row keys, and exact \
             decimal-string locators. Sources are ordered, repeated names are \
             deduplicated, and an empty array returns no items. `limit` is \
             applied after the complete merge or grouping. Returns a tagged \
             `{representation, groups|occurrences, truncated}` result without \
             a continuation cursor. Times accept JSON integer or canonical \
             signed decimal-string i64 Unix microseconds, RFC 3339 with a timezone, \
             `now`, and `now-N{us,ms,s,m,h,d,w}`.",
            schema_object::<EventsInput>(),
        )
        .with_output_schema::<EventsResult>(),
    ]
}

/// Plain `PostgreSQL` tools, kept separate to satisfy the line-count lint.
fn postgresql_plain_tools() -> [Tool; 4] {
    [
        Tool::new(
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            "Finds stored PostgreSQL backend activity, state, waits, query \
             text, and transaction-age fields at optional `at`, or at the \
             latest recorded point when omitted. Each compatible layout \
             contributes its latest usable observation no later than that point, \
             so layouts may contribute different `at` values. It does not \
             query PostgreSQL. Filters are ANDed before sorting and `limit`; \
             omitted filters match all. XID ages are counts; `backend_start`, \
             `xact_start`, `query_start`, and `state_change` are Unix \
             microseconds. Null denotes no transaction, query, or wait where \
             applicable, a null recording, or a field absent from the physical \
             layout. Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. Each row has decimal-string `segment_id`, \
             `type_id`, `row_ordinal`, and `at` accepted unchanged by \
             `kronika_get_row_detail`.",
            schema_object::<ActivityInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_LOCKS_TOOL,
            "Finds stored PostgreSQL backends in a direct lock-wait graph at \
             optional `at`, or at the latest recorded point when omitted. Each \
             compatible layout contributes its latest usable observation no \
             later than that point; it does not query PostgreSQL. \
             Filters are ANDed before sorting and `limit`; omitted filters \
             match all. `blocked_by` is always a list of direct blocker PIDs; \
             an empty list marks a root or blocker-only row, and PID 0 denotes \
             a prepared-transaction holder. Timestamp fields are Unix \
             microseconds. Null denotes an inapplicable, null, or unavailable \
             field. The wait graph is recorded only while contention \
             exists, so the anchor can predate the present and the rows can \
             describe contention that has since ended. Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. Each row has \
             decimal-string `segment_id`, `type_id`, `row_ordinal`, and `at` \
             accepted unchanged by `kronika_get_row_detail`.",
            schema_object::<LocksInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_VACUUM_TOOL,
            "Finds PostgreSQL backends recorded as running `VACUUM` at optional \
             `at`, or at the latest recorded point when omitted. Each compatible \
             layout contributes its latest usable observation no later than that \
             point; it does not query PostgreSQL. \
             Filters are ANDed before sorting and `limit`; omitted filters \
             match all. Heap, index, and dead-item fields are counts; \
             `dead_tuple_bytes` and `max_dead_tuple_bytes` are bytes; \
             `delay_time` is milliseconds. Version-specific unavailable \
             fields are null. The section is recorded only while a vacuum \
             runs: empty rows with null `as_of` mean no usable observation \
             was selected for the requested point, and the anchor can point \
             at the last vacuum, not the present. \
             Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. Each row has decimal-string `segment_id`, `type_id`, \
             `row_ordinal`, and `at` accepted unchanged by \
             `kronika_get_row_detail`.",
            schema_object::<VacuumInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_DATABASES_TOOL,
            "Finds stored per-database PostgreSQL statistics at optional `at`, \
             or at the latest recorded point when omitted. Each compatible \
             layout contributes its latest usable observation no later than \
             that point; it does not query PostgreSQL. Filters are ANDed \
             before sorting and `limit`; omitted filters match all. \
             `numbackends` is a recorded count. Cumulative count fields are \
             returned as count/s, `temp_bytes` as bytes/s, and cumulative time \
             fields as ms/s. `stats_reset` and `checksum_last_failure` are Unix \
             microseconds. Interval rates are null without a usable predecessor \
             or after counter rollback; absent or null fields are null. Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. \
             Each row has decimal-string locator fields accepted unchanged \
             by `kronika_get_row_detail`.",
            schema_object::<DatabasesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
    ]
}

/// The statement and plan tools, kept separate to satisfy the line-count
/// lint.
fn ratio_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_STATEMENTS_TOOL,
            "Finds stored `pg_stat_statements` rows at optional `at`, or at the \
             latest recorded point when omitted, with the latest usable \
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
             rollback. Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. Locator fields are decimal strings accepted \
             by `kronika_get_row_detail`.",
            schema_object::<StatementsInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_PLANS_TOOL,
            "Finds stored `pg_store_plans` rows at optional `at`, or at the \
             latest recorded point when omitted, with the latest usable \
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
             non-finite result, missing predecessor, or rollback. Returns `{rows, truncated, as_of}`. `as_of`: Decimal-string timestamp of the nearest usable observation found no later than the requested point; it may be earlier than the requested time. It is null only when no usable observation was selected. Pass it unchanged to an MCP `from`, `to`, or `at` input. `truncated` means matching rows were omitted, and no continuation \
             cursor is returned. \
             Locator fields are decimal strings accepted by \
             `kronika_get_row_detail`.",
            schema_object::<PlansInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
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

/// Output schemas describe serialization, so nullable fields that are always
/// emitted remain required even though they accept JSON null.
fn output_schema_object<T: JsonSchema>() -> Arc<JsonObject> {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let value = serde_json::to_value(schema).expect("schema serializes to JSON");
    let object = value.as_object().expect("schema is a JSON object").clone();
    Arc::new(object)
}

#[cfg(test)]
mod tests;
