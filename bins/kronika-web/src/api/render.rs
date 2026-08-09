//! JS-safe JSON rendering for registry, dictionary, and index values.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use kronika_index::{IdentityValue, Number, Observation, Sample};
use kronika_reader::{Cell, Dictionary, Resolved};
use kronika_registry::{Column, ColumnType, TypeContract, section_implementation};
use serde_json::{Value, json};

use super::ApiError;

pub(super) fn record(value: impl std::borrow::Borrow<Value>) -> Result<Vec<u8>, ApiError> {
    let mut bytes = serde_json::to_vec(value.borrow())?;
    bytes.push(b'\n');
    Ok(bytes)
}

pub(super) fn layout(
    logical_name: &str,
    contract: &'static TypeContract,
    fields: &[&'static str],
) -> Value {
    json!({
        "logical_name": logical_name,
        "physical_name": contract.name,
        "type_id": contract.type_id.get().to_string(),
        "implementation": section_implementation(contract.type_id.get()),
        "identity": contract.identity,
        "columns": fields
            .iter()
            .filter_map(|name| contract.column(name))
            .map(column)
            .collect::<Vec<_>>(),
    })
}

pub(super) fn projected_layout(
    logical_name: &str,
    contract: &'static TypeContract,
    fields: &[(&str, Option<&Column>)],
) -> Value {
    json!({
        "logical_name": logical_name,
        "physical_name": contract.name,
        "type_id": contract.type_id.get().to_string(),
        "implementation": section_implementation(contract.type_id.get()),
        "identity": contract.identity,
        "columns": fields
            .iter()
            .map(|(name, selected)| selected.map_or_else(
                || json!({ "name": name, "available": false }),
                |column| {
                    let mut value = column_value(column);
                    value.insert("available".to_owned(), json!(true));
                    Value::Object(value)
                },
            ))
            .collect::<Vec<_>>(),
    })
}

pub(super) fn column(column: &Column) -> Value {
    Value::Object(column_value(column))
}

fn column_value(column: &Column) -> serde_json::Map<String, Value> {
    let mut value = serde_json::Map::new();
    value.insert("name".to_owned(), json!(column.name));
    value.insert("type".to_owned(), json!(column_type(column.ty)));
    value.insert("class".to_owned(), json!(column.class.code()));
    value.insert(
        "unit".to_owned(),
        json!(column.unit.map_or("none", kronika_registry::Unit::code)),
    );
    value.insert("nullable".to_owned(), json!(column.nullable));
    value
}

pub(super) fn cell(cell: &Cell, dictionary: &Dictionary) -> Result<Value, ApiError> {
    Ok(match cell {
        Cell::Null => Value::Null,
        Cell::I16(value) => json!(value),
        Cell::I32(value) => json!(value),
        Cell::I64(value) | Cell::Ts(value) => Value::String(value.to_string()),
        Cell::U32(value) => json!(value),
        Cell::U64(value) => Value::String(value.to_string()),
        Cell::F64(value) => finite(*value),
        Cell::Bool(value) => json!(value),
        Cell::ListI32(values) => json!(values),
        Cell::StrId(id) => match dictionary.resolve(*id) {
            Some(Resolved::Str(bytes)) => bytes_value(bytes),
            Some(Resolved::Blob(blob)) => blob_value(
                blob.stored_bytes,
                blob.full_len,
                blob.truncated,
                blob.full_sha256,
            ),
            None => {
                return Err(ApiError::Unreadable(Box::new(std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("unresolved dictionary id {id}"),
                ))));
            }
        },
    })
}

pub(super) fn identity(value: &IdentityValue) -> Value {
    match value {
        IdentityValue::Null => Value::Null,
        IdentityValue::I16(value) => json!(value),
        IdentityValue::I32(value) => json!(value),
        IdentityValue::I64(value) | IdentityValue::Ts(value) => Value::String(value.to_string()),
        IdentityValue::U32(value) => json!(value),
        IdentityValue::U64(value) => Value::String(value.to_string()),
        IdentityValue::F64(value) => finite(*value),
        IdentityValue::Bool(value) => json!(value),
        IdentityValue::Text(bytes) => bytes_value(bytes),
        IdentityValue::Blob {
            stored_bytes,
            full_len,
            truncated,
            full_sha256,
        } => blob_value(stored_bytes, *full_len, *truncated, *full_sha256),
        IdentityValue::ListI32(values) => json!(values),
    }
}

pub(super) fn observation(value: Observation) -> Value {
    json!({
        "count": value.count.to_string(),
        "first": value.first.map(sample),
        "last": value.last.map(sample),
        "nonnegative_delta": value.nonnegative_delta.map(number),
        "observed_us": value.observed_us.to_string(),
    })
}

fn sample(value: Sample) -> Value {
    json!({
        "ts": value.ts.to_string(),
        "value": number(value.value),
    })
}

pub(super) fn number(value: Number) -> Value {
    match value {
        Number::I16(value) => json!(value),
        Number::I32(value) => json!(value),
        Number::I64(value) => Value::String(value.to_string()),
        Number::U32(value) => json!(value),
        Number::U64(value) => Value::String(value.to_string()),
        Number::F64(value) => finite(value),
    }
}

fn finite(value: f64) -> Value {
    serde_json::Number::from_f64(value).map_or_else(
        || {
            let code = if value.is_nan() {
                "nan"
            } else if value.is_sign_positive() {
                "positive_infinity"
            } else {
                "negative_infinity"
            };
            json!({
                "representation": "non_finite",
                "value": code,
            })
        },
        Value::Number,
    )
}

fn bytes_value(bytes: &[u8]) -> Value {
    std::str::from_utf8(bytes).map_or_else(
        |_invalid| {
            json!({
                "representation": "bytes",
                "base64": STANDARD.encode(bytes),
            })
        },
        |text| Value::String(text.to_owned()),
    )
}

fn blob_value(stored: &[u8], full_len: u64, truncated: bool, hash: Option<[u8; 32]>) -> Value {
    let mut value = serde_json::Map::new();
    match std::str::from_utf8(stored) {
        Ok(text) => {
            value.insert("representation".to_owned(), json!("text"));
            value.insert("stored_text".to_owned(), json!(text));
        }
        Err(_invalid) => {
            value.insert("representation".to_owned(), json!("bytes"));
            value.insert("stored_base64".to_owned(), json!(STANDARD.encode(stored)));
        }
    }
    value.insert("full_len".to_owned(), Value::String(full_len.to_string()));
    value.insert("truncated".to_owned(), json!(truncated));
    value.insert(
        "sha256".to_owned(),
        hash.map_or(Value::Null, |bytes| Value::String(hex(&bytes))),
    );
    Value::Object(value)
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len().saturating_mul(2));
    for byte in bytes {
        out.push(char::from(DIGITS[usize::from(byte >> 4)]));
        out.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    out
}

const fn column_type(ty: ColumnType) -> &'static str {
    match ty {
        ColumnType::I8 => "i8",
        ColumnType::I16 => "i16",
        ColumnType::I32 => "i32",
        ColumnType::I64 => "i64",
        ColumnType::U8 => "u8",
        ColumnType::U16 => "u16",
        ColumnType::U32 => "u32",
        ColumnType::U64 => "u64",
        ColumnType::F32 => "f32",
        ColumnType::F64 => "f64",
        ColumnType::Bool => "bool",
        ColumnType::Ts => "timestamp_us",
        ColumnType::StrId => "dictionary_value",
        ColumnType::ListI32 => "list_i32",
    }
}

#[cfg(test)]
mod tests;
