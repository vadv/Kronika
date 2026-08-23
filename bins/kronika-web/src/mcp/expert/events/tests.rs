use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::pg_log::{PgLogErrors, PgLogLifecycle};
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Map, Value, json};

use crate::config::SOURCE_POSTGRESQL;
use crate::mcp::{STRUCTURED_CONTENT_BYTES, State};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const FROM_US: i64 = SEGMENT_ID;
const TO_US: i64 = SEGMENT_ID + 100;

#[test]
fn handler_orders_the_exact_interval_and_continues_without_duplicates() {
    let mut fixture = Fixture::new();
    fixture.append_initial_events();
    let state = fixture.state();

    let first = execute(&state, arguments("asc", 2, None, None)).expect("first Event page");
    assert_eq!(timestamps(&first.data), [FROM_US + 10, FROM_US + 20]);
    assert_eq!(sections(&first.data), ["pg_log_lifecycle", "pg_log_errors"]);
    assert_eq!(first.page["stop_reason"], "page_limit");
    let cursor = first.page["next_cursor"]
        .as_str()
        .expect("Event continuation")
        .to_owned();

    let second =
        execute(&state, arguments("asc", 2, Some(&cursor), None)).expect("second Event page");
    assert_eq!(timestamps(&second.data), [FROM_US + 30]);
    assert_eq!(second.page["stop_reason"], "complete");
    assert!(second.page["next_cursor"].is_null());
}

#[test]
fn continuation_pins_the_original_active_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_initial_events();
    let state = fixture.state();
    let first = execute(&state, arguments("asc", 2, None, None)).expect("first Event page");
    let cursor = first.page["next_cursor"]
        .as_str()
        .expect("Event continuation")
        .to_owned();

    fixture.append_error(FROM_US + 25, 0, 4, b"later append");
    let second = execute(&state, arguments("asc", 10, Some(&cursor), None))
        .expect("continuation over pinned active prefix");
    assert_eq!(timestamps(&second.data), [FROM_US + 30]);
}

#[test]
fn handler_applies_shared_find_and_descending_order() {
    let mut fixture = Fixture::new();
    fixture.append_initial_events();
    let state = fixture.state();

    let payload = execute(&state, arguments("desc", 10, None, Some("kind:critical")))
        .expect("filtered Event page");

    assert_eq!(timestamps(&payload.data), [FROM_US + 30]);
    let events = payload.data["events"].as_array().expect("Event rows");
    assert_eq!(events[0]["tier"], "critical");
    assert!(events[0]["segment_id"].is_string());
    assert!(events[0]["type_id"].is_string());
    assert!(events[0]["row_ordinal"].is_string());

    let text = execute(&state, arguments("desc", 10, None, Some("recorded error")))
        .expect("free-text Event search");
    assert_eq!(timestamps(&text.data), [FROM_US + 30, FROM_US + 20]);
}

#[test]
fn handler_rejects_mismatched_fields_and_typed_order_inputs() {
    let mut fixture = Fixture::new();
    fixture.append_initial_events();
    let state = fixture.state();
    let mut fields = arguments("asc", 10, None, None);
    fields.insert("sources".to_owned(), json!(["pg_log_errors"]));
    fields.insert("fields".to_owned(), json!(["level"]));
    let error = execute(&state, fields).expect_err("field is absent from selected sources");
    assert_eq!(
        (error.code, error.parameter.as_deref()),
        ("no_such_column", Some("fields"))
    );

    let mut order = arguments("asc", 10, None, None);
    order.insert("order".to_owned(), Value::Null);
    let error = execute(&state, order).expect_err("null order is not omission");
    assert_eq!(error.parameter.as_deref(), Some("order"));

    let mut direction = arguments("asc", 10, None, None);
    direction.insert("direction".to_owned(), json!(7));
    let error = execute(&state, direction).expect_err("numeric direction is invalid");
    assert_eq!(error.parameter.as_deref(), Some("direction"));
}

fn execute(
    state: &State,
    args: Map<String, Value>,
) -> Result<super::super::ExpertPayload, super::super::ExpertFailure> {
    super::execute(state, &args, &|| false)
}

