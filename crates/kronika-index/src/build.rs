//! Turning a segment's pressure rows into the points an index holds.

use std::collections::BTreeMap;

use kronika_registry::instance_metadata::Environment;
use kronika_registry::{Cell, Row};

use crate::file::Point;
use crate::health::{Stall, health};

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

#[cfg(test)]
mod tests;
