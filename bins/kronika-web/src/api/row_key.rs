//! Stable row identity carried alongside physical locator hints.

use kronika_reader::{Cell, Row};
use kronika_registry::{ColumnClass, Semantics, TypeContract, contract};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

/// Opaque registry identity copied unchanged between mass and detail tools.
pub(crate) type RowIdentity = Map<String, Value>;

/// Ready-to-use input for `kronika_get_row_detail`.
#[derive(Debug, Clone, PartialEq, Deserialize, Serialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub(crate) struct DetailLocator {
    /// Recorded logical section.
    pub(crate) section: String,
    /// Signed 64-bit segment ID as a JSON integer or decimal string.
    pub(crate) segment_id: Value,
    /// Signed 64-bit row timestamp as a JSON integer or decimal string.
    pub(crate) at: Value,
    /// Unsigned 32-bit physical layout ID as a JSON integer or decimal string.
    pub(crate) type_id: Value,
    /// Unsigned 64-bit physical row hint as a JSON integer or decimal string.
    pub(crate) row_ordinal: Value,
    /// Complete opaque registry identity; copy every member unchanged.
    pub(crate) identity: RowIdentity,
}

/// Builds one stable locator. The ordinal is only a physical hint.
pub(crate) fn detail_locator(
    section: &str,
    segment_id: i64,
    at: i64,
    type_id: u32,
    row_ordinal: u64,
    identity: RowIdentity,
) -> DetailLocator {
    DetailLocator {
        section: section.to_owned(),
        segment_id: Value::String(segment_id.to_string()),
        at: Value::String(at.to_string()),
        type_id: Value::String(type_id.to_string()),
        row_ordinal: Value::String(row_ordinal.to_string()),
        identity,
    }
}

/// Columns forming the durable locator identity for a physical layout.
///
/// Snapshot-like sources use their declared cross-snapshot registry identity.
/// Event streams can repeat a semantic key at one timestamp, so the complete
/// non-timestamp stored row is their identity.
pub(crate) fn identity_columns(
    contract: &'static TypeContract,
) -> impl Iterator<Item = &'static str> {
    let event = contract.semantics == Semantics::EventStream;
    contract
        .columns
        .iter()
        .filter(move |column| {
            if event {
                column.class != ColumnClass::Timestamp
            } else {
                contract.identity.contains(&column.name)
            }
        })
        .map(|column| column.name)
}

/// Encodes one row's identity without resolving dictionary-backed payloads.
pub(crate) fn identity(type_id: u32, row: &Row) -> Result<RowIdentity, String> {
    let contract =
        contract(type_id).ok_or_else(|| format!("type_id {type_id} has no registry contract"))?;
    if contract.type_id.get() != row.contract().type_id.get() {
        return Err(format!(
            "type_id {type_id} does not match decoded row type_id {}",
            row.contract().type_id.get()
        ));
    }
    identity_columns(contract)
        .map(|name| {
            row.get(name)
                .map(|cell| (name.to_owned(), identity_value(cell)))
                .ok_or_else(|| format!("type_id {type_id} identity column {name:?} is absent"))
        })
        .collect()
}

/// Validates that an input carries exactly the registry identity members.
pub(crate) fn validate(type_id: u32, requested: &RowIdentity) -> Result<(), String> {
    let contract =
        contract(type_id).ok_or_else(|| format!("type_id {type_id} has no registry contract"))?;
    let expected = identity_columns(contract).collect::<Vec<_>>();
    let missing = expected
        .iter()
        .copied()
        .filter(|name| !requested.contains_key(*name))
        .collect::<Vec<_>>();
    let extra = requested
        .keys()
        .filter(|name| !expected.contains(&name.as_str()))
        .map(String::as_str)
        .collect::<Vec<_>>();
    if missing.is_empty() && extra.is_empty() {
        return Ok(());
    }
    Err(format!(
        "invalid detail_locator identity for type_id {type_id}: expected [{}], missing [{}], unexpected [{}]",
        expected.join(", "),
        missing.join(", "),
        extra.join(", "),
    ))
}

fn identity_value(cell: &Cell) -> Value {
    match cell {
        Cell::Null => Value::Null,
        Cell::Bool(value) => Value::Bool(*value),
        Cell::I16(value) => Value::String(value.to_string()),
        Cell::I32(value) => Value::String(value.to_string()),
        Cell::I64(value) | Cell::Ts(value) => Value::String(value.to_string()),
        Cell::U32(value) => Value::String(value.to_string()),
        Cell::U64(value) | Cell::StrId(value) => Value::String(value.to_string()),
        Cell::F64(value) => Value::String(format!("f64:{:016x}", value.to_bits())),
        Cell::ListI32(values) => Value::Array(
            values
                .iter()
                .map(|value| Value::String(value.to_string()))
                .collect(),
        ),
    }
}

/// Stored text kept out of mass results and returned only by row detail.
pub(crate) fn is_detail_text(section: &str, field: &str) -> bool {
    matches!(
        (section, field),
        ("os_process", "cmdline")
            | (
                "pg_stat_activity" | "pg_locks" | "pg_stat_statements",
                "query"
            )
            | ("pg_store_plans", "plan")
            | (
                "pg_log_errors",
                "sample" | "detail" | "hint" | "context" | "statement"
            )
            | ("pg_log_slow_queries", "sample")
            | ("pg_log_checkpoints", "reason")
            | ("pg_log_lock_waits", "detail" | "context" | "statement")
            | ("pg_log_temp_files", "statement")
            | ("pg_log_lifecycle", "message" | "query_detail")
            | ("pgbouncer_events", "text")
    )
}

#[cfg(test)]
mod tests;