fn arguments(
    direction: &str,
    page_size: usize,
    cursor: Option<&str>,
    find: Option<&str>,
) -> Map<String, Value> {
    let mut args = json!({
        "from_us": FROM_US.to_string(),
        "to_us": TO_US.to_string(),
        "direction": direction,
        "order": "timestamp",
        "fields": ["severity", "category", "kind"],
        "page_size": page_size,
        "data_budget_bytes": STRUCTURED_CONTENT_BYTES,
    })
    .as_object()
    .expect("object Event arguments")
    .clone();
    if let Some(cursor) = cursor {
        args.insert("cursor".to_owned(), json!(cursor));
    }
    if let Some(find) = find {
        args.insert("find".to_owned(), json!(find));
    }
    args
}

fn timestamps(data: &Value) -> Vec<i64> {
    data["events"]
        .as_array()
        .expect("Event rows")
        .iter()
        .map(|event| {
            event["timestamp_us"]
                .as_str()
                .expect("lossless Event timestamp")
                .parse()
                .expect("numeric Event timestamp")
        })
        .collect()
}

fn sections(data: &Value) -> Vec<&str> {
    data["events"]
        .as_array()
        .expect("Event rows")
        .iter()
        .map(|event| event["section"].as_str().expect("Event section"))
        .collect()
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
        let directory = tempfile::tempdir().expect("temporary Event data root");
        let root = DataRoot::open(directory.path()).expect("open Event data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire Event writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open Event journal");
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

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn state(&self) -> State {
        State {
            data_root: self.root().to_owned(),
            sources: SOURCE_POSTGRESQL,
            synthetic_demo: false,
            heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    fn append_initial_events(&mut self) {
        let source = self.intern(b"postgresql.csv");
        let pattern = self.intern(b"recorded error");
        let sample = self.intern(b"recorded error sample");
        let lifecycle_message = self.intern(b"server shutdown");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgLogLifecycle {
                ts: Ts(FROM_US + 10),
                system_identifier: Some(7),
                source_file: source,
                kind: 1,
                pid: None,
                signal: None,
                shutdown_mode: None,
                message: lifecycle_message,
                query_detail: None,
            })
            .expect("Lifecycle row fits");
        buffers
            .push(error(FROM_US + 20, 0, 0, source, pattern, sample))
            .expect("notable error fits");
        buffers
            .push(error(FROM_US + 30, 1, 5, source, pattern, sample))
            .expect("critical error fits");
        self.append(buffers);
    }

    fn append_error(&mut self, at: i64, severity: u8, category: u8, text: &[u8]) {
        let source = self.intern(b"postgresql.csv");
        let pattern = self.intern(text);
        let sample = self.intern(text);
        let mut buffers = SectionBuffers::new();
        buffers
            .push(error(at, severity, category, source, pattern, sample))
            .expect("appended error fits");
        self.append(buffers);
    }

    fn intern(&mut self, value: &[u8]) -> StrId {
        StrId(
            self.interner
                .intern(value)
                .expect("intern Event text")
                .get(),
        )
    }

    fn append(&mut self, mut buffers: SectionBuffers) {
        let dictionary = dict::encode(self.interner.window()).expect("encode Event dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode Event fixture")
            .expect("nonempty Event fixture");
        let journal = &mut self.journal;
        let segment_id = self.address.id;
        self.interner
            .flush_window(|_window| journal.append(segment_id, &part).map(|_part| ()))
            .expect("append Event fixture");
    }
}

fn error(
    at: i64,
    severity: u8,
    category: u8,
    source_file: StrId,
    pattern: StrId,
    sample: StrId,
) -> PgLogErrors {
    PgLogErrors {
        ts: Ts(at),
        system_identifier: Some(7),
        source_file,
        severity,
        category,
        sqlstate: None,
        pattern,
        count: 1,
        sample,
        detail: None,
        hint: None,
        context: None,
        statement: None,
        database: None,
        username: None,
    }
}
