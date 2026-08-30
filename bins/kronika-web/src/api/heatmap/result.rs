//! Typed Heatmap result shared by HTTP and MCP transports.

use std::collections::BTreeMap;

use kronika_registry::{ColumnClass, Unit};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::Value;

use super::query::NormalizedRanking;

pub(crate) type NamedValues = BTreeMap<String, Value>;

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapBatchResult {
    pub(crate) results: Vec<HeatmapItemResult>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapItemResult {
    pub(crate) ranking: NormalizedRanking,
    /// Decimal-string latest usable metric observation. Pass it unchanged to
    /// an MCP `from`, `to`, or `at` input.
    #[serde(serialize_with = "serialize_optional_i64")]
    #[schemars(with = "Option<String>")]
    pub(crate) as_of: Option<i64>,
    pub(crate) coverage: HeatmapCoverage,
    #[serde(serialize_with = "serialize_class")]
    #[schemars(with = "String")]
    pub(crate) class: ColumnClass,
    #[serde(serialize_with = "serialize_optional_unit")]
    #[schemars(with = "Option<String>")]
    pub(crate) unit: Option<Unit>,
    pub(crate) entities: Vec<HeatmapEntity>,
    pub(crate) totals_total: Option<f64>,
    pub(crate) others_total: Option<f64>,
    #[serde(serialize_with = "serialize_u64")]
    #[schemars(with = "String")]
    pub(crate) entity_count: u64,
    #[serde(serialize_with = "serialize_u64")]
    #[schemars(with = "String")]
    pub(crate) out_of_order: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub(crate) grid: Option<HeatmapGrid>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CoverageState {
    Data,
    NoData,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(crate) struct HeatmapCoverage {
    pub(crate) state: CoverageState,
    /// Decimal-string first recorded timestamp. Pass it unchanged to an MCP
    /// `from`, `to`, or `at` input.
    #[serde(serialize_with = "serialize_optional_i64")]
    #[schemars(with = "Option<String>")]
    pub(crate) recorded_from: Option<i64>,
    /// Decimal-string last recorded timestamp. Pass it unchanged to an MCP
    /// `from`, `to`, or `at` input.
    #[serde(serialize_with = "serialize_optional_i64")]
    #[schemars(with = "Option<String>")]
    pub(crate) recorded_to: Option<i64>,
    #[serde(serialize_with = "serialize_optional_i64")]
    #[schemars(with = "Option<String>")]
    pub(crate) nearest_row_before: Option<i64>,
    #[serde(serialize_with = "serialize_optional_i64")]
    #[schemars(with = "Option<String>")]
    pub(crate) nearest_row_after: Option<i64>,
    #[serde(serialize_with = "serialize_u64")]
    #[schemars(with = "String")]
    pub(crate) window_rows: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapEntity {
    #[serde(serialize_with = "serialize_u32")]
    #[schemars(with = "String")]
    pub(crate) type_id: u32,
    pub(crate) identity: NamedValues,
    pub(crate) labels: NamedValues,
    pub(crate) total: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[schemars(skip)]
    pub(crate) cells: Option<Vec<Option<f64>>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapGrid {
    pub(crate) label_names: Vec<String>,
    pub(crate) group_names: Vec<String>,
    pub(crate) intervals: Vec<HeatmapInterval>,
    pub(crate) groups: Vec<HeatmapGroup>,
    pub(crate) totals: HeatmapBand,
    pub(crate) others: HeatmapBand,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, JsonSchema)]
pub(crate) struct HeatmapInterval {
    #[serde(serialize_with = "serialize_i64")]
    #[schemars(with = "String")]
    pub(crate) start: i64,
    #[serde(serialize_with = "serialize_i64")]
    #[schemars(with = "String")]
    pub(crate) end: i64,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapGroup {
    pub(crate) values: Vec<Value>,
    pub(crate) members: u32,
    pub(crate) total: Option<f64>,
    pub(crate) cells: Vec<Option<f64>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, JsonSchema)]
pub(crate) struct HeatmapBand {
    pub(crate) total: Option<f64>,
    pub(crate) cells: Vec<Option<f64>>,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the field by reference"
)]
fn serialize_i64<S>(value: &i64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the field by reference"
)]
fn serialize_u32<S>(value: &u32, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde serialize_with passes the field by reference"
)]
fn serialize_u64<S>(value: &u64, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    serializer.serialize_str(&value.to_string())
}

#[expect(
    clippy::ref_option,
    reason = "serde serialize_with passes the optional field by reference"
)]
fn serialize_optional_i64<S>(value: &Option<i64>, serializer: S) -> Result<S::Ok, S::Error>
where
    S: serde::Serializer,
{
    match value {
        Some(value) => serializer.serialize_some(&value.to_string()),
        None => serializer.serialize_none(),
    }
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
