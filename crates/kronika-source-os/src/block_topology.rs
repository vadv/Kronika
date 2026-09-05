//! Exact block-device edges from Linux sysfs.
//!
//! Sysfs names two kinds of edge and both are recorded: a partition sits on its
//! whole device (`<device>/partition` marker, parent `dev` one directory up),
//! and a layered device such as dm/LVM/MD lists the devices beneath it in
//! `<device>/slaves/`. Nothing is inferred: an edge exists only where sysfs
//! names both ends.

use std::fmt;
use std::path::Path;

use crate::{SysFs, parse_dev_pair};

#[cfg(test)]
mod tests;

const MAX_BLOCK_DEVICES: usize = 4096;

/// One exact edge from a block device to the device directly beneath it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct BlockEdge {
    /// Upper device identity: a partition or a layered device.
    pub child: (i32, i32),
    /// The device directly beneath it.
    pub parent: (i32, i32),
}

/// A sysfs block-device set exceeded the complete reference ceiling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockTopologyError {
    devices: usize,
}

impl fmt::Display for BlockTopologyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "sysfs block device count {} exceeds complete-section ceiling {MAX_BLOCK_DEVICES}",
            self.devices
        )
    }
}

impl std::error::Error for BlockTopologyError {}

/// Read every exact sysfs edge: partition markers with their parent `dev`, and
/// the `slaves/` of layered devices.
///
/// Missing, unresolved and plain whole devices emit no edge.
///
/// # Errors
/// Returns an error instead of a prefix when the device ceiling is exceeded.
pub fn collect(sys: &SysFs) -> Result<Vec<BlockEdge>, BlockTopologyError> {
    let Ok(entries) = sys.read_dir("dev/block") else {
        return Ok(Vec::new());
    };
    if entries.len() > MAX_BLOCK_DEVICES {
        return Err(BlockTopologyError {
            devices: entries.len(),
        });
    }
    let mut edges = Vec::new();
    for entry in entries {
        let Some(child) = parse_dev_pair(&entry.name) else {
            continue;
        };
        let Ok(target) = sys.canonical_path(&format!("dev/block/{}", entry.name)) else {
            continue;
        };
        if target.join("partition").is_file()
            && let Some(parent) = target
                .parent()
                .and_then(|whole| read_dev(&whole.join("dev")))
        {
            edges.push(BlockEdge { child, parent });
        }
        for parent in slaves(&target) {
            edges.push(BlockEdge { child, parent });
        }
    }
    edges.sort_unstable();
    edges.dedup();
    Ok(edges)
}

fn read_dev(path: &Path) -> Option<(i32, i32)> {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|content| parse_dev_pair(&content))
}

/// The devices a layered device lists beneath itself, in name order.
fn slaves(target: &Path) -> Vec<(i32, i32)> {
    let Ok(entries) = std::fs::read_dir(target.join("slaves")) else {
        return Vec::new();
    };
    let mut paths: Vec<_> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    paths
        .iter()
        .filter_map(|slave| read_dev(&slave.join("dev")))
        .collect()
}
