use std::collections::{BTreeMap, BTreeSet};

use serde_json::{Map, Value, json};

use super::super::{
    Anchor, MAX_ROWS, MAX_SEGMENTS, PostgresqlFailure, State, api_failure, failure,
};
use super::SECTION;
use crate::api::{self, ValueLimits, ValueStopReason};
use crate::route::{HourRequest, Route, SeriesRequest, Window};

const MAX_ENTITIES: usize = 256;
const MAX_STREAM_RECORDS: usize = MAX_ROWS + MAX_SEGMENTS * 4 + 2;

pub(super) fn collect_hour(
    state: &State,
    from: i64,
    to: i64,
    anchor: &Anchor,
    cancelled: &impl Fn() -> bool,
) -> Result<api::ValueCollection, PostgresqlFailure> {
    let prepared = api::prepare_for_mcp(
        &state.data_root,
        state.sources,
        state.synthetic_demo,
        Route::Hour(HourRequest {
            window: Window {
                from: Some(from),
                to: Some(to),
            },
            active_segment: anchor
                .active_wal_position
                .map(|position| (anchor.segment_id, position)),
            // An empty projection keeps every physical layout exact. The
            // reducer applies the public projection after episode admission.
            series: Some(SeriesRequest {
                section: SECTION.to_owned(),
                fields: Vec::new(),
                filters: Vec::new(),
                type_id: None,
                group: None,
            }),
        }),
    )
    .map_err(|error| api_failure(&error))?;
    let collected = prepared
        .collect_values(
            ValueLimits {
                records: MAX_STREAM_RECORDS,
                ndjson_bytes: super::super::super::STRUCTURED_CONTENT_BYTES,
            },
            cancelled,
        )
        .map_err(|error| api_failure(&error))?;
    match collected.stop_reason {
        ValueStopReason::Complete => Ok(collected),
        ValueStopReason::Cancelled => Err(failure(
            "cancelled",
            "the Vacuum history read was cancelled",
            None,
        )),
        ValueStopReason::RecordLimit => Err(failure(
            "whole_set_bound_exceeded",
            "the complete Vacuum native sample stream exceeds its retained record bound",
            Some("page_size"),
        )),
        ValueStopReason::ByteLimit => Err(failure(
            "result_bound_exceeded",
            "the complete Vacuum native sample stream exceeds its retained byte bound",
            Some("fields"),
        )),
    }
}

pub(super) struct DecodedHour {
    pub(super) rows: Vec<Sample>,
    pub(super) layouts: Vec<Value>,
    pub(super) warnings: Vec<Value>,
}

pub(super) fn decode_hour(records: Vec<Value>) -> Result<DecodedHour, PostgresqlFailure> {
    let mut segment_id = None;
    let mut layouts = BTreeMap::<u32, Value>::new();
    let mut rows = Vec::new();
    let mut warnings = Vec::new();
    for record in records {
        match record.get("record").and_then(Value::as_str) {
            Some("series_segment") => {
                segment_id = record
                    .pointer("/segment/id")
                    .and_then(decimal_i64)
                    .filter(|value| *value >= 0);
                if segment_id.is_none() {
                    return Err(malformed("a Vacuum series segment has no valid id"));
                }
            }
            Some("layout") => {
                let layout = record
                    .get("layout")
                    .and_then(Value::as_object)
                    .ok_or_else(|| malformed("a Vacuum layout is not an object"))?;
                if layout.get("logical_name").and_then(Value::as_str) != Some(SECTION) {
                    continue;
                }
                let type_id = layout
                    .get("type_id")
                    .and_then(decimal_u32)
                    .ok_or_else(|| malformed("a Vacuum layout has no valid type_id"))?;
                layouts.insert(type_id, Value::Object(layout.clone()));
            }
            Some("row") => {
                let current_segment =
                    segment_id.ok_or_else(|| malformed("a Vacuum row has no physical segment"))?;
                rows.push(decode_row(&record, current_segment, &layouts)?);
            }
            Some("warning") => warnings.push(record),
            _ => {}
        }
    }
    Ok(DecodedHour {
        rows,
        layouts: layouts.into_values().collect(),
        warnings,
    })
}

#[derive(Clone)]
pub(super) struct Sample {
    pub(super) key: EpisodeKey,
    pub(super) timestamp: i64,
    pub(super) segment_id: i64,
    pub(super) type_id: u32,
    pub(super) ordinal: u64,
    pub(super) row: Map<String, Value>,
}

impl Sample {
    pub(super) fn value(&self, field: &str) -> Option<&Value> {
        self.row
            .get("values")
            .and_then(Value::as_object)
            .and_then(|values| values.get(field))
    }

    pub(super) fn phase(&self) -> Result<&str, PostgresqlFailure> {
        self.value("phase")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a Vacuum row has no recorded phase"))
    }

