//! Shared accepted presentation semantics for Rust products and UI decoration.
//! Indexed finding boundaries and health formulas remain in `kronika-index`.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::OnceLock;

use kronika_index::{
    SemanticBoundary as IndexBoundary, SemanticDefinition as IndexDefinition,
    SemanticOperator as IndexOperator, SemanticOrigin as IndexOrigin, SemanticUnit as IndexUnit,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const STORED: &str = include_str!("../product-semantics.json");

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SemanticDefinition {
    pub(crate) id: String,
    pub(crate) origin: SemanticOrigin,
    pub(crate) unit: Option<SemanticUnit>,
    pub(crate) formula: Option<String>,
    pub(crate) operands: Vec<String>,
    pub(crate) thresholds: Vec<SemanticThreshold>,
    pub(crate) expected_band: Option<ExpectedBand>,
    pub(crate) policy: SemanticPolicy,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SemanticOrigin {
    Recorded,
    KronikaDerived,
    AcceptedPresentation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SemanticUnit {
    Percent,
    Milliseconds,
    MillisecondsPerCall,
    Samples,
    SamplingIntervals,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SemanticThreshold {
    pub(crate) operator: ThresholdOperator,
    pub(crate) value: f64,
    pub(crate) tone: ValueTone,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ThresholdOperator {
    Lt,
    Gte,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ExpectedBand {
    pub(crate) min_inclusive: Option<f64>,
    pub(crate) max_exclusive: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub(crate) enum SemanticPolicy {
    NumericValueTone {
        field: String,
        active_client_only: bool,
    },
    TextValueTone {
        field: String,
        values: BTreeMap<String, ValueTone>,
        ascii_values: bool,
        nonempty_tone: Option<ValueTone>,
    },
    RateZeroTone {
        tone: ValueTone,
    },
    VacuumEpisode {
        adjacency_factor: f64,
    },
    VacuumNoMovement {
        samples: u32,
        phases: Vec<VacuumMovement>,
    },
    VacuumRisk {
        default: VacuumRisk,
        order: Vec<VacuumRisk>,
        phases: BTreeMap<String, VacuumRisk>,
    },
    RelationSeverity {
        states: Vec<RelationState>,
    },
    EventTierOrder {
        tiers: Vec<EventTier>,
    },
    EventTier {
        section: String,
        discriminator: Option<String>,
        tiers: Vec<EventTier>,
        fallback: EventTier,
        provenance: SemanticOrigin,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ValueTone {
    Good,
    Warning,
    Critical,
    Inactive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VacuumMovement {
    pub(crate) phase: String,
    pub(crate) field: String,
    pub(crate) unavailable_type_ids: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum VacuumRisk {
    Ordinary,
    Heavy,
    Dangerous,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RelationState {
    pub(crate) valid: bool,
    pub(crate) ready: Option<bool>,
    pub(crate) severity: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EventTier {
    Critical,
    Notable,
    Routine,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProductSemanticsError(String);

impl fmt::Display for ProductSemanticsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for ProductSemanticsError {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StoredSemantics {
    version: u32,
    definitions: Vec<SemanticDefinition>,
}

static DEFINITIONS: OnceLock<Result<Box<[SemanticDefinition]>, ProductSemanticsError>> =
    OnceLock::new();

pub(crate) fn all() -> Result<&'static [SemanticDefinition], ProductSemanticsError> {
    match DEFINITIONS.get_or_init(load) {
        Ok(definitions) => Ok(definitions),
        Err(error) => Err(error.clone()),
    }
}

pub(crate) fn get(id: &str) -> Result<Option<&'static SemanticDefinition>, ProductSemanticsError> {
    Ok(all()?.iter().find(|definition| definition.id == id))
}

pub(crate) fn indexed(definition: IndexDefinition) -> Value {
    json!({
        "id": definition.id,
        "logical_name": definition.logical_name,
        "field": definition.field,
        "origin": index_origin(definition.origin),
        "source": "kronika_index",
        "unit": definition.unit.map(index_unit),
        "formula": definition.formula,
        "operands": definition.operands,
        "boundary": definition.boundary.map(index_boundary),
    })
}

pub(crate) fn findings() -> Vec<Value> {
    kronika_index::FINDING_SEMANTICS
        .iter()
        .copied()
        .map(indexed)
        .collect()
}

pub(crate) fn health() -> Vec<Value> {
    kronika_index::HEALTH_SEMANTICS
        .iter()
        .copied()
        .map(indexed)
        .collect()
}

const fn index_origin(origin: IndexOrigin) -> &'static str {
    match origin {
        IndexOrigin::KronikaDerived => "kronika_derived",
    }
}

const fn index_unit(unit: IndexUnit) -> &'static str {
    match unit {
        IndexUnit::Percent => "percent",
        IndexUnit::Milliseconds => "milliseconds",
        IndexUnit::Count => "count",
    }
}

fn index_boundary(boundary: IndexBoundary) -> Value {
    match boundary {
        IndexBoundary::Compare {
            operator,
            numerator,
            denominator,
        } => json!({
            "operator": index_operator(operator),
            "numerator": numerator.to_string(),
            "denominator": denominator.to_string(),
        }),
        IndexBoundary::Increase => json!({"operator": "increase"}),
        IndexBoundary::Nonempty => json!({"operator": "nonempty"}),
    }
}

const fn index_operator(operator: IndexOperator) -> &'static str {
    match operator {
        IndexOperator::Lt => "lt",
        IndexOperator::Lte => "lte",
        IndexOperator::Eq => "eq",
        IndexOperator::Gt => "gt",
        IndexOperator::Gte => "gte",
    }
}

fn load() -> Result<Box<[SemanticDefinition]>, ProductSemanticsError> {
    let stored: StoredSemantics = serde_json::from_str(STORED)
        .map_err(|error| ProductSemanticsError(format!("parse product semantics: {error}")))?;
    if stored.version != 1 {
        return Err(ProductSemanticsError(format!(
            "unsupported product semantics version {}",
            stored.version
        )));
    }
    let mut ids = BTreeSet::new();
    for definition in &stored.definitions {
        if definition.id.is_empty() || !ids.insert(definition.id.as_str()) {
            return Err(ProductSemanticsError(format!(
                "invalid product semantic id {}",
                definition.id
            )));
        }
    }
    Ok(stored.definitions.into_boxed_slice())
}

#[cfg(test)]
#[path = "product_semantics/tests.rs"]
mod tests;
