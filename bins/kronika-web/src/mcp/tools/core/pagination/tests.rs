use std::cell::Cell;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::pg_log::PgLogLifecycle;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Map, Value, json};

use crate::config::SOURCE_POSTGRESQL;
use crate::mcp::{STRUCTURED_CONTENT_BYTES, State};

const SEGMENT_ID: i64 = 1_710_000_000_000_000;
const HOUR: i64 = super::super::HOUR_US;
const WINDOW_FROM: i64 = SEGMENT_ID;
const WINDOW_TO: i64 = SEGMENT_ID + 100;

#[test]
fn cursor_round_trip_rejects_tampering_and_wrong_surface() {
    let cursor = super::Cursor {
        surface: super::Surface::Findings,
        query: super::Fingerprint([1; super::DIGEST_BYTES]),
        source: super::Fingerprint([2; super::DIGEST_BYTES]),
        position: super::PositionKey(super::Fingerprint([3; super::DIGEST_BYTES])),
        offset: 7,
    };
    let encoded = cursor.encode();

    assert_eq!(
        super::Cursor::parse(&encoded).expect("valid cursor"),
        cursor
    );
    let mut tampered = encoded.into_bytes();
    let index = tampered.len() / 2;
    tampered[index] = if tampered[index] == b'A' { b'B' } else { b'A' };
    assert!(super::Cursor::parse(std::str::from_utf8(&tampered).expect("ASCII cursor")).is_err());
    assert!(
        super::page_start(
            Some(&cursor.encode()),
            super::Surface::Timeline,
            cursor.query,
            cursor.source,
        )
        .is_err()
    );
}

#[test]
fn hours_pages_ranges_without_repeating_the_boundary_hour() {
    let mut fixture = Fixture::new();
    fixture.append(&[WINDOW_FROM + 10, WINDOW_FROM + HOUR + 10]);
    let state = fixture.state();
    let first = execute(
        &state,
        "kronika_list_hours",
        arguments(WINDOW_FROM, WINDOW_FROM + 2 * HOUR, 1, None),
    )
    .expect("first hour page");
    let cursor = next_cursor(&first.page);
    assert_eq!(
        hour_starts(&first.data),
        [WINDOW_FROM.div_euclid(HOUR) * HOUR]
    );

    let second = execute(
        &state,
        "kronika_list_hours",
        arguments(WINDOW_FROM, WINDOW_FROM + 2 * HOUR, 10, Some(&cursor)),
    )
    .expect("second hour page with a different limit");
    assert_eq!(
        hour_starts(&second.data),
        [(WINDOW_FROM + HOUR).div_euclid(HOUR) * HOUR]
    );
    assert_eq!(second.page["stop_reason"], "complete");
}

#[test]
fn findings_and_timeline_return_query_bound_physical_pages() {
    let mut fixture = Fixture::new();
    fixture.append(&[WINDOW_FROM + 10, WINDOW_FROM + 20, WINDOW_FROM + 30]);
    let state = fixture.state();

    let first = execute(
        &state,
        "kronika_list_findings",
        arguments(WINDOW_FROM, WINDOW_TO, 1, None),
    )
    .expect("first Findings page");
    let first_findings = first.data["findings"].as_array().expect("Findings rows");
    assert_eq!(first_findings.len(), 1);
    assert_exact_provenance(&first_findings[0]);
    let cursor = next_cursor(&first.page);

    let second = execute(
        &state,
        "kronika_list_findings",
        arguments(WINDOW_FROM, WINDOW_TO, 2, Some(&cursor)),
    )
    .expect("second Findings page");
    let second_findings = second.data["findings"].as_array().expect("Findings rows");
    assert_eq!(second_findings.len(), 2);
    assert_ne!(position(&first_findings[0]), position(&second_findings[0]));
    assert!(second.page["next_cursor"].is_null());

    let timeline = execute(
        &state,
        "kronika_get_timeline",
        arguments(WINDOW_FROM, WINDOW_TO, 2, None),
    )
    .expect("first Timeline page");
    assert!(timeline.data["lanes"].as_array().is_some_and(Vec::is_empty));
    assert_eq!(timeline.data["markers"].as_array().map(Vec::len), Some(2));
    assert!(timeline.page["next_cursor"].is_string());
}

#[test]
fn active_prefix_change_returns_the_retryable_source_error() {
    let mut fixture = Fixture::new();
    fixture.append(&[WINDOW_FROM + 10, WINDOW_FROM + 20]);
    let state = fixture.state();
    let first = execute(
        &state,
        "kronika_list_findings",
        arguments(WINDOW_FROM, WINDOW_TO, 1, None),
    )
    .expect("first Findings page");
    let cursor = next_cursor(&first.page);

    fixture.append(&[WINDOW_FROM + 30]);
    let error = match execute(
        &state,
        "kronika_list_findings",
        arguments(WINDOW_FROM, WINDOW_TO, 1, Some(&cursor)),
    ) {
        Ok(_payload) => panic!("changed active prefix retained the old page source"),
        Err(error) => error,
    };

    assert_eq!((error.code, error.retryable), ("source_changed", true));
}

