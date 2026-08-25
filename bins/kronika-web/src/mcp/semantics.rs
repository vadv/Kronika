//! Shared MCP serialization for authoritative semantic definitions.

use std::borrow::Borrow;
use std::collections::BTreeSet;
use std::fmt;

use kronika_index::SemanticDefinition;
use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct SemanticError(String);

impl fmt::Display for SemanticError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for SemanticError {}

pub(super) fn indexed(definition: SemanticDefinition) -> Value {
    crate::product_semantics::indexed(definition)
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
    crate::product_semantics::health()
}

#[cfg(test)]
#[path = "semantics/tests.rs"]
mod tests;
