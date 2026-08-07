//! Moving typed rows into the snapshot window.

use crate::logging::{layout_id, section_name};
use anyhow::Result;
use kronika_registry::snapshot_coverage::SnapshotCoverageV1;
use kronika_writer::SectionBuffers;

/// Buffer the completeness markers collected alongside the sources.
///
/// # Errors
/// Returns an error if a section buffer is full.
pub(crate) fn push_snapshot_coverages(
    buffers: &mut SectionBuffers,
    rows: &[SnapshotCoverageV1],
) -> Result<()> {
    for &row in rows {
        buffer_row(buffers, row)?;
    }
    Ok(())
}

/// Buffer one typed snapshot row, mapping a full buffer to an error.
pub(crate) fn buffer_row<S: kronika_registry::Section + 'static>(
    buffers: &mut SectionBuffers,
    row: S,
) -> Result<()> {
    let type_id = S::CONTRACT.type_id.get();
    buffers.push(row).map_err(|_row| {
        anyhow::anyhow!(
            "section buffer is full: collection={} type_id={} layout_id={}",
            section_name(type_id),
            type_id,
            layout_id(type_id)
        )
    })
}
