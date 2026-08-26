//! Converts MCP filter arrays to the snapshot search engine.
//! Predicates are combined with AND; OR and nested groups are not accepted.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::api::snapshot::search::{
    Expr, Quantity, SearchClause, SearchField, SearchFieldKind, SearchOperator, SearchValue,
    StructuredSearch, search_fields, valid_identifier,
};

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Op {
    Eq,
    Gt,
    Lt,
    Contains,
}

/// One predicate. Tool handlers combine all predicates with AND.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FilterInput {
    /// Canonical field named in the enclosing tool's filters description.
    pub(crate) field: String,
    /// `eq` accepts text or identifiers; `gt` and `lt` accept quantities;
    /// `contains` accepts text. Text `eq` and `contains` both use a
    /// case-insensitive substring glob; `*` and `?` are wildcards.
    pub(crate) op: Op,
    /// Text uses a JSON string. Identifiers use an integer or decimal string.
    /// Quantities use a non-negative JSON integer in the field's base unit:
    /// bytes, bytes/s, count, count/s, milliseconds, milliseconds/s,
    /// percentage points, or a unitless value. Missing values match neither
    /// `gt` nor `lt`.
    pub(crate) value: serde_json::Value,
}

/// Combines filters with AND. Empty input returns `None`; invalid input reports
/// the first rejected predicate.
pub(crate) fn build_search(
    logical_name: &str,
    filters: &[FilterInput],
) -> Result<Option<StructuredSearch>, String> {
    if filters.is_empty() {
        return Ok(None);
    }
    let fields = search_fields(logical_name);
    let mut expr: Option<Expr> = None;
    let mut clauses = Vec::with_capacity(filters.len());
    for filter in filters {
        let field = fields
            .iter()
            .find(|candidate| candidate.key == filter.field)
            .ok_or_else(|| format!("unknown field for {logical_name}: {}", filter.field))?;
        let operator = operator_for(field, filter.op)
            .ok_or_else(|| format!("operator is not valid for field {}", filter.field))?;
        let value = search_value(field, &filter.value).ok_or_else(|| {
            format!(
                "invalid value for {}: text uses a string, identifiers use an integer or decimal string, and quantities use a non-negative integer in the documented unit",
                filter.field
            )
        })?;
        let clause = SearchClause::from_parts(field.key, field.columns, operator, value);
        clauses.push(clause.clone());
        expr = Some(match expr {
            None => Expr::Predicate(clause),
            Some(existing) => Expr::And(Box::new(existing), Box::new(Expr::Predicate(clause))),
        });
    }
    Ok(Some(StructuredSearch::from_expr(
        expr.expect("filters is non-empty, so expr is Some"),
        clauses,
    )))
}

const fn operator_for(field: &SearchField, op: Op) -> Option<SearchOperator> {
    match (field.kind, op) {
        (SearchFieldKind::String, Op::Eq | Op::Contains)
        | (SearchFieldKind::Identifier { .. }, Op::Eq) => Some(SearchOperator::Colon),
        (SearchFieldKind::Quantity(_), Op::Gt) => Some(SearchOperator::Greater),
        (SearchFieldKind::Quantity(_), Op::Lt) => Some(SearchOperator::Less),
        _ => None,
    }
}

fn search_value(field: &SearchField, value: &serde_json::Value) -> Option<SearchValue> {
    match field.kind {
        SearchFieldKind::String => value.as_str().map(SearchValue::pattern),
        SearchFieldKind::Identifier { signed } => identifier_value(value, signed),
        SearchFieldKind::Quantity(_) => quantity_value(value),
    }
}

/// Accepts identifiers as JSON integers or canonical decimal strings; strings
/// preserve signed 64-bit IDs beyond JSON's safe range.
fn identifier_value(value: &serde_json::Value, signed: bool) -> Option<SearchValue> {
    let text = match value {
        serde_json::Value::String(text) => text.clone(),
        serde_json::Value::Number(number) if signed => number.as_i64()?.to_string(),
        serde_json::Value::Number(number) => number.as_u64()?.to_string(),
        _ => return None,
    };
    valid_identifier(&text, signed).then_some(SearchValue::Identifier(text))
}

fn quantity_value(value: &serde_json::Value) -> Option<SearchValue> {
    let count = value.as_u64()?;
    Some(SearchValue::Quantity(Quantity::from_integer(u128::from(
        count,
    ))))
}

#[cfg(test)]
mod tests;
