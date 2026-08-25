use kronika_reader::Listing;
use serde_json::{Value, json};

use super::catalog::PreparedCatalog;
use super::{Prepared, ValueCollector, ValueLimits, ValueStopReason};
use crate::route::Window;

fn empty_catalog() -> Prepared {
    Prepared::CatalogInventory(PreparedCatalog::from_listing(
        Listing {
            segments: Vec::new(),
            warnings: Vec::new(),
        },
        Window::default(),
        0,
        false,
    ))
}

#[test]
fn typed_records_are_the_exact_http_ndjson_records() {
    let mut http = Vec::new();
    empty_catalog()
        .stream(
            &mut |bytes| {
                http.push(bytes);
                true
            },
            &|| false,
        )
        .expect("HTTP stream");

    let mut typed = Vec::<Value>::new();
    empty_catalog()
        .stream_values(
            &mut |value| {
                typed.push(value);
                true
            },
            &|| false,
        )
        .expect("typed stream");
    let rendered = typed
        .iter()
        .map(super::render::record)
        .collect::<Result<Vec<_>, _>>()
        .expect("render typed records");

    assert_eq!(rendered, http);
}

#[test]
fn value_collector_stops_before_record_and_byte_limits() {
    let first = json!({"record": "first"});
    let first_bytes = super::render::record(&first)
        .expect("render first record")
        .len();
    let mut records = ValueCollector::new(ValueLimits {
        records: 1,
        ndjson_bytes: usize::MAX,
    });
    assert!(records.push(first.clone()));
    assert!(!records.push(json!({"record": "second"})));
    let records = records.finish(false).expect("record-limited values");
    assert_eq!(records.records.as_slice(), std::slice::from_ref(&first));
    assert_eq!(records.stop_reason, ValueStopReason::RecordLimit);
    assert_eq!(records.stop_reason.code(), "record_limit");

    let mut bytes = ValueCollector::new(ValueLimits {
        records: usize::MAX,
        ndjson_bytes: first_bytes - 1,
    });
    assert!(!bytes.push(first));
    let bytes = bytes.finish(false).expect("byte-limited values");
    assert!(bytes.records.is_empty());
    assert_eq!(bytes.ndjson_bytes, 0);
    assert_eq!(bytes.stop_reason, ValueStopReason::ByteLimit);
}

#[test]
fn collected_values_report_completion_and_cancellation() {
    let complete = empty_catalog()
        .collect_values(
            ValueLimits {
                records: 10,
                ndjson_bytes: 4_096,
            },
            &|| false,
        )
        .expect("complete values");
    assert_eq!(complete.records.len(), 1);
    assert_eq!(complete.stop_reason, ValueStopReason::Complete);
    assert_eq!(
        complete.ndjson_bytes,
        super::render::record(&complete.records[0]).unwrap().len()
    );

    let cancelled = empty_catalog()
        .collect_values(
            ValueLimits {
                records: 10,
                ndjson_bytes: 4_096,
            },
            &|| true,
        )
        .expect("cancelled values");
    assert!(cancelled.records.is_empty());
    assert_eq!(cancelled.ndjson_bytes, 0);
    assert_eq!(cancelled.stop_reason, ValueStopReason::Cancelled);
}
