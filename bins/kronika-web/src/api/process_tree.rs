//! Bounded whole-snapshot Process tree shared by HTTP and MCP.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use hyper::StatusCode;
use kronika_reader::SegmentKind;
use serde_json::{Value, json};

use super::{ApiError, CachePolicy, Prepared, ProductError, ResponseMeta};
use crate::route::{Order, SnapshotRequest};

pub(crate) const MAX_PROCESS_TREE_ROWS: usize = 500;

const REQUIRED_FIELDS: [&str; 3] = ["pid", "ppid", "starttime"];
const DERIVED_FIELDS: [&str; 3] = [
    "process_tree_parent_pid",
    "process_tree_depth",
    "process_tree_order",
];

pub(crate) struct PreparedProcessTree {
    complete: Box<Prepared>,
    matched: Option<Box<Prepared>>,
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
    ppid: usize,
    starttime: usize,
}

struct Node {
    pid: i64,
    ppid: i64,
    locator: Locator,
    row: Value,
}

pub(super) fn prepare(
    root: &Path,
    request: SnapshotRequest,
    if_none_match: Option<&str>,
) -> Result<Prepared, ApiError> {
    let original = request.clone();
    let (mut complete_request, matched_request) = normalize(request)?;
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
        super::weak_etag("process_tree", &format!("{original:?}"), candidates),
    );
    let complete = Box::new(super::snapshot::prepare(root, complete_request, None)?);
    let matched = matched_request
        .map(|request| super::snapshot::prepare(root, request, None).map(Box::new))
        .transpose()?;
    let concrete_validator = if_none_match.filter(|offered| offered.trim() != "*");
    if let Some(not_modified) = super::conditional_not_modified(meta.clone(), concrete_validator) {
        return Ok(not_modified);
    }
    Ok(Prepared::ProcessTree(PreparedProcessTree {
        complete,
        matched,
        meta,
    }))
}

