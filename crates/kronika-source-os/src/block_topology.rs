//! Exact partition-parent relationships from Linux sysfs.

use std::fmt;

use crate::{SysFs, parse_dev_pair};

const MAX_BLOCK_DEVICES: usize = 4096;

/// One exact partition to parent-block-device edge.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionParent {
    /// Partition device identity.
    pub child: (i32, i32),
    /// Parent whole-device identity.
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

/// Read only exact sysfs partition markers and parent `dev` identities.
///
/// Missing, unresolved, layered, and non-partition devices emit no edge.
///
/// # Errors
/// Returns an error instead of a prefix when the device ceiling is exceeded.
pub fn collect(sys: &SysFs) -> Result<Vec<PartitionParent>, BlockTopologyError> {
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
        if !target.join("partition").is_file() {
            continue;
        }
        let Some(parent_dir) = target.parent() else {
            continue;
        };
        let Ok(parent_dev) = std::fs::read_to_string(parent_dir.join("dev")) else {
            continue;
        };
        let Some(parent) = parse_dev_pair(&parent_dev) else {
            continue;
        };
        edges.push(PartitionParent { child, parent });
    }
    edges.sort_unstable_by_key(|edge| edge.child);
    Ok(edges)
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::symlink;

    use tempfile::tempdir;

    use super::{PartitionParent, collect};
    use crate::SysFs;

    #[test]
    fn emits_only_an_exact_partition_marker_and_parent_dev() {
        let directory = tempdir().expect("create sysfs fixture");
        let root = directory.path();
        let whole = root.join("devices/pci/block/nvme0n1");
        let partition = whole.join("nvme0n1p1");
        std::fs::create_dir_all(root.join("dev/block")).expect("create dev block");
        std::fs::create_dir_all(&partition).expect("create partition");
        std::fs::write(whole.join("dev"), "259:0\n").expect("write parent dev");
        std::fs::write(partition.join("partition"), "1\n").expect("write partition marker");
        symlink(
            "../../devices/pci/block/nvme0n1/nvme0n1p1",
            root.join("dev/block/259:1"),
        )
        .expect("link partition");

        assert_eq!(
            collect(&SysFs::new(root.to_path_buf())).expect("collect topology"),
            [PartitionParent {
                child: (259, 1),
                parent: (259, 0)
            }]
        );
    }

    #[test]
    fn leaves_whole_layered_and_unresolved_devices_opaque() {
        let directory = tempdir().expect("create sysfs fixture");
        let root = directory.path();
        let dm = root.join("devices/virtual/block/dm-0");
        std::fs::create_dir_all(root.join("dev/block")).expect("create dev block");
        std::fs::create_dir_all(&dm).expect("create dm device");
        std::fs::write(dm.join("dev"), "253:0\n").expect("write dm dev");
        symlink(
            "../../devices/virtual/block/dm-0",
            root.join("dev/block/253:0"),
        )
        .expect("link dm");
        symlink("../../devices/missing", root.join("dev/block/8:1"))
            .expect("link unresolved device");

        assert!(
            collect(&SysFs::new(root.to_path_buf()))
                .expect("collect topology")
                .is_empty()
        );
    }
}
