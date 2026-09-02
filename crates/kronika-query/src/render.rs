//! Exact NDJSON framing shared by query outputs.

use serde_json::Value;

use crate::QueryError;

pub(crate) fn record(value: impl std::borrow::Borrow<Value>) -> Result<Vec<u8>, QueryError> {
    let mut bytes = serde_json::to_vec(value.borrow())?;
    bytes.push(b'\n');
    Ok(bytes)
}
