//! Health, and the index files a dashboard reads instead of every segment.
//!
//! An `.idx` sits next to its `.zms` and holds what a dashboard needs without
//! reopening the segment: the health line, and the objects every section saw.

mod build;
mod file;
mod health;
mod objects;

pub use build::{INSTANCE_METADATA_TYPE_ID, OS_PSI_TYPE_ID, points};
pub use file::{ENTRY_LEN, HEADER_LEN, Index, IndexError, MAGIC, POINT_LEN, Point};
pub use health::{Stall, health};
pub use objects::{Object, SectionObjects, Value};
