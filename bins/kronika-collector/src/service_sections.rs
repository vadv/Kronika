//! The per-segment identity row.

use crate::buffering::buffer_row;
use crate::logging::{log_collection_failure, log_collection_finish, log_collection_start};
use crate::scheduler::{DueSet, SourceKind};
use anyhow::{Context, Result};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::{StrId, Ts};
use kronika_source_os::{OsInstanceFacts, collect_os_instance_facts};
use kronika_writer::{Interner, SectionBuffers};
use std::time::Instant;

const INSTANCE_METADATA_TYPE_ID: u32 = 1_021_001;

/// Read the host identity when the scheduler says it is due.
///
/// # Errors
///
/// Returns an error naming the `/proc` file that could not be read. The
/// identity is what makes a sealed segment self-contained, so a failure here
/// is not a degraded section.
pub(crate) fn collect_due_instance(due: &DueSet) -> Result<Option<OsInstanceFacts>> {
    if !due.has(SourceKind::InstanceMetadata) {
        return Ok(None);
    }
    let started = Instant::now();
    log_collection_start(INSTANCE_METADATA_TYPE_ID, "procfs");
    match collect_os_instance_facts() {
        Ok(facts) => {
            log_collection_finish(INSTANCE_METADATA_TYPE_ID, "procfs", 1, started.elapsed());
            Ok(Some(facts))
        }
        Err(err) => {
            log_collection_failure(INSTANCE_METADATA_TYPE_ID, "procfs", &err, started.elapsed());
            Err(err).context("collect OS instance facts")
        }
    }
}

/// Intern the identity strings and buffer the `1_021_001` row.
///
/// `in_container` is decided at collection time and stored, so nothing
/// downstream re-derives whether these numbers describe a machine or a
/// container.
///
/// # Errors
///
/// Returns an error if a string cannot be interned or the section buffer is
/// full.
pub(crate) fn push_instance_metadata(
    buffers: &mut SectionBuffers,
    interner: &mut Interner,
    facts: &OsInstanceFacts,
    in_container: bool,
    ts: i64,
) -> Result<()> {
    let mut intern = |value: &str| -> Result<StrId> {
        interner
            .intern(value.as_bytes())
            .map(|id| StrId(id.get()))
            .map_err(|err| anyhow::anyhow!("intern instance metadata string: {err}"))
    };
    let row = InstanceMetadata {
        ts: Ts(ts),
        hostname: intern(&facts.hostname)?,
        kernel_version: intern(&facts.kernel_version)?,
        environment: Environment::from_container_flag(in_container).as_u8(),
        clock_ticks_per_sec: facts.clock_ticks_per_sec,
        page_size_bytes: facts.page_size_bytes,
        boot_id: intern(&facts.boot_id)?,
        btime: Ts(facts.btime),
    };
    buffer_row(buffers, row)
}
