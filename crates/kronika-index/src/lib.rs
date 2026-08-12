//! A small allowlist of presentation series for immutable and captured Kronika
//! segments. Raw and unlisted metrics stay only in ZMS/WAL.

mod build;
mod detect;
mod file;
mod findings;
mod health;
mod series;
mod store;

pub use build::{
    BuildError, DERIVED_HEALTH_TYPE_ID, INSTANCE_METADATA_TYPE_ID, INSTANCE_METADATA_V1_TYPE_ID,
    OS_PSI_TYPE_ID, build, build_from_reader, build_selected, keys, visit_health_points,
};
pub use file::{ENTRY_LEN, HEADER_LEN, Index, IndexError, MAGIC, TargetedIndex};
pub use findings::{
    Finding, FindingBlock, FindingKind, MAX_FINDINGS_PER_BLOCK, PriorValue, is_upward_spike,
    select_baseline, upper_tukey_fence,
};
pub use health::{SourcePenalty, Stall, health, overall_health, postgres_penalty};
pub use series::{
    ActiveBackendPoint, HealthPoint, SeriesBlock, SeriesKey, SeriesKind, TransactionPoint,
};
pub use store::{
    EXTENSION, LoadError, ResourceIndex, finding_keys, path_of, read, resource, resource_selected,
    series_keys,
};
