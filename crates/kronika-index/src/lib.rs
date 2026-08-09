//! Targeted derived summaries for immutable and captured Kronika segments.
//!
//! A finished `.idx` has one independently decodable block per physical
//! registry layout plus the ordinary derived `health` series. It stores exact
//! declared identities and bounded first/last observations, never arbitrary
//! label columns or every timestamped sample.

mod build;
mod file;
mod health;
mod store;
mod summary;

pub use build::{
    BuildError, DERIVED_HEALTH_TYPE_ID, INSTANCE_METADATA_TYPE_ID, OS_PSI_TYPE_ID, build,
    build_selected,
};
pub use file::{ENTRY_LEN, HEADER_LEN, Index, IndexError, MAGIC, TargetedIndex};
pub use health::{Stall, health};
pub use store::{EXTENSION, LoadError, ResourceIndex, path_of, read, resource};
pub use summary::{IdentityValue, Number, ObjectSummary, Observation, Sample, SectionSummary};
