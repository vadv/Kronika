//! Normalized Heatmap product queries and the legacy HTTP adapter shape.

use crate::api::time::TimeRange;
use schemars::JsonSchema;
use serde::Serialize;

pub(crate) const DEFAULT_TOP: usize = 25;
pub(crate) const MAX_TOP: usize = 500;
pub(crate) const MAX_FIELDS: usize = 4;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HeatmapBatchQuery {
    pub(crate) range: TimeRange,
    pub(crate) items: Vec<HeatmapItemQuery>,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) struct HeatmapItemQuery {
    pub(crate) ranking: NormalizedRanking,
    pub(crate) view: HeatmapView,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, JsonSchema)]
pub(crate) struct NormalizedRanking {
    pub(crate) section: String,
    #[schemars(length(min = 1, max = 1))]
    pub(crate) fields: Vec<String>,
    pub(crate) top: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub(crate) enum HeatmapView {
    RankingOnly,
    #[cfg_attr(
        not(test),
        expect(
            dead_code,
            reason = "the retained HTTP oracle constructs grid views in tests"
        )
    )]
    Grid {
        columns: usize,
        group: Vec<String>,
        type_id: Option<u32>,
    },
}

impl HeatmapView {
    pub(crate) const fn type_id(&self) -> Option<u32> {
        match self {
            Self::RankingOnly => None,
            Self::Grid { type_id, .. } => *type_id,
        }
    }

    pub(crate) fn groups(&self) -> &[String] {
        match self {
            Self::RankingOnly => &[],
            Self::Grid { group, .. } => group,
        }
    }

    pub(crate) const fn columns(&self) -> usize {
        match self {
            Self::RankingOnly => 1,
            Self::Grid { columns, .. } => *columns,
        }
    }
}

/// The existing HTTP transport shape. Its `to` remains inclusive; conversion
/// to [`HeatmapBatchQuery`] performs the checked half-open normalization.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct HeatmapRequest {
    pub(crate) from: i64,
    pub(crate) to: i64,
    pub(crate) section: String,
    pub(crate) fields: Vec<String>,
    pub(crate) columns: usize,
    pub(crate) top: usize,
    pub(crate) group: Vec<String>,
    pub(crate) type_id: Option<u32>,
}

#[cfg(test)]
impl HeatmapRequest {
    pub(crate) fn normalize(self) -> Result<HeatmapBatchQuery, LegacyRangeError> {
        let to_exclusive = self.to.checked_add(1).ok_or(LegacyRangeError)?;
        let range = TimeRange::new(self.from, to_exclusive).map_err(|_error| LegacyRangeError)?;
        Ok(HeatmapBatchQuery {
            range,
            items: vec![HeatmapItemQuery {
                ranking: NormalizedRanking {
                    section: self.section,
                    fields: self.fields,
                    top: self.top,
                },
                view: HeatmapView::Grid {
                    columns: self.columns,
                    group: self.group,
                    type_id: self.type_id,
                },
            }],
        })
    }
}

#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct LegacyRangeError;

#[cfg(test)]
impl std::fmt::Display for LegacyRangeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the inclusive heatmap window cannot be represented as [from,to)")
    }
}

#[cfg(test)]
impl std::error::Error for LegacyRangeError {}

#[cfg(test)]
mod tests {
    use super::{HeatmapRequest, HeatmapView};

    fn request(to: i64) -> HeatmapRequest {
        HeatmapRequest {
            from: 5,
            to,
            section: "os_process".to_owned(),
            fields: vec!["utime".to_owned()],
            columns: 60,
            top: 25,
            group: Vec::new(),
            type_id: None,
        }
    }

    #[test]
    fn legacy_inclusive_end_becomes_half_open() {
        let query = request(9).normalize().expect("query");
        assert_eq!(query.range.from, 5);
        assert_eq!(query.range.to_exclusive, 10);
        assert!(matches!(query.items[0].view, HeatmapView::Grid { .. }));
        assert!(request(i64::MAX).normalize().is_err());
    }
}
