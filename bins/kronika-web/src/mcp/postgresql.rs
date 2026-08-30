//! MCP adapters over recorded `PostgreSQL` finder results.

use kronika_reader::Reader;
use rmcp::model::CallToolResult;
use serde_json::{Map, Value, json};

use crate::api::snapshot;
use crate::api::snapshot::PlainRowOut;
use crate::api::snapshot::relation::{RelationKind, RelationRow};
use crate::api::snapshot::selector::{
    FinderOrder, FinderQuery, FinderResult, FinderSurface, execute_plain, execute_relation,
};
use crate::api::time::SnapshotPoint;
use crate::api::{ApiError, Prepared};
use crate::config::Config;
use crate::route::{MAX_SNAPSHOT_PAGE_SIZE, Order, RelationGroup, SnapshotRequest};

use super::catalog::{
    ActivityInput, DatabasesInput, FIND_POSTGRESQL_ACTIVITY_TOOL, FIND_POSTGRESQL_DATABASES_TOOL,
    FIND_POSTGRESQL_INDEXES_TOOL, FIND_POSTGRESQL_LOCKS_TOOL, FIND_POSTGRESQL_PLANS_TOOL,
    FIND_POSTGRESQL_STATEMENTS_TOOL, FIND_POSTGRESQL_TABLES_TOOL, FIND_POSTGRESQL_VACUUM_TOOL,
    IndexesInput, LocksInput, PlansInput, SortInput, StatementsInput, TablesInput, VacuumInput,
};
use super::filter::{FilterInput, build_search};
use super::semantics::{
    bounded_limit, finder_output, finder_storage_error, finder_summary, mcp_error, mcp_structured,
};
use super::time::{TimeSpecInput, resolve_point};

pub(crate) fn call_tables(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: TablesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_TABLES_TOOL,
                "group and limit are required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let group = input.group.into();
    let query = match finder_query(
        FIND_POSTGRESQL_TABLES_TOOL,
        FinderSurface::Tables,
        Some(group),
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call(RelationKind::Tables, config, group, query, cancelled)
}

pub(crate) fn call_indexes(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: IndexesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_INDEXES_TOOL,
                "group and limit are required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let group = input.group.into();
    let query = match finder_query(
        FIND_POSTGRESQL_INDEXES_TOOL,
        FinderSurface::Indexes,
        Some(group),
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call(RelationKind::Indexes, config, group, query, cancelled)
}

fn call(
    kind: RelationKind,
    config: &Config,
    group: RelationGroup,
    query: FinderQuery,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let result = match execute_relation(&config.data_root, query, &|| cancelled()) {
        Ok(result) => result,
        Err(error) => return finder_storage_error(kind.logical_name(), &error),
    };

    let row_count = result.rows.len();
    let rows: Vec<Value> = result
        .rows
        .into_iter()
        .map(|row| row_to_json(row, kind, group))
        .collect();
    let summary = finder_summary(kind.logical_name(), row_count, result.truncated);
    let output = match finder_output(rows, result.truncated) {
        Ok(output) => output,
        Err(error) => return error,
    };
    mcp_structured(output, summary)
}

fn finder_point(tool: &str, at: Option<&TimeSpecInput>) -> Result<SnapshotPoint, CallToolResult> {
    resolve_point(at).map_err(|error| {
        super::semantics::invalid_arguments(
            tool,
            "at is optional; group/limit and the documented finder fields keep their current shape",
            error,
        )
    })
}

fn finder_query(
    tool: &str,
    surface: FinderSurface,
    group: Option<RelationGroup>,
    at: Option<&TimeSpecInput>,
    filters: &[FilterInput],
    sort: Option<SortInput>,
    limit: u32,
) -> Result<FinderQuery, CallToolResult> {
    let limit = bounded_limit("limit", limit, MAX_SNAPSHOT_PAGE_SIZE)?;
    let search = build_search(surface.logical_name(), filters)
        .map_err(super::filter::Refusal::into_error)?;
    Ok(FinderQuery {
        surface,
        point: finder_point(tool, at)?,
        search,
        order: sort.map(|sort| FinderOrder {
            field: sort.field,
            direction: sort.direction.into(),
        }),
        group,
        limit,
    })
}

/// Returns the highest-ID segment carrying `logical_name` and its maximum
/// timestamp, or `None` when nothing recorded the section. The instance tool
/// uses this legacy snapshot anchor; finder tools use the shared selector.
pub(crate) fn current_segment(
    root: &std::path::Path,
    logical_name: &str,
) -> Result<Option<(i64, i64)>, ApiError> {
    let reader = Reader::open(root)?;
    let listing = reader.catalog_segments(..)?;
    let segment = listing
        .segments
        .iter()
        .filter(|segment| {
            segment.sections().iter().any(|section| {
                kronika_registry::logical_section_name(section.type_id) == Some(logical_name)
            })
        })
        .max_by_key(|segment| segment.id());
    Ok(segment.map(|segment| (segment.id(), segment.max_ts())))
}

/// Flattens metrics and group identity; identity fields win name collisions.
fn row_to_json(row: RelationRow, kind: RelationKind, group: RelationGroup) -> Value {
    let mut object = Map::new();
    for (name, metric) in row.metrics {
        object.insert(name, metric.map_or(Value::Null, |metric| metric.json()));
    }
    if let Value::Object(key_fields) = row.key.json(kind, group) {
        object.extend(key_fields);
    }
    Value::Object(object)
}

pub(crate) fn call_activity(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: ActivityInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_ACTIVITY_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_ACTIVITY_TOOL,
        FinderSurface::Activity,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, query, cancelled)
}

pub(crate) fn call_locks(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: LocksInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_LOCKS_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_LOCKS_TOOL,
        FinderSurface::Locks,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, query, cancelled)
}

