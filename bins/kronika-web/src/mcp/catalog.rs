//! Tool names, descriptions, and input schemas returned by `tools/list`.

use std::sync::Arc;

use rmcp::model::{JsonObject, Tool};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

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
    /// One recorded logical section name; omit for every recorded section.
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
    /// rejected; a plain tool retains identity order for a known column the
    /// selected rows do not expose.
    pub(crate) field: String,
    /// `asc` puts the lowest non-null value first; `desc` puts the highest.
    /// Nulls remain last.
    pub(crate) direction: DirectionInput,
}

/// Input for `kronika_find_postgresql_tables`.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct TablesInput {
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Returned field, such as `pid`, `datname`, `state`,
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
    #[serde(default)]
    pub(crate) at: Option<TimeSpecInput>,
    /// AND-only predicates. Text fields (`eq`, `in`, or `contains`): `text`,
    /// `database`, `role`, `state`, `lock_type`, `lock_mode`, `table_name`.
    /// Identifier field (`eq` or `in`): `pid`. Empty or omitted matches all
    /// rows.
    #[serde(default)]
    #[schemars(length(max = SEARCH_MAX_CLAUSES))]
    pub(crate) filters: Vec<FilterInput>,
    /// Returned scalar field, such as `pid`, `datname`, `state`,
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Returned field, such as `pid`, `datname`, `schemaname`,
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Returned field, such as `datid`, `datname`, `numbackends`,
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Returned field, such as `calls`, `total_exec_time`, `rows`,
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Returned field, such as `calls`, `total_time`, `rows`, or
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
    /// Omit to use the store-wide last recorded timestamp; otherwise select
    /// at this point.
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
    /// Returned field, such as `pid`, `comm`, `rmem_kb`, `vmem_kb`,
    /// `num_threads`, `utime`, `read_bytes`, or `rundelay_ns`. Filter aliases
    /// (`rss`, `vsz`, `threads`, and rate names) and virtual fields are not
    /// sort aliases; unknown names are rejected. Omit for identity order.
    #[serde(default)]
    pub(crate) sort: Option<SortInput>,
    /// Maximum rows to return, from 1 through 5,000.
    #[schemars(range(min = 1, max = MAX_SNAPSHOT_PAGE_SIZE))]
    pub(crate) limit: u32,
}

/// Opaque reference emitted by a result that supports full row detail.
#[derive(Debug, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct RowDetailInput {
    /// Copy one server-produced `detail_ref` string unchanged.
    #[schemars(length(
        min = 1,
        max = crate::api::row_key::DETAIL_REF_MAX_ENCODED_BYTES
    ))]
    pub(crate) detail_ref: String,
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
    /// Server-grouped console entries (default) or compact stored occurrences.
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
         maxima. Every entity includes its named product identity, compact automatic \
         labels when available in stored data, and a server-produced `detail_ref` \
         string to copy unchanged into `kronika_get_row_detail`; \
         an unavailable label is null. Query text, plans, command lines, and log \
         payloads are returned only by `kronika_get_row_detail`. Coverage reports \
         data/no_data state and the in-window row count. Timestamps, identifiers, \
         and counts that may exceed safe JSON \
         integer precision are decimal strings. A request-budget refusal names \
         `ranking_index`. `top` bounds returned identities; \
         it does not change pre-ranking scan state.",
        schema_object::<OverviewInput>(),
    )
    .with_raw_output_schema(opaque_output_schema::<HeatmapBatchResult>(true))
}