    pub(super) fn integer(&self, field: &str) -> Result<Option<i128>, PostgresqlFailure> {
        let Some(value) = self.value(field) else {
            return Ok(None);
        };
        if value.is_null() {
            return Ok(None);
        }
        decimal_i128(value)
            .map(Some)
            .ok_or_else(|| malformed("a Vacuum progress counter is not an exact integer"))
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct EpisodeKey {
    pub(super) type_id: u32,
    pub(super) pid: i32,
    pub(super) datid: u32,
    pub(super) relid: u32,
}

fn decode_row(
    record: &Value,
    segment_id: i64,
    layouts: &BTreeMap<u32, Value>,
) -> Result<Sample, PostgresqlFailure> {
    let mut row = record
        .as_object()
        .cloned()
        .ok_or_else(|| malformed("a Vacuum row is not an object"))?;
    let type_id = row
        .get("type_id")
        .and_then(decimal_u32)
        .ok_or_else(|| malformed("a Vacuum row has no valid type_id"))?;
    let layout = layouts
        .get(&type_id)
        .and_then(Value::as_object)
        .ok_or_else(|| malformed("a Vacuum row has no matching layout"))?;
    let columns = layout
        .get("columns")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("a Vacuum layout has no columns"))?;
    let values = row
        .get("values")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("a Vacuum row has no value array"))?;
    if columns.len() != values.len() {
        return Err(malformed("a Vacuum row does not match its layout"));
    }
    let mut named = Map::new();
    for (column, value) in columns.iter().zip(values) {
        let name = column
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("a Vacuum layout column has no name"))?;
        named.insert(name.to_owned(), value.clone());
    }
    let timestamp = row
        .get("timestamp")
        .and_then(decimal_i64)
        .ok_or_else(|| malformed("a Vacuum row has no exact timestamp"))?;
    let ordinal = row
        .get("ordinal")
        .and_then(decimal_u64)
        .ok_or_else(|| malformed("a Vacuum row has no physical ordinal"))?;
    let pid = named
        .get("pid")
        .and_then(decimal_i32)
        .filter(|pid| *pid > 0)
        .ok_or_else(|| malformed("a Vacuum row has no valid pid"))?;
    let datid = named
        .get("datid")
        .and_then(decimal_u32)
        .ok_or_else(|| malformed("a Vacuum row has no valid datid"))?;
    let relid = named
        .get("relid")
        .and_then(decimal_u32)
        .ok_or_else(|| malformed("a Vacuum row has no valid relid"))?;
    row.insert("logical_name".to_owned(), json!(SECTION));
    row.insert("segment_id".to_owned(), json!(segment_id.to_string()));
    row.insert("values".to_owned(), Value::Object(named));
    Ok(Sample {
        key: EpisodeKey {
            type_id,
            pid,
            datid,
            relid,
        },
        timestamp,
        segment_id,
        type_id,
        ordinal,
        row,
    })
}

pub(super) fn admit_samples(rows: &[Sample]) -> Result<(), PostgresqlFailure> {
    if rows.len() > MAX_ROWS {
        return Err(failure(
            "sample_bound_exceeded",
            "the complete Vacuum interval exceeds 500 native samples",
            Some("from_us"),
        ));
    }
    let entities = rows
        .iter()
        .map(|sample| &sample.key)
        .collect::<BTreeSet<_>>();
    if entities.len() > MAX_ENTITIES {
        return Err(failure(
            "entity_bound_exceeded",
            "the complete Vacuum interval exceeds 256 physical identities",
            Some("from_us"),
        ));
    }
    let segments = rows
        .iter()
        .map(|sample| sample.segment_id)
        .collect::<BTreeSet<_>>();
    if segments.len() > MAX_SEGMENTS {
        return Err(failure(
            "segment_bound_exceeded",
            "the Vacuum interval exceeds 64 recorded segments",
            Some("from_us"),
        ));
    }
    let mut locators = BTreeSet::new();
    for sample in rows {
        if !locators.insert((sample.segment_id, sample.type_id, sample.ordinal)) {
            return Err(malformed(
                "the Vacuum interval repeats a physical row locator",
            ));
        }
    }
    Ok(())
}

pub(super) fn decimal_i128(value: &Value) -> Option<i128> {
    value
        .as_i64()
        .map(i128::from)
        .or_else(|| value.as_u64().map(i128::from))
        .or_else(|| value.as_str()?.parse().ok())
}

pub(super) fn decimal_i64(value: &Value) -> Option<i64> {
    value.as_i64().or_else(|| value.as_str()?.parse().ok())
}

fn decimal_i32(value: &Value) -> Option<i32> {
    decimal_i64(value).and_then(|value| i32::try_from(value).ok())
}

pub(super) fn decimal_u64(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

pub(super) fn decimal_u32(value: &Value) -> Option<u32> {
    decimal_u64(value).and_then(|value| u32::try_from(value).ok())
}

pub(super) fn malformed(message: impl Into<String>) -> PostgresqlFailure {
    failure("malformed_vacuum_history", message, None)
}
