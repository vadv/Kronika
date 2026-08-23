//! Mechanical projection of one complete recorded `PostgreSQL` lock graph.

use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::{
    LOCK_FIELDS, MAX_FIELDS, MAX_ROWS, PostgresqlFailure, PostgresqlPayload, State, anchor_value,
    collect, failure, fields, input, page, page_size, resolve_anchor, selected_at,
};
use crate::route::{Order, Route, SnapshotRequest};

const REQUIRED_GRAPH_FIELDS: &[&str] = &["pid", "blocked_by", "datname", "lock_target"];
const PREPARED_FIELDS: &[&str] = &["datname", "prepared_count", "max_age_us", "max_xid_age_tx"];

pub(super) fn execute(
    state: &State,
    args: &Map<String, Value>,
    cancelled: &impl Fn() -> bool,
) -> Result<PostgresqlPayload, PostgresqlFailure> {
    if args.contains_key("find") {
        return Err(input(
            "find",
            "find is not supported by the shared Rust field registry for Locks",
        ));
    }
    if args.contains_key("cursor") {
        return Err(input(
            "cursor",
            "lock graphs are admitted and returned as one complete bounded set",
        ));
    }
    // Validate the advertised bound even though a complete graph is never cut
    // at the requested page size.
    let _ = page_size(args)?;
    let at = super::timestamp(args, "at_us")?;
    let projected = graph_fields(args)?;
    let anchor = resolve_anchor(state, at, &["pg_locks"], cancelled)?;
    let collected = collect(
        state,
        Route::Snapshot(Box::new(SnapshotRequest {
            segment_id: anchor.segment_id,
            at,
            sections: vec!["pg_locks".to_owned()],
            fields: projected,
            by: vec!["pid".to_owned()],
            direction: Order::Asc,
            group: None,
            page_size: Some(MAX_ROWS),
            cursor: None,
            search: None,
            first_match: false,
            text: None,
            filters: Vec::new(),
            type_id: None,
            row_ordinal: None,
        })),
        cancelled,
    )?;
    let graph_page = page(&collected.records, collected.stop_reason);
    let selected = selected_at(&collected.records);
    let mut decoded = decode_rows(&collected.records, "pg_locks")?;
    admit_complete_graph(&graph_page, decoded.rows.len())?;
    let prepared =
        prepared_transactions(state, anchor.segment_id, selected.unwrap_or(at), cancelled)?;
    decoded.warnings.extend(prepared.warnings);
    let (locks, components) = build_graph(decoded.rows, &prepared.rows)?;
    let returned = locks.len();
    let response_anchor = anchor_value(at, selected, Some(&anchor));
    let mut warnings = anchor.warnings;
    warnings.extend(decoded.warnings);

    Ok(PostgresqlPayload {
        anchor: response_anchor,
        data: json!({
            "locks": locks,
            "components": components,
            "semantics": lock_semantics(decoded.layouts),
        }),
        page: json!({
            "returned": returned,
            "truncated": false,
            "next_cursor": Value::Null,
            "stop_reason": "complete",
        }),
        warnings,
        summary: format!(
            "Returned {returned} recorded PostgreSQL lock rows in a complete mechanical graph."
        ),
    })
}

fn graph_fields(args: &Map<String, Value>) -> Result<Vec<String>, PostgresqlFailure> {
    let mut projected = fields(args, LOCK_FIELDS)?;
    for required in REQUIRED_GRAPH_FIELDS {
        if !projected.iter().any(|field| field == required) {
            projected.push((*required).to_owned());
        }
    }
    if projected.len() > MAX_FIELDS {
        return Err(input(
            "fields",
            "the requested projection plus required lock-graph fields exceeds 32 names",
        ));
    }
    Ok(projected)
}

fn admit_complete_graph(page: &Value, rows: usize) -> Result<(), PostgresqlFailure> {
    if rows > MAX_ROWS
        || page
            .get("truncated")
            .and_then(Value::as_bool)
            .unwrap_or(true)
    {
        return Err(failure(
            "whole_set_bound_exceeded",
            "the recorded lock set exceeds the 500-row whole-set bound",
            Some("page_size"),
        ));
    }
    Ok(())
}

