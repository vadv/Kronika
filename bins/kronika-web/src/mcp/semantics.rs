//! Shared MCP serialization for authoritative semantic definitions.

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fmt;

use kronika_index::{
    SemanticBoundary, SemanticDefinition, SemanticOperator, SemanticOrigin as IndexOrigin,
    SemanticUnit as IndexUnit,
};
use serde_json::{Map, Value, json};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticError(String);

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SemanticError {}

pub(super) fn accepted(id: &str) -> Result<Value, SemanticError> {
    let definition = crate::product_semantics::get(id)
        .map_err(|error| SemanticError(error.to_string()))?
        .ok_or_else(|| SemanticError(format!("missing accepted semantic {id}")))?;
    let mut value = serde_json::to_value(definition)
        .map_err(|error| SemanticError(format!("serialize accepted semantic {id}: {error}")))?;
    value
        .as_object_mut()
        .ok_or_else(|| SemanticError(format!("accepted semantic {id} is not an object")))?
        .insert("source".to_owned(), json!("kronika_product_registry"));
    Ok(value)
}

pub(super) fn indexed(definition: SemanticDefinition) -> Value {
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

pub(super) fn referenced<T: Borrow<Value>>(
    records: impl IntoIterator<Item = T>,
) -> Result<Vec<Value>, SemanticError> {
    let mut ids = BTreeSet::new();
    for record in records {
        if let Some(id) = record.borrow().get("semantic_id") {
            ids.insert(
                id.as_str()
                    .ok_or_else(|| SemanticError("semantic_id is not textual".to_owned()))?
                    .to_owned(),
            );
        }
    }
    ids.into_iter()
        .map(|id| {
            kronika_index::semantic_definition(&id)
                .map(indexed)
                .ok_or_else(|| SemanticError(format!("missing indexed semantic {id}")))
        })
        .collect()
}

pub(super) fn health() -> Vec<Value> {
    kronika_index::HEALTH_SEMANTICS
        .iter()
        .copied()
        .map(indexed)
        .collect()
}

pub(super) fn recorded_layout(layout: &Value) -> Result<Value, SemanticError> {
    let object = layout
        .as_object()
        .ok_or_else(|| SemanticError("recorded layout is not an object".to_owned()))?;
    let logical_name = text(object, "logical_name")?;
    let type_id = text(object, "type_id")?;
    Ok(json!({
        "id": format!("layout.{type_id}"),
        "origin": "recorded",
        "source": "kronika_registry",
        "logical_name": logical_name,
        "type_id": type_id,
        "layout": layout,
    }))
}

fn text<'a>(object: &'a Map<String, Value>, field: &str) -> Result<&'a str, SemanticError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .ok_or_else(|| SemanticError(format!("recorded layout has no textual {field}")))
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

fn index_boundary(boundary: SemanticBoundary) -> Value {
    match boundary {
        SemanticBoundary::Compare {
            operator,
            numerator,
            denominator,
        } => json!({
            "operator": index_operator(operator),
            "numerator": numerator.to_string(),
            "denominator": denominator.to_string(),
        }),
        SemanticBoundary::Increase => json!({"operator": "increase"}),
        SemanticBoundary::Nonempty => json!({"operator": "nonempty"}),
    }
}

const fn index_operator(operator: SemanticOperator) -> &'static str {
    match operator {
        SemanticOperator::Lt => "lt",
        SemanticOperator::Lte => "lte",
        SemanticOperator::Eq => "eq",
        SemanticOperator::Gt => "gt",
        SemanticOperator::Gte => "gte",
    }
}

#[cfg(test)]
#[path = "semantics/tests.rs"]
mod tests;
