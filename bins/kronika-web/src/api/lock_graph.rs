//! Bounded whole-snapshot `PostgreSQL` lock graph shared by HTTP and MCP.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::Path;

use hyper::StatusCode;
use kronika_reader::SegmentKind;
use serde_json::{Value, json};

use super::{ApiError, CachePolicy, Prepared, ProductError, ResponseMeta};
use crate::route::{Order, SnapshotRequest};

pub(crate) const MAX_LOCK_GRAPH_ROWS: usize = 500;

const REQUIRED_FIELDS: [&str; 2] = ["pid", "blocked_by"];
const DERIVED_FIELDS: [&str; 5] = [
    "lock_tree_parent_pid",
    "lock_tree_depth",
    "lock_tree_order",
    "lock_tree_extra_blockers",
    "lock_tree_waits_on_prepared",
];

pub(crate) struct PreparedLockGraph {
    complete: Box<Prepared>,
    matched: Option<Box<Prepared>>,
    admission_limit: usize,
    meta: ResponseMeta,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct Locator {
    segment_id: String,
    type_id: String,
    ordinal: String,
}

#[derive(Debug, Clone, Copy)]
struct Layout {
    pid: usize,
    blocked_by: usize,
}

struct Node {
    pid: i64,
    blockers: Vec<i64>,
    waits_on_prepared: bool,
    locator: Locator,
    row: Value,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Placement {
    pid: i64,
    parent_pid: Option<i64>,
    depth: usize,
    extra_blockers: Vec<i64>,
}

pub(super) fn prepare(
    root: &Path,
    request: SnapshotRequest,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let original = request.clone();
    let (mut complete_request, matched_request, admission_limit) = normalize(request)?;
    let (_reader, current, segments) =
        super::explicit_segment_with_listing(root, complete_request.segment_id)?;
    if complete_request.active_position.is_none() {
        complete_request.active_position = current.active_position();
    }
    let matched_request = matched_request.map(|mut request| {
        request.active_position = complete_request.active_position;
        request
    });
    let candidates =
        std::iter::once(&current).chain(segments.iter().filter(|candidate| {
            candidate.id() < current.id() && candidate.min_ts() <= original.at
        }));
    let meta = ResponseMeta::ok_with_etag(
        match current.kind() {
            SegmentKind::Finished => CachePolicy::Revalidate,
            SegmentKind::Active => CachePolicy::NoStore,
        },
        super::weak_etag("lock_graph", &format!("{original:?}"), candidates),
    );
    let complete = Box::new(super::snapshot::prepare(root, complete_request, None)?);
    let matched = matched_request
        .map(|request| super::snapshot::prepare(root, request, None).map(Box::new))
        .transpose()?;
    let concrete_validator = if_none_match.filter(|offered| offered.trim() != "*");
    if let Some(not_modified) = super::conditional_not_modified(meta.clone(), concrete_validator) {
        return Ok(not_modified);
    }
    Ok(Prepared::LockGraph(PreparedLockGraph {
        complete,
        matched,
        admission_limit,
        meta,
    }))
}

fn normalize(
    mut request: SnapshotRequest,
) -> Result<(SnapshotRequest, Option<SnapshotRequest>, usize), ApiError> {
    if request.sections.as_slice() != ["pg_locks"] {
        return Err(ApiError::NoSuchSection);
    }
    if request.cursor.is_some() {
        return Err(product_input(
            "cursor",
            "The PostgreSQL lock graph is a bounded whole-snapshot result and does not accept a cursor.",
        ));
    }
    let admission_limit = request.page_size.unwrap_or(MAX_LOCK_GRAPH_ROWS);
    if admission_limit == 0 || admission_limit > MAX_LOCK_GRAPH_ROWS {
        return Err(product_input(
            "page_size",
            "The PostgreSQL lock graph admission limit must be between 1 and 500 rows.",
        ));
    }
    let explicit_projection = !request.fields.is_empty();
    request
        .fields
        .retain(|field| !DERIVED_FIELDS.contains(&field.as_str()));
    if explicit_projection {
        for required in REQUIRED_FIELDS {
            if !request.fields.iter().any(|field| field == required) {
                request.fields.push(required.to_owned());
            }
        }
    }
    request.by = vec!["pid".to_owned()];
    request.direction = Order::Asc;
    request.page_size = None;
    request.postgresql = None;
    let search = request.search.take();
    let matched = search.map(|search| {
        let mut matched = request.clone();
        matched.search = Some(search);
        matched.page_size = Some(MAX_LOCK_GRAPH_ROWS);
        matched
    });
    Ok((request, matched, admission_limit))
}

impl PreparedLockGraph {
    pub(super) fn meta(&self) -> ResponseMeta {
        self.meta.clone()
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let complete = collect(*self.complete, self.admission_limit, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let matched = self
            .matched
            .map(|prepared| collect(*prepared, MAX_LOCK_GRAPH_ROWS, cancelled))
            .transpose()?;
        if cancelled() {
            return Ok(());
        }
        for record in transform(complete, matched.as_deref())? {
            if cancelled() || !emit(record) {
                break;
            }
        }
        Ok(())
    }
}

fn collect(
    prepared: Prepared,
    admission_limit: usize,
    cancelled: &impl Fn() -> bool,
) -> Result<Vec<Value>, ApiError> {
    let mut records = Vec::new();
    let mut rows = 0_usize;
    let mut exceeded = false;
    prepared.stream_values(
        &mut |record| {
            if cancelled() {
                return false;
            }
            if record.get("record").and_then(Value::as_str) == Some("row") {
                rows = rows.saturating_add(1);
                if rows > admission_limit {
                    exceeded = true;
                    return false;
                }
            }
            records.push(record);
            true
        },
        cancelled,
    )?;
    if exceeded {
        return Err(product_bounded(
            "lock_graph_bound_exceeded",
            format!(
                "The PostgreSQL lock snapshot exceeds the {admission_limit}-row graph admission limit."
            ),
            Some("page_size"),
        ));
    }
    Ok(records)
}

fn transform(
    records: Vec<Value>,
    matched_records: Option<&[Value]>,
) -> Result<Vec<Value>, ApiError> {
    let layouts = layouts(&records)?;
    let mut metadata = Vec::new();
    let mut nodes = BTreeMap::new();
    let mut locators = HashMap::new();
    for record in records {
        if record.get("record").and_then(Value::as_str) != Some("row") {
            if record.get("record").and_then(Value::as_str) != Some("snapshot_page") {
                metadata.push(record);
            }
            continue;
        }
        let node = node(record, &layouts)?;
        if nodes.contains_key(&node.pid) {
            return Err(product_internal(
                "lock_graph_identity_conflict",
                format!(
                    "The PostgreSQL lock snapshot contains more than one row for PID {}.",
                    node.pid
                ),
            ));
        }
        locators.insert(node.locator.clone(), node.pid);
        nodes.insert(node.pid, node);
    }
    if nodes.len() > MAX_LOCK_GRAPH_ROWS {
        return Err(product_bounded(
            "lock_graph_bound_exceeded",
            "The PostgreSQL lock snapshot exceeds the 500-row graph admission limit.",
            Some("page_size"),
        ));
    }

    let placements = placements(&nodes);
    let included = included_pids(matched_records, &locators, &placements)?;
    add_derived_layouts(&mut metadata);

    let mut rows = Vec::with_capacity(included.len());
    for (tree_order, placement) in placements.into_iter().enumerate() {
        if !included.contains(&placement.pid) {
            continue;
        }
        let mut node = nodes.remove(&placement.pid).ok_or_else(|| {
            product_internal(
                "lock_graph_unreadable",
                "The PostgreSQL lock graph lost a row while assigning its stable order.",
            )
        })?;
        let values = node
            .row
            .get_mut("values")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                product_internal(
                    "lock_graph_layout_unusable",
                    "A PostgreSQL lock row has no typed values array.",
                )
            })?;
        values.push(
            placement
                .parent_pid
                .map_or(Value::Null, |parent| json!(parent)),
        );
        values.push(json!(placement.depth));
        values.push(json!(tree_order));
        values.push(json!(placement.extra_blockers));
        values.push(json!(node.waits_on_prepared));
        rows.push(node.row);
    }
    let row_count = rows.len();
    let (from, to) = selected_range(&rows);
    metadata.extend(rows);
    metadata.push(json!({
        "record": "snapshot_page",
        "logical_name": "pg_locks",
        "eligible": row_count.to_string(),
        "returned": row_count.to_string(),
        "has_more": false,
        "truncated": false,
        "next_cursor": Value::Null,
        "page_size": MAX_LOCK_GRAPH_ROWS,
        "order_by": ["lock_tree_order"],
        "order_direction": "asc",
        "from": from,
        "to": to,
    }));
    Ok(metadata)
}

fn layouts(records: &[Value]) -> Result<HashMap<String, Layout>, ApiError> {
    let mut layouts = HashMap::new();
    for record in records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("layout"))
    {
        let Some(layout) = record.get("layout") else {
            continue;
        };
        if layout.get("logical_name").and_then(Value::as_str) != Some("pg_locks") {
            continue;
        }
        let Some(type_id) = layout.get("type_id").and_then(value_text) else {
            continue;
        };
        let columns = layout
            .get("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                product_internal(
                    "lock_graph_layout_unusable",
                    "A PostgreSQL lock layout has no typed columns array.",
                )
            })?;
        layouts.insert(
            type_id,
            Layout {
                pid: column(columns, "pid")?,
                blocked_by: column(columns, "blocked_by")?,
            },
        );
    }
    Ok(layouts)
}

