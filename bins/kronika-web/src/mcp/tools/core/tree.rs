use std::collections::{HashMap, HashSet};

use serde_json::{Value, json};

use super::{Failure, MAX_TREE_ROWS};
use crate::route::{Order, SnapshotRequest};

const REQUIRED_FIELDS: [&str; 3] = ["pid", "ppid", "starttime"];
const DERIVED_FIELDS: [&str; 3] = ["parent_pid", "depth", "tree_order"];

#[derive(Debug)]
pub(super) struct Prepared {
    pub(super) complete: SnapshotRequest,
    pub(super) matched: Option<SnapshotRequest>,
}

#[derive(Debug)]
pub(super) struct Transformed {
    pub(super) records: Vec<Value>,
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

pub(super) fn prepare(mut request: SnapshotRequest) -> Result<Prepared, Failure> {
    if request.cursor.is_some() {
        return Err(Failure::input(
            "cursor",
            "The Process tree is a bounded whole-snapshot result and does not accept a cursor.",
        ));
    }
    request
        .fields
        .retain(|field| !DERIVED_FIELDS.contains(&field.as_str()));
    if request.fields.is_empty() {
        request.fields = REQUIRED_FIELDS.iter().map(ToString::to_string).collect();
    } else {
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
        matched
    });
    Ok(Prepared {
        complete: request,
        matched,
    })
}

pub(super) fn transform(
    records: Vec<Value>,
    matched_records: Option<&[Value]>,
) -> Result<Transformed, Failure> {
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
            return Err(Failure::bounded(
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
    if nodes.len() > MAX_TREE_ROWS {
        return Err(Failure::bounded(
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
            Failure::bounded(
                "tree_unreadable",
                "The Process tree lost a row while assigning its stable order.",
            )
        })?;
        let values = node
            .row
            .get_mut("values")
            .and_then(Value::as_array_mut)
            .ok_or_else(|| {
                Failure::bounded(
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
        "order_by": ["tree"],
        "order_direction": "asc",
        "from": from,
        "to": to,
    }));
    Ok(Transformed { records: metadata })
}

fn layouts(records: &[Value]) -> Result<HashMap<String, Layout>, Failure> {
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
                Failure::bounded(
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

fn column(columns: &[Value], name: &'static str) -> Result<usize, Failure> {
    columns
        .iter()
        .position(|column| {
            column.get("name").and_then(Value::as_str) == Some(name)
                && column.get("available").and_then(Value::as_bool) != Some(false)
        })
        .ok_or_else(|| {
            Failure::bounded(
                "tree_layout_unusable",
                format!("The Process tree requires the {name} field."),
            )
        })
}

fn node(row: Value, layouts: &HashMap<String, Layout>) -> Result<Node, Failure> {
    let locator = locator(&row)?;
    let layout = layouts.get(&locator.type_id).ok_or_else(|| {
        Failure::bounded(
            "tree_layout_unusable",
            format!(
                "The Process row type {} has no matching projected layout.",
                locator.type_id
            ),
        )
    })?;
    let values = row.get("values").and_then(Value::as_array).ok_or_else(|| {
        Failure::bounded(
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

fn locator(row: &Value) -> Result<Locator, Failure> {
    let field = |name| {
        row.get(name).and_then(value_text).ok_or_else(|| {
            Failure::bounded(
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

fn decimal(value: Option<&Value>, name: &'static str) -> Result<i64, Failure> {
    value
        .and_then(|value| {
            value
                .as_i64()
                .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
                .or_else(|| value.as_str().and_then(|value| value.parse().ok()))
        })
        .ok_or_else(|| {
            Failure::bounded(
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
) -> Result<HashSet<i64>, Failure> {
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
            return Err(Failure {
                code: "source_changed",
                message:
                    "The Process source changed while applying the tree search; retry the request."
                        .to_owned(),
                parameter: None,
                retryable: true,
            });
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
                "name": "parent_pid",
                "type": "i32",
                "class": "label",
                "unit": "none",
                "nullable": true,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "depth",
                "type": "u32",
                "class": "label",
                "unit": "none",
                "nullable": false,
                "available": true,
                "origin": "kronika_derived",
            }),
            json!({
                "name": "tree_order",
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

#[cfg(test)]
mod tests;
