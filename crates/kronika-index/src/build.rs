//! Turning a segment's rows into what an index holds: the health line, and one
//! row per object of every section that declares an identity.

use std::collections::BTreeMap;

use kronika_registry::instance_metadata::Environment;
use kronika_registry::{Cell, ColumnClass, Row};

use crate::file::Point;
use crate::health::{Stall, health};
use crate::objects::{Object, SectionObjects, Value};

/// `type_id` of `instance_metadata`.
pub const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_001;
/// `type_id` of `os_psi`.
pub const OS_PSI_TYPE_ID: u32 = 1_107_001;

/// The `resource` column: `0` cpu, `1` memory, `2` io.
const CPU: u32 = 0;
const MEMORY: u32 = 1;
const IO: u32 = 2;

/// The `scope` column: `0` host, `1` pod, `3` container.
const HOST: u32 = 0;
const POD: u32 = 1;
const CONTAINER: u32 = 3;

#[derive(Debug, Default)]
struct PartialStall {
    cpu: Option<i64>,
    memory: Option<i64>,
    io: Option<i64>,
}

/// Health at every pressure snapshot, oldest first.
///
/// `metadata_rows` identifies the collector's resource boundary. Only pressure
/// rows with a matching scope can contribute counters. Every timestamp still
/// gets a point when the scope is wrong or a resource is absent, but its health
/// is `None`. Health needs complete counters in both adjacent snapshots, so a
/// resource reappearing after an absence starts a new baseline.
#[must_use]
pub fn points(metadata_rows: &[Row], psi_rows: &[Row]) -> Vec<Point> {
    let environment = metadata_rows
        .iter()
        .find_map(|row| match row.get("environment") {
            Some(Cell::U32(value)) => Some(*value),
            _other => None,
        });
    let mut snapshots: BTreeMap<i64, PartialStall> = BTreeMap::new();
    for row in psi_rows {
        let Some(Cell::Ts(ts)) = row.get("ts") else {
            continue;
        };
        let snapshot = snapshots.entry(*ts).or_default();
        let (Some(Cell::U32(resource)), Some(Cell::I64(total)), Some(Cell::U32(scope))) =
            (row.get("resource"), row.get("some_total"), row.get("scope"))
        else {
            continue;
        };
        let matching_scope = match environment {
            Some(value) if value == u32::from(Environment::Machine.as_u8()) => *scope == HOST,
            Some(value) if value == u32::from(Environment::Container.as_u8()) => {
                matches!(*scope, POD | CONTAINER)
            }
            _other => false,
        };
        if !matching_scope {
            continue;
        }
        match *resource {
            CPU => snapshot.cpu = Some(*total),
            MEMORY => snapshot.memory = Some(*total),
            IO => snapshot.io = Some(*total),
            _other => {}
        }
    }

    let mut points = Vec::with_capacity(snapshots.len());
    let mut previous: Option<(i64, Stall)> = None;
    for (ts, snapshot) in snapshots {
        let current = match (snapshot.cpu, snapshot.memory, snapshot.io) {
            (Some(cpu), Some(memory), Some(io)) => Some(Stall { cpu, memory, io }),
            _other => None,
        };
        let value = previous.and_then(|(before_ts, before)| {
            current.and_then(|after| health(before, before_ts, after, ts))
        });
        points.push(Point { ts, health: value });
        previous = current.map(|stall| (ts, stall));
    }
    points
}

/// What one object was between the first and the last snapshot that held it.
#[derive(Debug)]
struct Seen {
    labels: Vec<String>,
    first_ts: i64,
    last_ts: i64,
    first: Vec<Cell>,
    last: Vec<Cell>,
}

