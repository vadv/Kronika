use super::{Instant, OsSources, SysFs, Ts, log_collection_finish, log_degraded};
use kronika_registry::os_block_topology::OsBlockTopology;
use kronika_source_os::block_topology;

pub(super) fn collect_block_topology(sys: &SysFs, scope: u8, ts: i64, os: &mut OsSources) {
    let started = Instant::now();
    let edges = match block_topology::collect(sys) {
        Ok(edges) => edges,
        Err(error) => {
            log_degraded(1_123_001, "sysfs/dev/block", &error);
            return;
        }
    };
    os.block_topology = edges
        .into_iter()
        .map(|edge| OsBlockTopology {
            ts: Ts(ts),
            major: edge.child.0,
            minor: edge.child.1,
            parent_major: edge.parent.0,
            parent_minor: edge.parent.1,
            scope,
        })
        .collect();
    if !os.block_topology.is_empty() {
        log_collection_finish(
            1_123_001,
            "sysfs",
            os.block_topology.len(),
            started.elapsed(),
        );
    }
}