fn prepared_transactions(
    state: &State,
    segment_id: i64,
    at: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<DecodedRows, PostgresqlFailure> {
    let request = SnapshotRequest {
        segment_id,
        at,
        sections: vec!["pg_prepared_xacts".to_owned()],
        fields: PREPARED_FIELDS
            .iter()
            .map(|field| (*field).to_owned())
            .collect(),
        by: vec!["datname".to_owned()],
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
    match collect(state, Route::Snapshot(Box::new(request)), cancelled) {
        Ok(collected) => decode_rows(&collected.records, "pg_prepared_xacts"),
        Err(error) if error.code == "no_such_section" => Ok(DecodedRows::default()),
        Err(error) => Err(error),
    }
}

#[derive(Default)]
struct DecodedRows {
    rows: Vec<Value>,
    layouts: Vec<Value>,
    warnings: Vec<Value>,
}

fn decode_rows(records: &[Value], logical_name: &str) -> Result<DecodedRows, PostgresqlFailure> {
    let mut layouts = BTreeMap::<String, Value>::new();
    let mut warnings = Vec::new();
    for record in records {
        match record.get("record").and_then(Value::as_str) {
            Some("layout") => {
                let Some(layout) = record.get("layout").and_then(Value::as_object) else {
                    return Err(malformed("a lock snapshot layout is not an object"));
                };
                if layout.get("logical_name").and_then(Value::as_str) != Some(logical_name) {
                    continue;
                }
                let Some(type_id) = layout.get("type_id").and_then(Value::as_str) else {
                    return Err(malformed("a lock snapshot layout has no type_id"));
                };
                layouts.insert(type_id.to_owned(), Value::Object(layout.clone()));
            }
            Some("warning") => warnings.push(record.clone()),
            _ => {}
        }
    }

    let mut rows = Vec::new();
    for record in records {
        if record.get("record").and_then(Value::as_str) != Some("row") {
            continue;
        }
        let type_id = record
            .get("type_id")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a recorded row has no type_id"))?;
        let layout = layouts
            .get(type_id)
            .and_then(Value::as_object)
            .ok_or_else(|| malformed("a recorded row has no matching projected layout"))?;
        let columns = layout
            .get("columns")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("a projected layout has no columns"))?;
        let values = record
            .get("values")
            .and_then(Value::as_array)
            .ok_or_else(|| malformed("a recorded row has no value array"))?;
        if columns.len() != values.len() {
            return Err(malformed(
                "a recorded row does not match its projected layout",
            ));
        }
        let mut named = Map::new();
        for (column, value) in columns.iter().zip(values) {
            let name = column
                .get("name")
                .and_then(Value::as_str)
                .ok_or_else(|| malformed("a projected column has no name"))?;
            named.insert(name.to_owned(), value.clone());
        }
        let mut row = record
            .as_object()
            .cloned()
            .ok_or_else(|| malformed("a recorded row is not an object"))?;
        row.insert("logical_name".to_owned(), json!(logical_name));
        row.insert("values".to_owned(), Value::Object(named));
        rows.push(Value::Object(row));
    }
    Ok(DecodedRows {
        rows,
        layouts: layouts.into_values().collect(),
        warnings,
    })
}

struct LockNode {
    row: Map<String, Value>,
    blockers: Vec<i32>,
    datname: Value,
}

#[derive(Clone)]
struct Placement {
    component_id: i32,
    component_order: usize,
    parent_pid: Option<i32>,
    depth: usize,
    tree_order: usize,
}

fn build_graph(
    rows: Vec<Value>,
    prepared: &[Value],
) -> Result<(Vec<Value>, Vec<Value>), PostgresqlFailure> {
    let mut nodes = BTreeMap::<i32, LockNode>::new();
    for row in rows {
        let row = row
            .as_object()
            .cloned()
            .ok_or_else(|| malformed("a lock row is not an object"))?;
        let values = row
            .get("values")
            .and_then(Value::as_object)
            .ok_or_else(|| malformed("a lock row has no named values"))?;
        let pid = parse_i32(
            values
                .get("pid")
                .ok_or_else(|| malformed("a lock row has no pid"))?,
        )
        .filter(|pid| *pid > 0)
        .ok_or_else(|| malformed("a lock row has an invalid pid"))?;
        let blockers = parse_blockers(
            values
                .get("blocked_by")
                .ok_or_else(|| malformed("a lock row has no blocked_by edges"))?,
        )?;
        let datname = values.get("datname").cloned().unwrap_or(Value::Null);
        if nodes
            .insert(
                pid,
                LockNode {
                    row,
                    blockers,
                    datname,
                },
            )
            .is_some()
        {
            return Err(malformed("a complete lock graph contains a duplicate pid"));
        }
    }
    for node in nodes.values() {
        if node
            .blockers
            .iter()
            .any(|blocker| *blocker > 0 && !nodes.contains_key(blocker))
        {
            return Err(failure(
                "incomplete_lock_graph",
                "a positive recorded blocker PID has no row in the admitted lock set",
                None,
            ));
        }
    }

    let component_members = connected_components(&nodes);
    let mut placements = BTreeMap::<i32, Placement>::new();
    let mut traversal = Vec::with_capacity(nodes.len());
    for (component_order, members) in component_members.iter().enumerate() {
        place_component(
            &nodes,
            members,
            component_order,
            &mut placements,
            &mut traversal,
        )?;
    }

    let mut locks = Vec::with_capacity(traversal.len());
    for pid in &traversal {
        let node = nodes
            .get(pid)
            .ok_or_else(|| malformed("a placed lock row disappeared"))?;
        let placement = placements
            .get(pid)
            .ok_or_else(|| malformed("a lock row has no graph placement"))?;
        locks.push(enriched_row(*pid, node, placement)?);
    }

    let mut components = Vec::with_capacity(component_members.len());
    for (component_order, members) in component_members.iter().enumerate() {
        components.push(component_value(
            &nodes,
            &placements,
            members,
            component_order,
            prepared,
        )?);
    }
    Ok((locks, components))
}

fn connected_components(nodes: &BTreeMap<i32, LockNode>) -> Vec<Vec<i32>> {
    let mut adjacent = BTreeMap::<i32, BTreeSet<i32>>::new();
    for (&pid, node) in nodes {
        adjacent.entry(pid).or_default();
        for &blocker in node.blockers.iter().filter(|blocker| **blocker > 0) {
            adjacent.entry(pid).or_default().insert(blocker);
            adjacent.entry(blocker).or_default().insert(pid);
        }
    }
    let mut visited = BTreeSet::new();
    let mut components = Vec::new();
    for &start in nodes.keys() {
        if visited.contains(&start) {
            continue;
        }
        let mut pending = BTreeSet::from([start]);
        let mut members = Vec::new();
        while let Some(pid) = pending.pop_first() {
            if !visited.insert(pid) {
                continue;
            }
            members.push(pid);
            if let Some(neighbours) = adjacent.get(&pid) {
                pending.extend(neighbours.iter().filter(|pid| !visited.contains(pid)));
            }
        }
        components.push(members);
    }
    components
}

fn place_component(
    nodes: &BTreeMap<i32, LockNode>,
    members: &[i32],
    component_order: usize,
    placements: &mut BTreeMap<i32, Placement>,
    traversal: &mut Vec<i32>,
) -> Result<(), PostgresqlFailure> {
    let member_set = members.iter().copied().collect::<BTreeSet<_>>();
    let mut children = BTreeMap::<i32, BTreeSet<i32>>::new();
    let mut roots = Vec::new();
    for &pid in members {
        let blockers = nodes
            .get(&pid)
            .ok_or_else(|| malformed("a component member has no lock row"))?
            .blockers
            .iter()
            .copied()
            .filter(|blocker| *blocker > 0 && member_set.contains(blocker))
            .collect::<Vec<_>>();
        if blockers.is_empty() {
            roots.push(pid);
        }
        for blocker in blockers {
            children.entry(blocker).or_default().insert(pid);
        }
    }
    if roots.is_empty()
        && let Some(first) = members.first()
    {
        roots.push(*first);
    }
    let component_id = members
        .first()
        .copied()
        .ok_or_else(|| malformed("a lock component has no members"))?;
    for root in roots {
        walk_component(
            root,
            None,
            1,
            component_id,
            component_order,
            &children,
            placements,
            traversal,
        );
    }
    for &pid in members {
        if !placements.contains_key(&pid) {
            walk_component(
                pid,
                None,
                1,
                component_id,
                component_order,
                &children,
                placements,
                traversal,
            );
        }
    }
    Ok(())
}

#[allow(
    clippy::too_many_arguments,
    reason = "an iterative graph walk carries its deterministic placement context explicitly"
)]
fn walk_component(
    root: i32,
    parent: Option<i32>,
    depth: usize,
    component_id: i32,
    component_order: usize,
    children: &BTreeMap<i32, BTreeSet<i32>>,
    placements: &mut BTreeMap<i32, Placement>,
    traversal: &mut Vec<i32>,
) {
    let mut pending = vec![(root, parent, depth)];
    while let Some((pid, parent_pid, depth)) = pending.pop() {
        if placements.contains_key(&pid) {
            continue;
        }
        let tree_order = traversal.len();
        placements.insert(
            pid,
            Placement {
                component_id,
                component_order,
                parent_pid,
                depth,
                tree_order,
            },
        );
        traversal.push(pid);
        if let Some(children) = children.get(&pid) {
            pending.extend(
                children
                    .iter()
                    .rev()
                    .map(|child| (*child, Some(pid), depth.saturating_add(1))),
            );
        }
    }
}