fn context_tool() -> Tool {
    Tool::new(
        GET_CONTEXT_TOOL,
        "Lists logical product sections found across all stored segments; \
         it does not inspect the live host or database. Top-level \
         `recorded_from` is the inclusive first stored timestamp and \
         `recorded_to` is the exclusive upper bound one microsecond after the \
         last stored timestamp. Both are decimal-string Unix microseconds \
         accepted unchanged by MCP `from`, `to`, and `at` inputs — an \
         empty answer elsewhere may just mean the window fell outside \
         them. Each `sections` item contains `logical_name`, `source_family`, \
         decimal-string `rows` and `bytes`, and `fields` — every public \
         column's `name`, `class`, and `unit`. A `cumulative` column is a monotonic \
         counter that `find_*` rows return as a per-second rate under \
         the raw column name and `kronika_overview` ranks as a \
         whole-window delta; a `gauge` is an instantaneous value. \
         `source_family` may be null. Rows and bytes are summed for each \
         logical product section across stored data. Internal-only sections \
         are omitted. The full answer \
         is tens of kilobytes — read it once per session, or pass \
         `section` to keep one logical section only.",
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
             it is never cut to a row prefix. Every host or settings row carries \
             one server-produced `detail_ref` string to copy unchanged into \
             `kronika_get_row_detail`.",
        schema_object::<GetInstanceInput>(),
    )
    .with_raw_output_schema(output_schema_object::<InstanceOutput>())
}

/// The table and index tools, kept separate to satisfy the line-count lint.
fn relation_tools() -> [Tool; 2] {
    [
        Tool::new(
            FIND_POSTGRESQL_TABLES_TOOL,
            "Finds stored PostgreSQL table statistics at optional `at`; when \
             omitted, `at` is the store-wide last recorded timestamp. It does \
             not query PostgreSQL. Filters are \
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
             unavailable metrics are null. Returns `{rows, truncated}`. \
             `truncated` means matching rows were omitted by `limit`; no \
             continuation cursor is returned. Aggregated relation rows do not include \
             `detail_ref` and cannot be passed to `kronika_get_row_detail`.",
            schema_object::<TablesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_INDEXES_TOOL,
            "Finds stored PostgreSQL index statistics at optional `at`; when \
             omitted, `at` is the store-wide last recorded timestamp. It does \
             not query PostgreSQL. Filters are \
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
             Returns `{rows, truncated}`. `truncated` means matching rows were \
             omitted by `limit`; no continuation cursor is returned. Aggregated relation rows do \
             not include `detail_ref` and cannot be passed to \
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
            "Finds stored Linux process observations at optional `at`; when \
             omitted, `at` is the store-wide last recorded timestamp. It does \
             not query the OS. Filters are ANDed \
             before sorting and `limit`; omitted filters match all. Returned \
             `rmem_kb`, `vmem_kb`, and `vswap_kb` are KiB; `utime` and `stime` \
             are jiffies/s; `rundelay_ns` is ns/s; `blkdelay_ticks` is \
             jiffies/s; fault, context-switch, and syscall counters are \
             count/s; I/O counters are bytes/s. Rates are null without a \
             usable predecessor or across a PID start-time change. \
             Unrecorded `/proc/PID/io` fields are null. Returns `{rows, truncated}`. \
             `truncated` means matching rows were omitted by `limit`, and no continuation \
             cursor is returned. Command lines are available only through \
             `kronika_get_row_detail`; copy a row's `detail_ref` string \
             unchanged into that tool.",
            schema_object::<ProcessesInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            GET_ROW_DETAIL_TOOL,
            "Returns the full rendered stored row addressed by a server-produced \
             `detail_ref` from a mass result. Pass only that string as \
             `{detail_ref: string}` and copy it unchanged; never construct, \
             inspect, guess, or modify it. The reference remains valid when \
             active recorded data is finalized and reordered. An invalid, stale, \
             or ambiguous reference is rejected and can never select a different row. \
             `pg_store_plans.calls` remains the exact stored count and \
             adds `calls_per_second`; other cumulative columns are rendered as \
             interval rates rather than stored counter values. Missing, \
             unavailable, or underivable values are null. Plain timestamped \
             sections, event sections, and individual relation rows from Overview \
             are supported; aggregated relation finder rows have no `detail_ref`. \
             Recognized event \
             codes receive the same `<field>_label` siblings as \
             `kronika_find_events`; the find-only `source` field is not part of \
             the stored row and is not returned here. Every designated long-text \
             field has the stable `{stored_text, full_len, truncated, sha256}` \
             shape regardless of stored length. `truncated` \
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
             same ordered server-grouped entries as the web Events console. \
             Groups contain structural summaries, a short bounded label when \
             available, and one `detail_ref` string, but no raw text or nested \
             source rows. `occurrences` returns structural fields, known code \
             labels, and a `detail_ref` string, with raw payloads available only \
             through `kronika_get_row_detail`. Sources are ordered, repeated names are \
             deduplicated, and an empty array returns no items. `limit` is \
             applied after the complete merge or grouping. Returns a tagged \
             `{representation, groups|occurrences, truncated}` result without \
             a continuation cursor. Times accept JSON integer or canonical \
             signed decimal-string i64 Unix microseconds, RFC 3339 with a timezone, \
             `now`, and `now-N{us,ms,s,m,h,d,w}`.",
            schema_object::<EventsInput>(),
        )
        .with_raw_output_schema(opaque_output_schema::<EventsResult>(false)),
    ]
}

