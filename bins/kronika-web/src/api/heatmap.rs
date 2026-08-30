//! Shared ordered Heatmap product query, execution, and result.

mod execution;
mod query;
mod result;

pub(crate) use execution::{HeatmapError, PreparedHeatmap, prepare, prepare_batch};
pub(crate) use query::{
    DEFAULT_TOP, HeatmapBatchQuery, HeatmapItemQuery, HeatmapRequest, HeatmapView, MAX_TOP,
    NormalizedRanking,
};
pub(crate) use result::HeatmapBatchResult;

#[cfg(test)]
mod tests;

#[cfg(test)]
mod real_fixture;