fn enriched_row(
    pid: i32,
    node: &LockNode,
    placement: &Placement,
) -> Result<Value, PostgresqlFailure> {
    let mut row = node.row.clone();
    let values = row
        .get_mut("values")
        .and_then(Value::as_object_mut)
        .ok_or_else(|| malformed("a decoded lock row has no named values"))?;
    let extra_blockers = node
        .blockers
        .iter()
        .copied()
        .filter(|blocker| *blocker > 0 && Some(*blocker) != placement.parent_pid)
        .collect::<Vec<_>>();
    values.insert(
        "lock_tree_component_id".to_owned(),
        json!(placement.component_id),
    );
    values.insert(
        "lock_tree_component_order".to_owned(),
        json!(placement.component_order),
    );
    values.insert("lock_tree_order".to_owned(), json!(placement.tree_order));
    values.insert("lock_tree_depth".to_owned(), json!(placement.depth));
    values.insert(
        "lock_tree_parent_pid".to_owned(),
        json!(placement.parent_pid),
    );
    values.insert("lock_tree_extra_blockers".to_owned(), json!(extra_blockers));
    values.insert(
        "lock_tree_waits_on_prepared".to_owned(),
        json!(node.blockers.contains(&0)),
    );
    if !node.blockers.is_empty() {
        let segment_id = row.get("segment_id").cloned().unwrap_or(Value::Null);
        let type_id = row.get("type_id").cloned().unwrap_or(Value::Null);
        let row_ordinal = row.get("ordinal").cloned().unwrap_or(Value::Null);
        let timestamp = row.get("timestamp").cloned().unwrap_or(Value::Null);
        row.insert(
            "accepted_finding".to_owned(),
            json!({
                "origin": "kronika_derived",
                "source": "kronika_index",
                "logical_name": "pg_locks",
                "kind": "known_bad",
                "field": "blocked_by",
                "field_ordinal": 2,
                "segment_id": segment_id,
                "type_id": type_id,
                "row_ordinal": row_ordinal,
                "timestamp_us": timestamp,
            }),
        );
    }
    debug_assert_eq!(
        row.get("values")
            .and_then(Value::as_object)
            .and_then(|values| values.get("pid"))
            .and_then(parse_i32),
        Some(pid),
        "the enriched row must preserve its recorded PID"
    );
    Ok(Value::Object(row))
}