#[test]
fn findings_and_timeline_fit_the_exact_envelope_and_continue_across_budget_changes() {
    let mut fixture = Fixture::new();
    fixture.append(&[WINDOW_FROM + 10, WINDOW_FROM + 20, WINDOW_FROM + 30]);
    let state = fixture.state();

    for name in ["kronika_list_findings", "kronika_get_timeline"] {
        let one = execute(&state, name, arguments(WINDOW_FROM, WINDOW_TO, 1, None))
            .expect("single-item page");
        let budget = payload_bytes(&one);
        let first = execute_with_budget(
            &state,
            name,
            arguments(WINDOW_FROM, WINDOW_TO, 3, None),
            budget,
        )
        .expect("byte-bounded first page");
        assert!(payload_bytes(&first) <= budget);
        assert_eq!(first.page["returned"], 1);
        let cursor = next_cursor(&first.page);

        let rest = execute_with_budget(
            &state,
            name,
            arguments(WINDOW_FROM, WINDOW_TO, 10, Some(&cursor)),
            STRUCTURED_CONTENT_BYTES,
        )
        .expect("continuation with changed row and byte limits");
        assert_eq!(rest.page["returned"], 2);
        assert!(rest.page["next_cursor"].is_null());
    }
}

#[test]
fn fixed_metadata_and_first_item_have_stable_budget_errors() {
    let query = super::Fingerprint([1; super::DIGEST_BYTES]);
    let source = super::Fingerprint([2; super::DIGEST_BYTES]);
    let window = crate::route::Window {
        from: Some(WINDOW_FROM),
        to: Some(WINDOW_TO),
    };
    let metadata_error = super::fit_findings_page(
        window,
        super::Surface::Findings,
        query,
        source,
        0,
        super::Accumulated {
            items: Vec::new(),
            has_more: false,
        },
        &[json!({"large_metadata": "m".repeat(4_096)})],
        &[],
        false,
        1_024,
    )
    .expect_err("oversized fixed metadata");
    assert_eq!(metadata_error.code, "result_too_large");
    assert_eq!(
        metadata_error.parameter.as_deref(),
        Some("data_budget_bytes")
    );
    assert!(metadata_error.message.contains("metadata"));

    let first_item_error = super::fit_findings_page(
        window,
        super::Surface::Findings,
        query,
        source,
        0,
        super::Accumulated {
            items: vec![super::Positioned {
                item: json!({"large_item": "i".repeat(4_096)}),
                position: super::PositionKey(super::Fingerprint([3; super::DIGEST_BYTES])),
            }],
            has_more: false,
        },
        &[],
        &[],
        false,
        1_024,
    )
    .expect_err("oversized first item");
    assert_eq!(first_item_error.code, "result_too_large");
    assert_eq!(
        first_item_error.parameter.as_deref(),
        Some("data_budget_bytes")
    );
    assert!(first_item_error.message.contains("first selected Finding"));
}

#[test]
fn timeline_byte_fitting_keeps_the_interleaved_scan_prefix() {
    let query = super::Fingerprint([4; super::DIGEST_BYTES]);
    let source = super::Fingerprint([5; super::DIGEST_BYTES]);
    let window = crate::route::Window {
        from: Some(WINDOW_FROM),
        to: Some(WINDOW_TO),
    };
    let accumulated = synthetic_timeline();
    let page = super::candidate_page_info(
        super::Surface::Timeline,
        query,
        source,
        0,
        &accumulated,
        2,
        false,
    )
    .expect("two-item continuation page");
    let budget = super::envelope_len(
        &super::super::anchor(None, window.from, None),
        &super::timeline_data(&accumulated.items[..2], &[]),
        &page,
        &[],
    );
    let (items, fitted_page) = super::fit_timeline_page(
        window,
        super::Surface::Timeline,
        query,
        source,
        0,
        accumulated,
        &[],
        &[],
        false,
        budget,
    )
    .expect("interleaved bounded Timeline page");

    assert_eq!(fitted_page.returned, 2);
    assert!(fitted_page.next_cursor.is_some());
    assert!(matches!(&items[0], super::TimelineItem::Lane(_)));
    assert!(matches!(&items[1], super::TimelineItem::Marker(_)));
}

#[test]
fn hours_checks_cancellation_during_catalog_and_page_work() {
    let mut fixture = Fixture::new();
    fixture.append(&[WINDOW_FROM + 10, WINDOW_FROM + HOUR + 10]);
    let checks = Cell::new(0_usize);
    let cancelled = || {
        let next = checks.get().saturating_add(1);
        checks.set(next);
        next >= 5
    };
    let error = super::hours(
        fixture.directory.path(),
        crate::route::Window {
            from: Some(WINDOW_FROM),
            to: Some(WINDOW_FROM + 2 * HOUR),
        },
        None,
        10,
        &cancelled,
    )
    .expect_err("cancelled hour scan");

    assert_eq!(error.code, "cancelled");
    assert!(checks.get() >= 5);
}

