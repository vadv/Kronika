//! Health, and the index files a dashboard reads instead of every segment.
//!
//! An `.idx` sits next to its `.zms` and holds what a dashboard needs without
//! reopening the segment. Today that is health and nothing else.

mod build;
mod file;
mod health;

pub use build::{OS_PSI_TYPE_ID, points, stalls};
pub use file::{FORMAT_VERSION, HEADER_LEN, Index, IndexError, MAGIC, POINT_LEN, Point};
pub use health::{Stall, health};