fn component_value(
    nodes: &BTreeMap<i32, LockNode>,
    placements: &BTreeMap<i32, Placement>,
    members: &[i32],
    component_order: usize,
    prepared: &[Value],
) -> Result<Value, PostgresqlFailure> {
    let mut ordered = members.to_vec();
    ordered.sort_by_key(|pid| {
        placements
            .get(pid)
            .map_or(usize::MAX, |item| item.tree_order)
    });
    let mut roots = Vec::new();
    let mut prepared_waiters = Vec::new();
    for pid in &ordered {
        let placement = placements
            .get(pid)
            .ok_or_else(|| malformed("a component member has no graph placement"))?;
        let node = nodes
            .get(pid)
            .ok_or_else(|| malformed("a component member has no lock row"))?;
        if placement.parent_pid.is_none() {
            roots.push(*pid);
        }
        if node.blockers.contains(&0) {
            prepared_waiters.push(*pid);
        }
    }
    let prepared_datnames = prepared_waiters
        .iter()
        .filter_map(|pid| nodes.get(pid).map(|node| &node.datname))
        .collect::<Vec<_>>();
    let prepared_transactions = prepared
        .iter()
        .filter(|row| {
            row.pointer("/values/datname")
                .is_some_and(|datname| prepared_datnames.contains(&datname))
        })
        .cloned()
        .collect::<Vec<_>>();
    let mut edges = Vec::new();
    for pid in &ordered {
        let node = nodes
            .get(pid)
            .ok_or_else(|| malformed("a component member has no lock row"))?;
        edges.extend(
            node.blockers
                .iter()
                .map(|blocker| json!({"waiter_pid": pid, "blocker_pid": blocker})),
        );
    }
    Ok(json!({
        "component_id": members.first().copied(),
        "component_order": component_order,
        "root_pids": roots,
        "member_pids": ordered,
        "edges": edges,
        "prepared_waiter_pids": prepared_waiters,
        "prepared_transactions": prepared_transactions,
    }))
}

