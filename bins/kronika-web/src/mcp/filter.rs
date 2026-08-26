//! Typed MCP filter input, converted into the same `Expr`/`SearchClause`
//! tree the HTTP structured-search text parser builds — so filtering goes
//! through the one already-tested matching engine, just fed from JSON
//! instead of parsed from a query string. Deliberately flat AND only: no
//! OR, no nested groups (a session that needs OR semantics calls a `find_*`
//! tool twice, or filters the results itself).

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

/// One AND-ed predicate: `field <op> value`.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FilterInput {
    /// Field name on the target section — see the tool's own schema for
    /// the exact allowed set; it differs per entity.
    pub(crate) field: String,
    pub(crate) op: Op,
    /// A JSON string for text/identifier fields, or a non-negative integer
    /// for quantity fields (raw bytes, milliseconds, a count, or a 0-100
    /// percentage — whatever base unit the field uses).
    pub(crate) value: serde_json::Value,
}

/// Build a `StructuredSearch`-equivalent expression tree from typed
/// filters, or `Ok(None)` when there are no filters at all. `Err` names the
/// first invalid field/operator/value combination.
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
        let operator = operator_for(field, filter.op).ok_or_else(|| {
            format!(
                "field {} does not support operator {:?}",
                filter.field, filter.op
            )
        })?;
        let value = search_value(field, &filter.value)
            .ok_or_else(|| format!("value does not match field kind for {}", filter.field))?;
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

/// Map a typed `Op` to the 3-variant `SearchOperator` the matching engine
/// understands, rejecting every combination the engine cannot express.
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

/// Accept either a JSON string or a JSON number: a `pid` fits comfortably
/// in a JSON number, but a `query_id` is a full `i64` that can exceed
/// JSON's safe-integer range, so a caller that already has one as a
/// decimal string (the same convention `DecimalI64` uses on the way out)
/// can pass it through unchanged.
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