pub(crate) fn call_vacuum(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: VacuumInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_VACUUM_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_VACUUM_TOOL,
        FinderSurface::Vacuum,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, query, cancelled)
}

pub(crate) fn call_databases(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: DatabasesInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_DATABASES_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_DATABASES_TOOL,
        FinderSurface::Databases,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, query, cancelled)
}

fn call_plain(config: &Config, query: FinderQuery, cancelled: &dyn Fn() -> bool) -> CallToolResult {
    let surface = query.surface;
    let result = match execute_plain(&config.data_root, query, &|| cancelled()) {
        Ok(result) => result,
        Err(error) => return finder_storage_error(surface.logical_name(), &error),
    };
    let row_count = result.rows.len();
    let rows: Vec<Value> = result
        .rows
        .into_iter()
        .map(|row| plain_row_to_json(surface.logical_name(), row))
        .collect();
    let summary = finder_summary(surface.logical_name(), row_count, result.truncated);
    let output = match finder_output(rows, result.truncated) {
        Ok(output) => output,
        Err(error) => return error,
    };
    mcp_structured(output, summary)
}

pub(super) fn plain_rows(
    logical_name: &str,
    config: &Config,
    cancelled: &dyn Fn() -> bool,
) -> Result<Option<FinderResult<PlainRowOut>>, CallToolResult> {
    let Some((segment_id, at)) = current_segment(&config.data_root, logical_name)
        .map_err(|error| super::semantics::storage_error(&error))?
    else {
        return Ok(None);
    };
    let request = SnapshotRequest {
        segment_id,
        at,
        sections: vec![logical_name.to_owned()],
        fields: Vec::new(),
        by: Vec::new(),
        direction: Order::Asc,
        group: None,
        page_size: None,
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
    };
    let prepared = snapshot::prepare(&config.data_root, request, None)
        .map_err(|error| super::semantics::storage_error(&error))?;
    let Prepared::Snapshot(prepared) = prepared else {
        return Err(mcp_error(
            "internal error: snapshot preparation returned an unexpected response type",
        ));
    };
    let result = prepared
        .compute_plain_rows(usize::MAX, &|| cancelled())
        .map_err(|error| super::semantics::storage_error(&error))?;
    Ok(Some(result))
}

pub(crate) fn call_statements(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: StatementsInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_STATEMENTS_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_STATEMENTS_TOOL,
        FinderSurface::Statements,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, query, cancelled)
}

pub(crate) fn call_plans(
    config: &Config,
    arguments: Map<String, Value>,
    cancelled: &dyn Fn() -> bool,
) -> CallToolResult {
    let input: PlansInput = match serde_json::from_value(Value::Object(arguments)) {
        Ok(input) => input,
        Err(error) => {
            return super::semantics::invalid_arguments(
                FIND_POSTGRESQL_PLANS_TOOL,
                "limit is required; at, filters, and sort are optional",
                error,
            );
        }
    };
    let query = match finder_query(
        FIND_POSTGRESQL_PLANS_TOOL,
        FinderSurface::Plans,
        None,
        input.at.as_ref(),
        &input.filters,
        input.sort,
        input.limit,
    ) {
        Ok(query) => query,
        Err(error) => return error,
    };
    call_plain(config, query, cancelled)
}

/// Flattens projected fields and appends `row_key` plus the decimal-string
/// locator fields accepted by `kronika_get_row_detail`.
pub(super) fn plain_row_to_json(logical_name: &str, row: PlainRowOut) -> Value {
    let mut object: Map<String, Value> = row.fields.into_iter().collect();
    crate::api::row_key::attach(logical_name, &mut object);
    object.insert("segment_id".to_owned(), json!(row.segment_id.to_string()));
    object.insert("type_id".to_owned(), json!(row.type_id.to_string()));
    object.insert("row_ordinal".to_owned(), json!(row.row_ordinal.to_string()));
    object.insert("at".to_owned(), json!(row.at.to_string()));
    Value::Object(object)
}
