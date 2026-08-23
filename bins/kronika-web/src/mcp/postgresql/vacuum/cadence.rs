use std::collections::BTreeMap;

use serde_json::{Map, Value, json};

use super::super::{PostgresqlFailure, State, collect, resolve_anchor};
use super::reader::{decimal_u32, decimal_u64, malformed};
use crate::route::{Order, Route, SnapshotRequest};

pub(super) struct Cadence {
    pub(super) seconds: Option<u64>,
    pub(super) provenance: Option<Value>,
    pub(super) warnings: Vec<Value>,
}

pub(super) fn recorded_cadence(
    state: &State,
    at: i64,
    cancelled: &impl Fn() -> bool,
) -> Result<Cadence, PostgresqlFailure> {
    let anchor = match resolve_anchor(state, at, &["instance_metadata"], cancelled) {
        Ok(anchor) => anchor,
        Err(error) if error.code == "no_recorded_data" => return Ok(missing_cadence()),
        Err(error) => return Err(error),
    };
    let request = SnapshotRequest {
        segment_id: anchor.segment_id,
        at,
        sections: vec!["instance_metadata".to_owned()],
        fields: vec!["postgresql_interval_seconds".to_owned()],
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        page_size: Some(1),
        cursor: None,
        search: None,
        first_match: false,
        text: None,
        filters: Vec::new(),
        type_id: None,
        row_ordinal: None,
    };
    let collected = match collect(state, Route::Snapshot(Box::new(request)), cancelled) {
        Ok(collected) => collected,
        Err(error) if matches!(error.code, "no_such_section" | "no_such_column") => {
            return Ok(missing_cadence());
        }
        Err(error) => return Err(error),
    };
    let mut warnings = anchor.warnings;
    warnings.extend(
        collected
            .records
            .iter()
            .filter(|record| record.get("record").and_then(Value::as_str) == Some("warning"))
            .cloned(),
    );
    let Some(row) = named_snapshot_row(&collected.records, "instance_metadata")? else {
        warnings.push(cadence_warning());
        return Ok(Cadence {
            seconds: None,
            provenance: None,
            warnings,
        });
    };
    let seconds = row
        .pointer("/values/postgresql_interval_seconds")
        .and_then(decimal_u64);
    let provenance = seconds.map(|seconds| {
        json!({
            "origin": "recorded",
            "source": "instance_metadata",
            "field": "postgresql_interval_seconds",
            "value": seconds.to_string(),
            "row": row,
        })
    });
    if seconds.is_none() {
        warnings.push(cadence_warning());
    }
    Ok(Cadence {
        seconds,
        provenance,
        warnings,
    })
}

fn missing_cadence() -> Cadence {
    Cadence {
        seconds: None,
        provenance: None,
        warnings: vec![cadence_warning()],
    }
}

fn cadence_warning() -> Value {
    json!({
        "code": "cadence_not_recorded",
        "message": "Vacuum episode adjacency has no recorded PostgreSQL cadence; no time-gap condition was applied.",
    })
}

fn named_snapshot_row(
    records: &[Value],
    logical_name: &str,
) -> Result<Option<Value>, PostgresqlFailure> {
    let mut layouts = BTreeMap::<u32, &Map<String, Value>>::new();
    for record in records {
        if record.get("record").and_then(Value::as_str) != Some("layout") {
            continue;
        }
        let Some(layout) = record.get("layout").and_then(Value::as_object) else {
            return Err(malformed("a metadata layout is not an object"));
        };
        if layout.get("logical_name").and_then(Value::as_str) != Some(logical_name) {
            continue;
        }
        let type_id = layout
            .get("type_id")
            .and_then(decimal_u32)
            .ok_or_else(|| malformed("a metadata layout has no valid type_id"))?;
        layouts.insert(type_id, layout);
    }
    let Some(record) = records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("row"))
    else {
        return Ok(None);
    };
    let type_id = record
        .get("type_id")
        .and_then(decimal_u32)
        .ok_or_else(|| malformed("a metadata row has no valid type_id"))?;
    let layout = layouts
        .get(&type_id)
        .ok_or_else(|| malformed("a metadata row has no matching layout"))?;
    let columns = layout
        .get("columns")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("a metadata layout has no columns"))?;
    let values = record
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("a metadata row has no values"))?;
    if columns.len() != values.len() {
        return Err(malformed("a metadata row does not match its layout"));
    }
    let mut named = Map::new();
    for (column, value) in columns.iter().zip(values) {
        let name = column
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a metadata column has no name"))?;
        named.insert(name.to_owned(), value.clone());
    }
    let mut row = record
        .as_object()
        .cloned()
        .ok_or_else(|| malformed("a metadata row is not an object"))?;
    row.insert("logical_name".to_owned(), json!(logical_name));
    row.insert("values".to_owned(), Value::Object(named));
    Ok(Some(Value::Object(row)))
}
