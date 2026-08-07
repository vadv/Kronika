//! Completeness provenance for the sources that can be truncated.

use kronika_registry::Ts;
use kronika_registry::snapshot_coverage::SnapshotCoverageV1;
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// Process-session start, used to tell one collector run from the next after a
/// restart within the same segment.
fn collector_started_at_us() -> i64 {
    static STARTED_AT: OnceLock<i64> = OnceLock::new();
    *STARTED_AT.get_or_init(|| {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_micros()).ok())
            .unwrap_or(0)
    })
}

/// Build immutable provenance for one attempted multi-row snapshot.
pub(crate) fn snapshot_coverage(
    ts: i64,
    section_type_id: u32,
    read_state: u8,
    visibility: u8,
    source_total: u64,
    collected: usize,
) -> SnapshotCoverageV1 {
    SnapshotCoverageV1 {
        ts: Ts(ts),
        section_type_id,
        collector_pid: std::process::id(),
        collector_started_at: Ts(collector_started_at_us()),
        read_state,
        visibility,
        source_total: u32::try_from(source_total).unwrap_or(u32::MAX),
        collected: u32::try_from(collected).unwrap_or(u32::MAX),
    }
}
