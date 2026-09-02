//! A small allowlist of presentation series for immutable and captured Kronika
//! segments. Raw and unlisted metrics stay only in ZMS/WAL.

mod build;
mod detect;
mod file;
mod findings;
mod health;
mod selection;
mod series;
#[cfg(feature = "posix")]
mod store;

#[cfg(feature = "posix")]
pub use build::build_from_reader;
pub use build::{
    BuildError, DERIVED_HEALTH_TYPE_ID, INSTANCE_METADATA_TYPE_ID, INSTANCE_METADATA_V1_TYPE_ID,
    OS_PSI_TYPE_ID, build, build_selected, keys, visit_health_points,
};
pub use file::{ENTRY_LEN, HEADER_LEN, Index, IndexError, MAGIC, TargetedIndex};
pub use findings::{Finding, FindingBlock, FindingKind, MAX_FINDINGS_PER_BLOCK};
pub use health::{SourcePenalty, Stall, health, overall_health, postgres_penalty};
#[cfg(feature = "posix")]
pub use selection::{finding_keys, series_keys};
pub use selection::{finding_keys_for_sections, series_keys_for_sections};
pub use series::{
    ActiveBackendPoint, HealthPoint, SeriesBlock, SeriesKey, SeriesKind, TransactionPoint,
};
#[cfg(feature = "posix")]
pub use store::{EXTENSION, LoadError, ResourceIndex, path_of, read, resource, resource_selected};