fn column(columns: &[Value], name: &'static str) -> Result<usize, ApiError> {
    columns
        .iter()
        .position(|column| {
            column.get("name").and_then(Value::as_str) == Some(name)
                && column.get("available").and_then(Value::as_bool) != Some(false)
        })
        .ok_or_else(|| {
            product_internal(
                "lock_graph_layout_unusable",
                format!("The PostgreSQL lock graph requires the {name} field."),
            )
        })
}

fn node(row: Value, layouts: &HashMap<String, Layout>) -> Result<Node, ApiError> {
    let locator = locator(&row)?;
    let layout = layouts.get(&locator.type_id).ok_or_else(|| {
        product_internal(
            "lock_graph_layout_unusable",
            format!(
                "The PostgreSQL lock row type {} has no matching projected layout.",
                locator.type_id
            ),
        )
    })?;
    let values = row.get("values").and_then(Value::as_array).ok_or_else(|| {
        product_internal(
            "lock_graph_layout_unusable",
            "A PostgreSQL lock row has no typed values array.",
        )
    })?;
    let pid = decimal(values.get(layout.pid), "pid")?;
    let blocked_by = values
        .get(layout.blocked_by)
        .and_then(Value::as_array)
        .ok_or_else(|| {
            product_internal(
                "lock_graph_layout_unusable",
                "A PostgreSQL lock row has no blocked_by PID array.",
            )
        })?;
    let blocked_by = blocked_by
        .iter()
        .map(|value| decimal(Some(value), "blocked_by"))
        .collect::<Result<Vec<_>, _>>()?;
    let waits_on_prepared = blocked_by.contains(&0);
    Ok(Node {
        pid,
        blockers: blocked_by,
        waits_on_prepared,
        locator,
        row,
    })
}