fn execute(
    state: &State,
    name: &str,
    args: Map<String, Value>,
) -> Result<super::super::Payload, super::super::Failure> {
    execute_with_budget(state, name, args, STRUCTURED_CONTENT_BYTES)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "test argument maps are built inline and consumed by each execution"
)]
fn execute_with_budget(
    state: &State,
    name: &str,
    args: Map<String, Value>,
    budget: usize,
) -> Result<super::super::Payload, super::super::Failure> {
    super::super::execute(state, name, &args, budget, &|| false)
}

fn arguments(from: i64, to: i64, limit: usize, cursor: Option<&str>) -> Map<String, Value> {
    let mut args = json!({
        "from_us": from.to_string(),
        "to_us": to.to_string(),
        "limit": limit,
        "data_budget_bytes": STRUCTURED_CONTENT_BYTES,
    })
    .as_object()
    .expect("object pagination arguments")
    .clone();
    if let Some(cursor) = cursor {
        args.insert("cursor".to_owned(), json!(cursor));
    }
    args
}

fn next_cursor(page: &Value) -> String {
    page["next_cursor"]
        .as_str()
        .expect("continuation cursor")
        .to_owned()
}

fn payload_bytes(payload: &super::super::Payload) -> usize {
    super::super::super::structured_envelope_len(
        &payload.anchor,
        &payload.data,
        &payload.page,
        &payload.warnings,
    )
}

fn synthetic_timeline() -> super::Accumulated<super::TimelineItem> {
    super::Accumulated {
        items: vec![
            super::Positioned {
                item: super::TimelineItem::Lane(json!({"sequence": 1})),
                position: super::PositionKey(super::Fingerprint([6; super::DIGEST_BYTES])),
            },
            super::Positioned {
                item: super::TimelineItem::Marker(json!({"sequence": 2})),
                position: super::PositionKey(super::Fingerprint([7; super::DIGEST_BYTES])),
            },
            super::Positioned {
                item: super::TimelineItem::Lane(json!({"sequence": 3, "large": "x".repeat(4_096)})),
                position: super::PositionKey(super::Fingerprint([8; super::DIGEST_BYTES])),
            },
        ],
        has_more: false,
    }
}

fn hour_starts(data: &Value) -> Vec<i64> {
    data["hours"]
        .as_array()
        .expect("recorded hours")
        .iter()
        .map(|hour| {
            hour["start_us"]
                .as_str()
                .expect("lossless hour start")
                .parse()
                .expect("numeric hour start")
        })
        .collect()
}

fn assert_exact_provenance(finding: &Value) {
    for name in [
        "segment_id",
        "active_wal_position",
        "type_id",
        "row_ordinal",
        "ts",
    ] {
        assert!(finding.get(name).is_some_and(Value::is_string), "{name}");
    }
    assert_eq!(finding["source"], "kronika_index");
}

fn position(finding: &Value) -> (Value, Value, Value, Value) {
    (
        finding["segment_id"].clone(),
        finding["type_id"].clone(),
        finding["row_ordinal"].clone(),
        finding["kind"].clone(),
    )
}

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
    interner: Interner,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary pagination data root");
        let root = DataRoot::open(directory.path()).expect("open pagination data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire pagination writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open pagination journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            _writer: writer,
            journal,
            address,
            interner: Interner::new(DictLimits::default()),
        }
    }

    fn state(&self) -> State {
        State {
            data_root: self.directory.path().to_owned(),
            sources: SOURCE_POSTGRESQL,
            synthetic_demo: false,
            heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    fn append(&mut self, timestamps: &[i64]) {
        let source = self.intern(b"postgresql.csv");
        let message = self.intern(b"recorded lifecycle event");
        let mut buffers = SectionBuffers::new();
        for (index, &timestamp) in timestamps.iter().enumerate() {
            buffers
                .push(PgLogLifecycle {
                    ts: Ts(timestamp),
                    system_identifier: Some(7),
                    source_file: source,
                    kind: u8::try_from(index % 3).expect("Lifecycle kind"),
                    pid: None,
                    signal: None,
                    shutdown_mode: None,
                    message,
                    query_detail: None,
                })
                .expect("Lifecycle row fits");
        }
        let dictionary = dict::encode(self.interner.window()).expect("encode dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode pagination fixture")
            .expect("nonempty pagination fixture");
        let journal = &mut self.journal;
        let segment_id = self.address.id;
        self.interner
            .flush_window(|_window| journal.append(segment_id, &part).map(|_part| ()))
            .expect("append pagination fixture");
    }

    fn intern(&mut self, value: &[u8]) -> StrId {
        StrId(
            self.interner
                .intern(value)
                .expect("intern pagination text")
                .get(),
        )
    }
}