/// Plain `PostgreSQL` tools, kept separate to satisfy the line-count lint.
fn postgresql_plain_tools() -> [Tool; 4] {
    [
        Tool::new(
            FIND_POSTGRESQL_ACTIVITY_TOOL,
            "Finds stored PostgreSQL backend activity, state, waits, query IDs, \
             and transaction-age fields at optional `at`; when omitted, \
             `at` is the store-wide last recorded timestamp. It does not \
             query PostgreSQL. Filters are ANDed before sorting and `limit`; \
             omitted filters match all. XID ages are counts; `backend_start`, \
             `xact_start`, `query_start`, and `state_change` are Unix \
             microseconds. Null denotes no transaction, query, or wait where \
             applicable, a null recording, or a field the source did not record. \
             Returns `{rows, truncated}`. `truncated` means matching rows \
             were omitted by `limit`, and no continuation \
             cursor is returned. Query text is available only through \
             `kronika_get_row_detail`; copy a row's `detail_ref` string \
             unchanged into that tool.",
            schema_object::<ActivityInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_LOCKS_TOOL,
            "Finds stored PostgreSQL backends in a direct lock-wait graph at \
             optional `at`; when omitted, `at` is the store-wide last recorded \
             timestamp. It does not query PostgreSQL. \
             Filters are ANDed before sorting and `limit`; omitted filters \
             match all. `blocked_by` is always a list of direct blocker PIDs; \
             an empty list marks a root or blocker-only row, and PID 0 denotes \
             a prepared-transaction holder. Timestamp fields are Unix \
             microseconds. Null denotes an inapplicable, null, or unavailable \
             field. Returns `{rows, truncated}`. `truncated` means matching \
             rows were omitted by `limit`, and no continuation \
             cursor is returned. Query text is available only through \
             `kronika_get_row_detail`; copy a row's `detail_ref` string \
             unchanged into that tool.",
            schema_object::<LocksInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_VACUUM_TOOL,
            "Finds PostgreSQL backends recorded as running `VACUUM` at optional \
             `at`; when omitted, `at` is the store-wide last recorded timestamp. \
             It does not query PostgreSQL. \
             Filters are ANDed before sorting and `limit`; omitted filters \
             match all. Heap, index, and dead-item fields are counts; \
             `dead_tuple_bytes` and `max_dead_tuple_bytes` are bytes; \
             `delay_time` is milliseconds. Version-specific unavailable \
             fields are null. Empty rows mean no vacuum observation exists \
             in the internal selection window. Returns `{rows, truncated}`. \
             `truncated` means matching rows were omitted by `limit`, and no continuation \
             cursor is returned. Copy a row's `detail_ref` string unchanged into \
             `kronika_get_row_detail`.",
            schema_object::<VacuumInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_DATABASES_TOOL,
            "Finds stored per-database PostgreSQL statistics at optional `at`; \
             when omitted, `at` is the store-wide last recorded timestamp. It \
             does not query PostgreSQL. Filters are ANDed \
             before sorting and `limit`; omitted filters match all. \
             `numbackends` is a recorded count. Cumulative count fields are \
             returned as count/s, `temp_bytes` as bytes/s, and cumulative time \
             fields as ms/s. `stats_reset` and `checksum_last_failure` are Unix \
             microseconds. Interval rates are null without a usable predecessor \
             or after counter rollback; absent or null fields are null. Returns \
             `{rows, truncated}`. `truncated` means matching rows were omitted by \
             `limit`, and no continuation \
             cursor is returned. Copy a row's `detail_ref` string unchanged into \
             `kronika_get_row_detail`.",
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
            "Finds stored `pg_stat_statements` rows at optional `at`; when \
             omitted, `at` is the store-wide last recorded timestamp. It does \
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
             rollback. Returns `{rows, truncated}`. `truncated` means matching \
             rows were omitted by `limit`, and no continuation \
             cursor is returned. Query text is available only through \
             `kronika_get_row_detail`; copy a row's `detail_ref` string \
             unchanged into that tool.",
            schema_object::<StatementsInput>(),
        )
        .with_raw_output_schema(output_schema_object::<FinderOutput>()),
        Tool::new(
            FIND_POSTGRESQL_PLANS_TOOL,
            "Finds stored `pg_store_plans` rows at optional `at`; when omitted, \
             `at` is the store-wide last recorded timestamp. It does \
             not query PostgreSQL. Filters are ANDed before sorting and \
             `limit`. `calls` is the exact cumulative count and \
             `calls_per_second` its interval rate; other cumulative fields are \
             returned as count/s, PostgreSQL blocks/s, or ms/s. Seven added \
             fields use the statement formulas: mean execution ms/call, \
             rows/call, shared-plus-local blocks/call, shared hit fraction \
             (0..1), WAL bytes/call, planning fraction (0..1), and execution \
             coefficient of variation. `derived_wal_per_call` is always null \
             because stored plan rows do not carry WAL bytes. \
             `derived_plan_time_fraction` is non-null only when planning counters \
             were recorded. \
             Other derived nulls mean a missing/null operand, zero denominator, \
             non-finite result, missing predecessor, or rollback. Returns \
             `{rows, truncated}`. `truncated` means matching rows were omitted \
             by `limit`, and no continuation \
             cursor is returned. \
             Plan text is available only through `kronika_get_row_detail`; \
             copy a row's `detail_ref` string unchanged into that tool.",
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

/// Adapts shared HTTP schemas to the opaque MCP detail boundary.
fn opaque_output_schema<T: JsonSchema>(hide_layout: bool) -> Arc<JsonObject> {
    let generator = schemars::generate::SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    let schema = generator.into_root_schema_for::<T>();
    let mut value = serde_json::to_value(schema).expect("schema serializes to JSON");
    if let Some(definitions) = value.get_mut("$defs").and_then(Value::as_object_mut) {
        definitions.remove("DetailLocator");
    }
    rewrite_detail_schema(&mut value, hide_layout);
    let object = value.as_object_mut().expect("schema is a JSON object");
    object.insert("type".to_owned(), json!("object"));
    Arc::new(object.clone())
}

fn rewrite_detail_schema(value: &mut Value, hide_layout: bool) {
    match value {
        Value::Object(object) => {
            if object.get("format").and_then(Value::as_str) == Some("uint") {
                object.remove("format");
            }
            if let Some(properties) = object.get_mut("properties").and_then(Value::as_object_mut) {
                if properties.remove("detail_locator").is_some() {
                    properties.insert(
                        "detail_ref".to_owned(),
                        json!({
                            "description": "Opaque server-produced row-detail reference; copy it unchanged.",
                            "type": "string",
                        }),
                    );
                }
                if hide_layout {
                    properties.remove("type_id");
                }
            }
            if let Some(required) = object.get_mut("required").and_then(Value::as_array_mut) {
                for name in required.iter_mut() {
                    if name == "detail_locator" {
                        *name = json!("detail_ref");
                    }
                }
                if hide_layout {
                    required.retain(|name| name != "type_id");
                }
            }
            for child in object.values_mut() {
                rewrite_detail_schema(child, hide_layout);
            }
        }
        Value::Array(values) => {
            for child in values {
                rewrite_detail_schema(child, hide_layout);
            }
        }
        _ => {}
    }
}

#[cfg(test)]
mod tests;