/// The objects `rows` held, reduced to one row each.
///
/// A cumulative column becomes its delta over the segment, a gauge its last
/// reading. Where a counter went backwards the delta is not defined and the
/// value is [`Value::Null`]; nothing is recorded about why.
///
/// Returns `None` for a section that declares no identity: with nothing saying
/// which rows are the same object, there is nothing to reduce.
#[must_use]
pub fn objects(rows: &[Row], resolve: impl Fn(u64) -> Option<String>) -> Option<SectionObjects> {
    let contract = rows.first()?.contract();
    if contract.identity.is_empty() {
        return None;
    }
    let labels: Vec<&str> = named(contract, ColumnClass::Label);
    let values: Vec<(&str, ColumnClass)> = contract
        .columns
        .iter()
        .filter(|column| matches!(column.class, ColumnClass::Cumulative | ColumnClass::Gauge))
        .map(|column| (column.name, column.class))
        .collect();

    let mut seen: BTreeMap<Vec<String>, Seen> = BTreeMap::new();
    for row in rows {
        let Some(Cell::Ts(ts)) = row.get("ts") else {
            continue;
        };
        let key: Vec<String> = contract
            .identity
            .iter()
            .map(|name| cell_text(row.get(name), &resolve))
            .collect();
        let cells: Vec<Cell> = values
            .iter()
            .map(|(name, _class)| row.get(name).cloned().unwrap_or(Cell::Null))
            .collect();
        match seen.entry(key) {
            std::collections::btree_map::Entry::Vacant(slot) => {
                slot.insert(Seen {
                    labels: labels
                        .iter()
                        .map(|name| cell_text(row.get(name), &resolve))
                        .collect(),
                    first_ts: *ts,
                    last_ts: *ts,
                    first: cells.clone(),
                    last: cells,
                });
            }
            std::collections::btree_map::Entry::Occupied(mut slot) => {
                let object = slot.get_mut();
                if *ts < object.first_ts {
                    object.first_ts = *ts;
                    object.first = cells;
                } else if *ts >= object.last_ts {
                    object.last_ts = *ts;
                    object.labels = labels
                        .iter()
                        .map(|name| cell_text(row.get(name), &resolve))
                        .collect();
                    object.last = cells;
                }
            }
        }
    }

    Some(SectionObjects {
        type_id: contract.type_id.get(),
        label_count: u16::try_from(labels.len()).unwrap_or(u16::MAX),
        value_count: u16::try_from(values.len()).unwrap_or(u16::MAX),
        objects: seen
            .into_values()
            .map(|object| Object {
                labels: object.labels,
                values: values
                    .iter()
                    .enumerate()
                    .map(|(at, (_name, class))| {
                        reduce(*class, object.first.get(at), object.last.get(at))
                    })
                    .collect(),
            })
            .collect(),
    })
}

/// The names of the contract's columns of one class, in declared order.
fn named(
    contract: &'static kronika_registry::TypeContract,
    class: ColumnClass,
) -> Vec<&'static str> {
    contract
        .columns
        .iter()
        .filter(|column| column.class == class)
        .map(|column| column.name)
        .collect()
}

/// One column of one object over the segment.
fn reduce(class: ColumnClass, first: Option<&Cell>, last: Option<&Cell>) -> Value {
    let last = number(last);
    if class == ColumnClass::Gauge {
        return last;
    }
    match (number(first), last) {
        (Value::Int(before), Value::Int(after)) => after
            .checked_sub(before)
            .filter(|delta| *delta >= 0)
            .map_or(Value::Null, Value::Int),
        (Value::Float(before), Value::Float(after)) => {
            let delta = after - before;
            if delta < 0.0 {
                Value::Null
            } else {
                Value::Float(delta)
            }
        }
        _other => Value::Null,
    }
}

/// A numeric cell as the index stores it.
fn number(cell: Option<&Cell>) -> Value {
    match cell {
        Some(Cell::I16(v)) => Value::Int(i64::from(*v)),
        Some(Cell::I32(v)) => Value::Int(i64::from(*v)),
        Some(Cell::I64(v) | Cell::Ts(v)) => Value::Int(*v),
        Some(Cell::U32(v)) => Value::Int(i64::from(*v)),
        Some(Cell::U64(v)) => i64::try_from(*v).map_or(Value::Null, Value::Int),
        Some(Cell::F64(v)) => Value::Float(*v),
        Some(Cell::Bool(v)) => Value::Int(i64::from(*v)),
        _other => Value::Null,
    }
}

/// A label cell as text. A dictionary id becomes what the segment interned
/// under it.
fn cell_text(cell: Option<&Cell>, resolve: &impl Fn(u64) -> Option<String>) -> String {
    match cell {
        Some(Cell::I16(v)) => v.to_string(),
        Some(Cell::I32(v)) => v.to_string(),
        Some(Cell::I64(v) | Cell::Ts(v)) => v.to_string(),
        Some(Cell::U32(v)) => v.to_string(),
        Some(Cell::U64(v)) => v.to_string(),
        Some(Cell::F64(v)) => v.to_string(),
        Some(Cell::Bool(v)) => v.to_string(),
        Some(Cell::StrId(id)) => resolve(*id).unwrap_or_else(|| format!("<str {id}>")),
        Some(Cell::ListI32(v)) => format!("{v:?}"),
        Some(Cell::Null) | None => String::new(),
    }
}

#[cfg(test)]
mod tests;