fn locator(row: &Value) -> Result<Locator, ApiError> {
    let field = |name| {
        row.get(name).and_then(value_text).ok_or_else(|| {
            product_internal(
                "lock_graph_locator_unavailable",
                format!("A PostgreSQL lock row has no {name} locator."),
            )
        })
    };
    Ok(Locator {
        segment_id: field("segment_id")?,
        type_id: field("type_id")?,
        ordinal: field("ordinal")?,
    })
}

fn decimal(value: Option<&Value>, name: &'static str) -> Result<i64, ApiError> {
    value
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .ok_or_else(|| {
            product_internal(
                "lock_graph_layout_unusable",
                format!("A PostgreSQL lock row has no decimal {name} value."),
            )
        })
}

fn value_text(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Null | Value::Bool(_) | Value::Array(_) | Value::Object(_) => None,
    }
}

fn placements(nodes: &BTreeMap<i64, Node>) -> Vec<Placement> {
    let present = nodes.keys().copied().collect::<BTreeSet<_>>();
    let blockers = nodes
        .iter()
        .map(|(&pid, node)| {
            (
                pid,
                node.blockers
                    .iter()
                    .copied()
                    .filter(|blocker| *blocker != 0 && present.contains(blocker))
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let mut children = BTreeMap::<i64, Vec<i64>>::new();
    for (&pid, direct) in &blockers {
        for blocker in direct {
            children.entry(*blocker).or_default().push(pid);
        }
    }
    for direct in children.values_mut() {
        direct.sort_unstable();
    }

    let mut parents = nodes
        .keys()
        .copied()
        .map(|pid| (pid, pid))
        .collect::<BTreeMap<_, _>>();
    for (&pid, direct) in &blockers {
        for blocker in direct {
            let left = component_root(pid, &parents);
            let right = component_root(*blocker, &parents);
            if left != right {
                parents.insert(left, right);
            }
        }
    }
    let mut components = BTreeMap::<i64, Vec<i64>>::new();
    for pid in nodes.keys().copied() {
        components
            .entry(component_root(pid, &parents))
            .or_default()
            .push(pid);
    }

    let mut raw = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::with_capacity(nodes.len());
    for members in components.values_mut() {
        members.sort_unstable();
        let roots = members
            .iter()
            .copied()
            .filter(|pid| blockers.get(pid).is_none_or(Vec::is_empty))
            .collect::<Vec<_>>();
        if roots.is_empty() {
            if let Some(root) = members.first().copied() {
                walk(root, None, 1, &children, &mut visited, &mut raw);
            }
        } else {
            for root in roots {
                walk(root, None, 1, &children, &mut visited, &mut raw);
            }
        }
        for pid in members.iter().copied() {
            if !visited.contains(&pid) {
                walk(pid, None, 1, &children, &mut visited, &mut raw);
            }
        }
    }

    raw.into_iter()
        .map(|(pid, parent_pid, depth)| Placement {
            pid,
            parent_pid,
            depth,
            extra_blockers: blockers
                .get(&pid)
                .into_iter()
                .flatten()
                .copied()
                .filter(|blocker| Some(*blocker) != parent_pid)
                .collect(),
        })
        .collect()
}

fn component_root(pid: i64, parents: &BTreeMap<i64, i64>) -> i64 {
    let mut root = pid;
    while parents.get(&root).is_some_and(|parent| *parent != root) {
        root = parents[&root];
    }
    root
}

fn walk(
    root: i64,
    parent: Option<i64>,
    depth: usize,
    children: &BTreeMap<i64, Vec<i64>>,
    visited: &mut HashSet<i64>,
    ordered: &mut Vec<(i64, Option<i64>, usize)>,
) {
    let mut stack = vec![(root, parent, depth)];
    while let Some((pid, parent, depth)) = stack.pop() {
        if !visited.insert(pid) {
            continue;
        }
        ordered.push((pid, parent, depth));
        if let Some(children) = children.get(&pid) {
            for child in children.iter().rev() {
                stack.push((*child, Some(pid), depth.saturating_add(1)));
            }
        }
    }
}

fn included_pids(
    matched_records: Option<&[Value]>,
    locators: &HashMap<Locator, i64>,
    placements: &[Placement],
) -> Result<HashSet<i64>, ApiError> {
    let Some(matched_records) = matched_records else {
        return Ok(placements.iter().map(|placement| placement.pid).collect());
    };
    let by_pid = placements
        .iter()
        .map(|placement| (placement.pid, placement))
        .collect::<HashMap<_, _>>();
    let mut included = HashSet::new();
    for row in matched_records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("row"))
    {
        let locator = locator(row)?;
        let Some(pid) = locators.get(&locator).copied() else {
            return Err(ApiError::Product(Box::new(ProductError {
                code: "source_changed",
                message:
                    "The PostgreSQL lock source changed while applying the graph search; retry the request."
                        .to_owned(),
                parameter: None,
                retryable: true,
                status: StatusCode::CONFLICT,
            })));
        };
        let Some(placement) = by_pid.get(&pid) else {
            continue;
        };
        let mut pending = Vec::with_capacity(placement.extra_blockers.len().saturating_add(1));
        pending.push(pid);
        pending.extend(placement.extra_blockers.iter().copied());
        while let Some(current) = pending.pop() {
            if !included.insert(current) {
                continue;
            }
            if let Some(parent) = by_pid
                .get(&current)
                .and_then(|placement| placement.parent_pid)
            {
                pending.push(parent);
            }
        }
    }
    Ok(included)
}

fn add_derived_layouts(records: &mut [Value]) {
    for record in records {
        let Some(layout) = record.get_mut("layout") else {
            continue;
        };
        if layout.get("logical_name").and_then(Value::as_str) != Some("pg_locks") {
            continue;
        }
        let Some(columns) = layout.get_mut("columns").and_then(Value::as_array_mut) else {
            continue;
        };
        columns.extend([
            json!({
                "name": "lock_tree_parent_pid",
                "type": "i32",
                "class": "label",
                "unit": "none",
                "nullable": true,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "lock_tree_depth",
                "type": "u32",
                "class": "label",
                "unit": "none",
                "nullable": false,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "lock_tree_order",
                "type": "u32",
                "class": "label",
                "unit": "none",
                "nullable": false,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "lock_tree_extra_blockers",
                "type": "list_i32",
                "class": "label",
                "unit": "none",
                "nullable": false,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "lock_tree_waits_on_prepared",
                "type": "bool",
                "class": "label",
                "unit": "none",
                "nullable": false,
                "available": true,
                "origin": "kronika_derived",
            }),
        ]);
    }
}

fn selected_range(rows: &[Value]) -> (Value, Value) {
    let timestamps = rows
        .iter()
        .filter_map(|row| row.get("timestamp").and_then(value_text))
        .filter_map(|value| value.parse::<i64>().ok())
        .collect::<Vec<_>>();
    (
        timestamps
            .iter()
            .min()
            .map_or(Value::Null, |value| json!(value.to_string())),
        timestamps
            .iter()
            .max()
            .map_or(Value::Null, |value| json!(value.to_string())),
    )
}

fn product_input(parameter: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code: "invalid_input",
        message: message.into(),
        parameter: Some(parameter),
        retryable: false,
        status: StatusCode::BAD_REQUEST,
    }))
}

fn product_bounded(
    code: &'static str,
    message: impl Into<String>,
    parameter: Option<&'static str>,
) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code,
        message: message.into(),
        parameter,
        retryable: false,
        status: StatusCode::UNPROCESSABLE_ENTITY,
    }))
}

fn product_internal(code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code,
        message: message.into(),
        parameter: None,
        retryable: false,
        status: StatusCode::INTERNAL_SERVER_ERROR,
    }))
}

#[cfg(test)]
#[path = "lock_graph/tests.rs"]
mod tests;
