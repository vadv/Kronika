//! Turning a segment's pressure rows into the points an index holds.

use std::collections::BTreeMap;

use kronika_registry::{Cell, Row};

use crate::file::Point;
use crate::health::{Stall, health};

/// `type_id` of `os_psi`.
pub const OS_PSI_TYPE_ID: u32 = 1_107_001;

/// The `resource` column: `0` cpu, `1` memory, `2` io.
const CPU: u32 = 0;
const MEMORY: u32 = 1;
const IO: u32 = 2;

/// Health at every snapshot, oldest first.
///
/// The first snapshot has nothing before it to subtract from, so its health is
/// `None`. Every later point covers the interval since the point before it.
#[must_use]
pub fn points(snapshots: &BTreeMap<i64, Stall>) -> Vec<Point> {
    let mut points = Vec::with_capacity(snapshots.len());
    let mut previous: Option<(i64, Stall)> = None;
    for (ts, stall) in snapshots {
        let value = previous.and_then(|(before_ts, before)| health(before, before_ts, *stall, *ts));
        points.push(Point {
            ts: *ts,
            health: value,
        });
        previous = Some((*ts, *stall));
    }
    points
}

/// The stall counters each snapshot reported, from decoded `os_psi` rows.
///
/// A snapshot writes one row per resource, and a resource absent from the host
/// produces no row at all. A missing resource keeps zero rather than dropping
/// the snapshot, because the other two still have something to say.
///
/// A resource that appears part way through a segment therefore reads as one
/// interval of complete stall: its counter jumps from zero to everything since
/// boot. The point that lands is `0`, which overstates the trouble rather than
/// hiding it.
#[must_use]
pub fn stalls(rows: &[Row]) -> BTreeMap<i64, Stall> {
    let mut snapshots: BTreeMap<i64, Stall> = BTreeMap::new();
    for row in rows {
        let (Some(ts), Some(resource), Some(total)) = (
            timestamp(row),
            unsigned(row, "resource"),
            signed(row, "some_total"),
        ) else {
            continue;
        };
        let stall = snapshots.entry(ts).or_insert(Stall {
            cpu: 0,
            memory: 0,
            io: 0,
        });
        match resource {
            CPU => stall.cpu = total,
            MEMORY => stall.memory = total,
            IO => stall.io = total,
            _other => {}
        }
    }
    snapshots
}

fn timestamp(row: &Row) -> Option<i64> {
    match row.get("ts")? {
        Cell::Ts(ts) => Some(*ts),
        _other => None,
    }
}

fn unsigned(row: &Row, column: &str) -> Option<u32> {
    match row.get(column)? {
        Cell::U32(value) => Some(*value),
        _other => None,
    }
}

fn signed(row: &Row, column: &str) -> Option<i64> {
    match row.get(column)? {
        Cell::I64(value) => Some(*value),
        _other => None,
    }
}

#[cfg(test)]
mod tests;