fn parse_i32(value: &Value) -> Option<i32> {
    value
        .as_i64()
        .and_then(|value| i32::try_from(value).ok())
        .or_else(|| value.as_str()?.parse().ok())
}

fn parse_blockers(value: &Value) -> Result<Vec<i32>, PostgresqlFailure> {
    let values = value
        .as_array()
        .ok_or_else(|| malformed("blocked_by is not a recorded PID array"))?;
    let mut blockers = Vec::with_capacity(values.len());
    for value in values {
        let blocker = parse_i32(value)
            .filter(|blocker| *blocker >= 0)
            .ok_or_else(|| malformed("blocked_by contains an invalid PID"))?;
        if blockers.contains(&blocker) {
            return Err(malformed("blocked_by contains a duplicate direct edge"));
        }
        blockers.push(blocker);
    }
    Ok(blockers)
}

fn lock_semantics(layouts: Vec<Value>) -> Vec<Value> {
    let mut semantics = layouts
        .into_iter()
        .map(|layout| {
            json!({
                "origin": "recorded",
                "source": "kronika_registry",
                "layout": layout,
            })
        })
        .collect::<Vec<_>>();
    semantics.push(json!({
        "origin": "recorded",
        "source": "pg_blocking_pids",
        "logical_name": "pg_locks",
        "field": "blocked_by",
        "prepared_transaction_pid": 0,
    }));
    semantics.push(json!({
        "origin": "kronika_derived",
        "source": "kronika_index",
        "logical_name": "pg_locks",
        "kind": "known_bad",
        "field": "blocked_by",
        "predicate": "nonempty",
    }));
    semantics.push(json!({
        "origin": "kronika_derived",
        "source": "recorded_blocked_by_edges",
        "operation": "mechanical_component_parent_depth_order",
    }));
    semantics
}

fn malformed(message: &'static str) -> PostgresqlFailure {
    failure("malformed_lock_graph", message, None)
}

#[cfg(test)]
#[path = "locks/tests.rs"]
mod tests;
