//! Converts MCP filter arrays to the snapshot search engine.
//! Predicates are combined with AND; OR and nested groups are not accepted.

use schemars::JsonSchema;
use serde::Deserialize;

use crate::api::snapshot::search::{
    Expr, Quantity, SEARCH_MAX_CLAUSES, SEARCH_MAX_VALUE_CHARS, SearchClause, SearchField,
    SearchFieldKind, SearchOperator, SearchValue, StructuredSearch, search_fields,
    valid_identifier,
};

#[derive(Debug, Clone, Copy, Deserialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Op {
    /// Whole-value match: case-insensitive equality on a string field,
    /// exact value on an identifier field.
    Eq,
    Gt,
    Lt,
    /// Case-insensitive substring match, string fields only.
    Contains,
}

/// One predicate. Tool handlers combine all predicates with AND; at most
/// 8 predicates per call, the same clause budget the text parser
/// enforces.
#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct FilterInput {
    /// Canonical field named in the enclosing tool's filters description.
    pub(crate) field: String,
    /// `eq` accepts text or identifiers; `gt` and `lt` accept quantities;
    /// `contains` accepts text. On text, `eq` is whole-value equality
    /// with `*` and `?` literal; `contains` is a substring glob with `*`
    /// and `?` wildcards. Both are case-insensitive.
    pub(crate) op: Op,
    /// Text uses a JSON string of at most 256 characters. Identifiers use
    /// an integer or decimal string. Quantities use a non-negative JSON
    /// integer in the field's base unit: bytes, bytes/s, count, count/s,
    /// milliseconds, milliseconds/s, percentage points, or a unitless
    /// value. Missing values match neither `gt` nor `lt`.
    pub(crate) value: serde_json::Value,
}

/// A refused filter together with the values that a retry can pick from.
#[derive(Debug)]
pub(crate) struct Refusal {
    pub(crate) message: String,
    pub(crate) valid_options: Vec<String>,
}

impl Refusal {
    const fn new(message: String) -> Self {
        Self {
            message,
            valid_options: Vec::new(),
        }
    }

    pub(crate) fn into_error(self) -> rmcp::model::CallToolResult {
        super::semantics::mcp_error_with(self.message, self.valid_options)
    }
}

const fn op_name(op: Op) -> &'static str {
    match op {
        Op::Eq => "eq",
        Op::Gt => "gt",
        Op::Lt => "lt",
        Op::Contains => "contains",
    }
}

fn accepted_ops(field: &SearchField) -> Vec<String> {
    [Op::Eq, Op::Gt, Op::Lt, Op::Contains]
        .into_iter()
        .filter(|op| operator_for(field, *op).is_some())
        .map(|op| op_name(op).to_owned())
        .collect()
}

/// Combines filters with AND. Empty input returns `None`; invalid input reports
/// the first rejected predicate together with the accepted alternatives.
pub(crate) fn build_search(
    logical_name: &str,
    filters: &[FilterInput],
) -> Result<Option<StructuredSearch>, Refusal> {
    if filters.is_empty() {
        return Ok(None);
    }
    if filters.len() > SEARCH_MAX_CLAUSES {
        return Err(Refusal::new(format!(
            "too many filters: {}, the limit is {SEARCH_MAX_CLAUSES}",
            filters.len()
        )));
    }
    let fields = search_fields(logical_name);
    let mut expr: Option<Expr> = None;
    let mut clauses = Vec::with_capacity(filters.len());
    for filter in filters {
        let field = fields
            .iter()
            .find(|candidate| candidate.key == filter.field)
            .ok_or_else(|| {
                let names: Vec<String> = fields.iter().map(|field| field.key.to_owned()).collect();
                Refusal {
                    message: format!(
                        "unknown field for {logical_name}: {}; the filterable fields are {}",
                        filter.field,
                        names.join(", "),
                    ),
                    valid_options: names,
                }
            })?;
        let operator = operator_for(field, filter.op).ok_or_else(|| {
            let accepted = accepted_ops(field);
            Refusal {
                message: format!(
                    "operator {} is not valid for field {}: it accepts {}",
                    op_name(filter.op),
                    filter.field,
                    accepted.join(", "),
                ),
                valid_options: accepted,
            }
        })?;
        if let Some(text) = filter.value.as_str()
            && text.chars().count() > SEARCH_MAX_VALUE_CHARS
        {
            return Err(Refusal::new(format!(
                "value for {} is longer than {SEARCH_MAX_VALUE_CHARS} characters",
                filter.field
            )));
        }
        let value = search_value(field, filter.op, &filter.value).ok_or_else(|| {
            Refusal::new(format!(
                "invalid value for {}: text uses a string, identifiers use an integer or decimal string, and quantities use a non-negative integer in the documented unit",
                filter.field
            ))
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

/// `eq` on a string field builds an anchored whole-value pattern;
/// `contains` builds the substring pattern the text DSL uses. Both are
/// case-insensitive — the distinction is anchoring, not case.
fn search_value(field: &SearchField, op: Op, value: &serde_json::Value) -> Option<SearchValue> {
    match (field.kind, op) {
        (SearchFieldKind::String, Op::Eq) => value.as_str().map(SearchValue::exact),
        (SearchFieldKind::String, _) => value.as_str().map(SearchValue::pattern),
        (SearchFieldKind::Identifier { signed }, _) => identifier_value(value, signed),
        (SearchFieldKind::Quantity(_), _) => quantity_value(value),
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
