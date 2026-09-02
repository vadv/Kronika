//! Typed Heatmap result shared by native adapters.

use std::collections::BTreeMap;

use kronika_registry::{ColumnClass, Unit};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::query::NormalizedRanking;
use crate::row_key::{DetailLocator, serialize_decimal};

/// Stable name-to-value object used for identities and labels.
pub(crate) type NamedValues = BTreeMap<String, Value>;

/// Results in exact requested ranking order.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapBatchResult {
    /// One result per requested item, including exact duplicates.
    pub(crate) results: Vec<HeatmapItemResult>,
}

/// One ranked metric fold and optional time grid.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapItemResult {
    /// Normalized input selection.
    pub(crate) ranking: NormalizedRanking,
    /// Whether the requested window carried eligible rows.
    pub(crate) coverage: HeatmapCoverage,
    /// Registry quantity class shared by the selected fields.
    #[serde(serialize_with = "serialize_class")]
    #[schemars(with = "String")]
    pub(crate) class: ColumnClass,
    /// Registry unit shared by the selected fields, when declared.
    #[serde(serialize_with = "serialize_optional_unit")]
    #[schemars(with = "Option<String>")]
    pub(crate) unit: Option<Unit>,
    /// Ranked entities in semantic order.
    pub(crate) entities: Vec<HeatmapEntity>,
    /// Authoritative total across every eligible entity.
    pub(crate) totals_total: Option<f64>,
    /// Total outside the retained top entities.
    pub(crate) others_total: Option<f64>,
    /// Complete eligible entity count before top-N truncation.
    #[serde(serialize_with = "serialize_decimal")]
    #[schemars(with = "String")]
    pub(crate) entity_count: u64,
    /// Rows whose physical timestamp order moved backwards.
    #[serde(serialize_with = "serialize_decimal")]
    #[schemars(with = "String")]
    pub(crate) out_of_order: u64,
    /// Bucketed output for a grid view.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub(crate) grid: Option<HeatmapGrid>,
}

/// Whether a ranking window contained eligible data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageState {
    /// At least one eligible row was observed.
    Data,
    /// No eligible rows were observed.
    NoData,
}

/// Coverage facts for one ranking.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(crate) struct HeatmapCoverage {
    /// Data/no-data state.
    pub(crate) state: CoverageState,
    /// Eligible physical rows in the selected window.
    #[serde(serialize_with = "serialize_decimal")]
    #[schemars(with = "String")]
    pub(crate) window_rows: u64,
}

/// One retained ranked entity.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapEntity {
    /// Stable public identity.
    pub(crate) identity: NamedValues,
    /// Display fields captured from the representative row.
    pub(crate) labels: NamedValues,
    pub(crate) detail_locator: DetailLocator,
    /// Folded total used for ranking.
    pub(crate) total: Option<f64>,
    /// Per-column values for a grid view.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub(crate) cells: Option<Vec<Option<f64>>>,
}

/// Grid labels, intervals, groups, totals, and remainder.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapGrid {
    /// Entity label fields in output order.
    pub(crate) label_names: Vec<String>,
    /// Aggregation fields in output order.
    pub(crate) group_names: Vec<String>,
    /// Equal time intervals with inclusive endpoints.
    pub(crate) intervals: Vec<HeatmapInterval>,
    /// Aggregated entity groups.
    pub(crate) groups: Vec<HeatmapGroup>,
    /// All-entity band.
    pub(crate) totals: HeatmapBand,
    /// Band outside the retained entities.
    pub(crate) others: HeatmapBand,
}

/// One grid interval with inclusive endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(crate) struct HeatmapInterval {
    /// Inclusive interval start.
    #[serde(serialize_with = "serialize_decimal")]
    #[schemars(with = "String")]
    pub(crate) start: i64,
    /// Inclusive interval end.
    #[serde(serialize_with = "serialize_decimal")]
    #[schemars(with = "String")]
    pub(crate) end: i64,
}

/// One aggregated entity group in a grid.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapGroup {
    /// Group values in `group_names` order.
    pub(crate) values: Vec<Value>,
    /// Retained entities represented by this group.
    pub(crate) members: u32,
    /// Folded total for the group.
    pub(crate) total: Option<f64>,
    /// Per-column group values.
    pub(crate) cells: Vec<Option<f64>>,
}

/// Total and per-column values for one grid band.
#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapBand {
    /// Folded band total.
    pub(crate) total: Option<f64>,
    /// Per-column band values.
    pub(crate) cells: Vec<Option<f64>>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the field by reference"
)]
fn serialize_class<S>(value: &ColumnClass, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(value.code())
}

#[expect(
    clippy::ref_option,
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the optional field by reference"
)]
fn serialize_optional_unit<S>(value: &Option<Unit>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(value.code()),
        None => serializer.serialize_none(),
    }
}
