use std::os::unix::fs::symlink;
use std::path::Path;

use tempfile::tempdir;

use super::{BlockEdge, collect};
use crate::SysFs;

fn device(root: &Path, dir: &str, dev: &str) {
    let path = root.join(dir);
    std::fs::create_dir_all(&path).expect("create device directory");
    std::fs::write(path.join("dev"), format!("{dev}\n")).expect("write dev");
    std::fs::create_dir_all(root.join("dev/block")).expect("create dev block");
    symlink(format!("../../{dir}"), root.join("dev/block").join(dev)).expect("link device");
}

#[test]
fn emits_an_exact_partition_marker_and_parent_dev() {
    let directory = tempdir().expect("create sysfs fixture");
    let root = directory.path();
    device(root, "devices/pci/block/nvme0n1", "259:0");
    device(root, "devices/pci/block/nvme0n1/nvme0n1p1", "259:1");
    std::fs::write(
        root.join("devices/pci/block/nvme0n1/nvme0n1p1/partition"),
        "1\n",
    )
    .expect("write partition marker");

    assert_eq!(
        collect(&SysFs::new(root.to_path_buf())).expect("collect topology"),
        [BlockEdge {
            child: (259, 1),
            parent: (259, 0)
        }]
    );
}

#[test]
fn emits_one_edge_per_slave_of_a_layered_device() {
    let directory = tempdir().expect("create sysfs fixture");
    let root = directory.path();
    device(root, "devices/pci/block/nvme0n1", "259:0");
    device(root, "devices/pci/block/nvme0n1/nvme0n1p4", "259:4");
    std::fs::write(
        root.join("devices/pci/block/nvme0n1/nvme0n1p4/partition"),
        "4\n",
    )
    .expect("write partition marker");
    device(root, "devices/pci/block/sdb", "8:16");
    device(root, "devices/virtual/block/dm-0", "252:0");
    let slaves = root.join("devices/virtual/block/dm-0/slaves");
    std::fs::create_dir_all(&slaves).expect("create slaves");
    symlink(
        "../../../../pci/block/nvme0n1/nvme0n1p4",
        slaves.join("nvme0n1p4"),
    )
    .expect("link first slave");
    symlink("../../../../pci/block/sdb", slaves.join("sdb")).expect("link second slave");

    assert_eq!(
        collect(&SysFs::new(root.to_path_buf())).expect("collect topology"),
        [
            BlockEdge {
                child: (252, 0),
                parent: (8, 16)
            },
            BlockEdge {
                child: (252, 0),
                parent: (259, 4)
            },
            BlockEdge {
                child: (259, 4),
                parent: (259, 0)
            },
        ]
    );
}

#[test]
fn leaves_plain_whole_and_unresolved_devices_without_edges() {
    let directory = tempdir().expect("create sysfs fixture");
    let root = directory.path();
    device(root, "devices/virtual/block/dm-0", "253:0");
    std::fs::create_dir_all(root.join("devices/virtual/block/dm-0/slaves")).expect("empty slaves");
    device(root, "devices/pci/block/sda", "8:0");
    symlink("../../devices/missing", root.join("dev/block/8:1")).expect("link unresolved device");

    assert!(
        collect(&SysFs::new(root.to_path_buf()))
            .expect("collect topology")
            .is_empty()
    );
}