fn normalize(
    mut request: SnapshotRequest,
) -> Result<(SnapshotRequest, Option<SnapshotRequest>), ApiError> {
    if request.sections.as_slice() != ["os_process"] {
        return Err(ApiError::NoSuchSection);
    }
    if request.cursor.is_some() {
        return Err(product_input(
            "cursor",
            "The Process tree is a bounded whole-snapshot result and does not accept a cursor.",
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
    let search = request.search.take();
    let matched = search.map(|search| {
        let mut matched = request.clone();
        matched.search = Some(search);
        // Structured search is evaluated by the snapshot page producer. The
        // complete source has already been admitted to this exact whole-set cap.
        matched.page_size = Some(MAX_PROCESS_TREE_ROWS);
        matched
    });
    Ok((request, matched))
}

impl PreparedProcessTree {
    pub(super) fn meta(&self) -> ResponseMeta {
        self.meta.clone()
    }

    pub(super) fn stream(
        self,
        emit: &mut impl FnMut(Value) -> bool,
        cancelled: &impl Fn() -> bool,
    ) -> Result<(), ApiError> {
        let complete = collect(*self.complete, cancelled)?;
        if cancelled() {
            return Ok(());
        }
        let matched = self
            .matched
            .map(|prepared| collect(*prepared, cancelled))
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

fn collect(prepared: Prepared, cancelled: &impl Fn() -> bool) -> Result<Vec<Value>, ApiError> {
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
                if rows > MAX_PROCESS_TREE_ROWS {
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
            "tree_bound_exceeded",
            "The Process snapshot exceeds the 500-row tree admission limit.",
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
    let mut nodes = HashMap::new();
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
            return Err(product_bounded(
                "tree_identity_conflict",
                format!(
                    "The Process snapshot contains more than one row for PID {}.",
                    node.pid
                ),
            ));
        }
        locators.insert(node.locator.clone(), node.pid);
        nodes.insert(node.pid, node);
    }
    if nodes.len() > MAX_PROCESS_TREE_ROWS {
        return Err(product_bounded(
            "tree_bound_exceeded",
            "The Process snapshot exceeds the 500-row tree admission limit.",
        ));
    }

    let parents = parents(&nodes);
    let included = included_pids(matched_records, &locators, &parents)?;
    let ordered = tree_order(&nodes, &parents);
    add_derived_layouts(&mut metadata);

    let mut rows = Vec::with_capacity(included.len());
    for (tree_order, (pid, parent_pid, depth)) in ordered.into_iter().enumerate() {
        if !included.contains(&pid) {
            continue;
        }
        let mut node = nodes.remove(&pid).ok_or_else(|| {
            product_internal(
                "tree_unreadable",
                "The Process tree lost a row while assigning its stable order.",
            )
        })?;
        let values = node
            .row
            .get_mut("values")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                product_internal(
                    "tree_layout_unusable",
                    "A Process row has no typed values array.",
                )
            })?;
        values.push(parent_pid.map_or(Value::Null, |parent| json!(parent)));
        values.push(json!(depth));
        values.push(json!(tree_order));
        rows.push(node.row);
    }
    let row_count = rows.len();
    let (from, to) = selected_range(&rows);
    metadata.extend(rows);
    metadata.push(json!({
        "record": "snapshot_page",
        "logical_name": "os_process",
        "eligible": row_count.to_string(),
        "returned": row_count.to_string(),
        "has_more": false,
        "truncated": false,
        "next_cursor": Value::Null,
        "page_size": Value::Null,
        "order_by": ["process_tree_order"],
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
        let Some(type_id) = layout.get("type_id").and_then(value_text) else {
            continue;
        };
        let columns = layout
            .get("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                product_internal(
                    "tree_layout_unusable",
                    "A Process layout has no typed columns array.",
                )
            })?;
        layouts.insert(
            type_id,
            Layout {
                pid: column(columns, "pid")?,
                ppid: column(columns, "ppid")?,
                starttime: column(columns, "starttime")?,
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
                "tree_layout_unusable",
                format!("The Process tree requires the {name} field."),
            )
        })
}

fn node(row: Value, layouts: &HashMap<String, Layout>) -> Result<Node, ApiError> {
    let locator = locator(&row)?;
    let layout = layouts.get(&locator.type_id).ok_or_else(|| {
        product_internal(
            "tree_layout_unusable",
            format!(
                "The Process row type {} has no matching projected layout.",
                locator.type_id
            ),
        )
    })?;
    let values = row.get("values").and_then(Value::as_array).ok_or_else(|| {
        product_internal(
            "tree_layout_unusable",
            "A Process row has no typed values array.",
        )
    })?;
    let process_id = decimal(values.get(layout.pid), "pid")?;
    let recorded_parent_id = decimal(values.get(layout.ppid), "ppid")?;
    let _starttime = decimal(values.get(layout.starttime), "starttime")?;
    Ok(Node {
        pid: process_id,
        ppid: recorded_parent_id,
        locator,
        row,
    })
}

fn locator(row: &Value) -> Result<Locator, ApiError> {
    let field = |name| {
        row.get(name).and_then(value_text).ok_or_else(|| {
            product_internal(
                "process_locator_unavailable",
                format!("A Process row has no {name} locator."),
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
                "tree_layout_unusable",
                format!("A Process row has no decimal {name} value."),
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

fn parents(nodes: &HashMap<i64, Node>) -> HashMap<i64, Option<i64>> {
    nodes
        .values()
        .map(|node| {
            let parent =
                (node.ppid != node.pid && nodes.contains_key(&node.ppid)).then_some(node.ppid);
            (node.pid, parent)
        })
        .collect()
}

fn included_pids(
    matched_records: Option<&[Value]>,
    locators: &HashMap<Locator, i64>,
    parents: &HashMap<i64, Option<i64>>,
) -> Result<HashSet<i64>, ApiError> {
    let Some(matched_records) = matched_records else {
        return Ok(parents.keys().copied().collect());
    };
    let mut included = HashSet::new();
    for row in matched_records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("row"))
    {
        let locator = locator(row)?;
        let Some(mut pid) = locators.get(&locator).copied() else {
            return Err(ApiError::Product(Box::new(ProductError {
                code: "source_changed",
                message:
                    "The Process source changed while applying the tree search; retry the request."
                        .to_owned(),
                parameter: None,
                retryable: true,
                status: StatusCode::CONFLICT,
            })));
        };
        while included.insert(pid) {
            let Some(parent) = parents.get(&pid).copied().flatten() else {
                break;
            };
            pid = parent;
        }
    }
    Ok(included)
}

fn tree_order(
    nodes: &HashMap<i64, Node>,
    parents: &HashMap<i64, Option<i64>>,
) -> Vec<(i64, Option<i64>, usize)> {
    let mut children: HashMap<i64, Vec<i64>> = HashMap::new();
    let mut roots = Vec::new();
    for (&pid, parent) in parents {
        if let Some(parent) = parent {
            children.entry(*parent).or_default().push(pid);
        } else {
            roots.push(pid);
        }
    }
    roots.sort_unstable();
    for children in children.values_mut() {
        children.sort_unstable();
    }

    let mut ordered = Vec::with_capacity(nodes.len());
    let mut visited = HashSet::with_capacity(nodes.len());
    for root in roots {
        walk(root, None, 0, &children, &mut visited, &mut ordered);
    }
    let mut remaining = nodes.keys().copied().collect::<Vec<_>>();
    remaining.sort_unstable();
    for root in remaining {
        if !visited.contains(&root) {
            walk(root, None, 0, &children, &mut visited, &mut ordered);
        }
    }
    ordered
}

fn walk(
    root: i64,
    parent: Option<i64>,
    depth: usize,
    children: &HashMap<i64, Vec<i64>>,
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

fn add_derived_layouts(records: &mut [Value]) {
    for record in records {
        let Some(columns) = record
            .get_mut("layout")
            .and_then(|layout| layout.get_mut("columns"))
            .and_then(Value::as_array_mut)
        else {
            continue;
        };
        columns.extend([
            json!({
                "name": "process_tree_parent_pid",
                "type": "i32",
                "class": "label",
                "unit": "none",
                "nullable": true,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "process_tree_depth",
                "type": "u32",
                "class": "label",
                "unit": "none",
                "nullable": false,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "process_tree_order",
                "type": "u32",
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

fn product_bounded(code: &'static str, message: impl Into<String>) -> ApiError {
    ApiError::Product(Box::new(ProductError {
        code,
        message: message.into(),
        parameter: None,
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
#[path = "process_tree/tests.rs"]
mod tests;
