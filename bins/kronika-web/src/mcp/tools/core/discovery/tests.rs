use std::path::Path;
use std::sync::Arc;

use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::Ts;
use kronika_registry::os_loadavg::OsLoadavg;
use kronika_writer::{Journal, JournalConfig, SectionBuffers, write_segment};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};

use crate::api;
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};
use crate::mcp::{STRUCTURED_CONTENT_BYTES, State};
use crate::route::{Route, Window};

const SEGMENT_ID: i64 = 1_710_000_000_000_000;

#[test]
fn http_and_mcp_return_the_same_shared_context_payload() {
    let mut fixture = Fixture::new();
    fixture.append_load(SEGMENT_ID + 10);

    let prepared = api::prepare(
        fixture.root(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        Route::Catalog(Window::default()),
        None,
    )
    .expect("HTTP catalog");
    let mut records = Vec::new();
    prepared
        .stream_values(
            &mut |record| {
                records.push(record);
                true
            },
            &|| false,
        )
        .expect("HTTP catalog records");
    let web = &records
        .iter()
        .find(|record| record["record"] == "product_context")
        .expect("HTTP product context")["context"];

    let payload = super::payload(&fixture.state(), &|| false).expect("MCP context");
    assert_eq!(&payload.data, web);
    assert_eq!(payload.page, Value::Null);
    assert!(payload.summary.contains("shared product definitions"));
    assert!(!payload.summary.contains("20"));
}

#[test]
fn context_adapter_preserves_shared_cancellation() {
    let fixture = Fixture::new();
    let error = match super::payload(&fixture.state(), &|| true) {
        Ok(_) => panic!("MCP Context was not cancelled"),
        Err(error) => error,
    };
    assert_eq!(error.code, "cancelled");
    assert!(error.retryable);
}

#[tokio::test]
async fn http_and_real_mcp_dispatch_share_the_atomic_catalog_bound() {
    let mut fixture = Fixture::new();
    fixture.fill_finished_segments(64);

    let prepared = api::prepare(
        fixture.root(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        Route::Catalog(Window::default()),
        None,
    )
    .expect("maximum HTTP catalog");
    let mut records = Vec::new();
    prepared
        .stream_values(
            &mut |record| {
                records.push(record);
                true
            },
            &|| false,
        )
        .expect("maximum HTTP context");
    assert!(
        records
            .iter()
            .any(|record| record["record"] == "product_context")
    );

    let default = crate::mcp::tools::dispatch(
        fixture.state(),
        CallToolRequestParams::new("kronika_get_context"),
        || false,
    )
    .await
    .expect("default MCP Context dispatch");
    let mut explicit_default_request = CallToolRequestParams::new("kronika_get_context");
    explicit_default_request.arguments = Some(serde_json::Map::from_iter([(
        "data_budget_bytes".to_owned(),
        json!(32 * 1_024),
    )]));
    let explicit_default =
        crate::mcp::tools::dispatch(fixture.state(), explicit_default_request, || false)
            .await
            .expect("explicit default MCP Context dispatch");
    assert_eq!(
        default.structured_content, explicit_default.structured_content,
        "the implicit Context budget must remain exactly 32 KiB"
    );

    let mut request = CallToolRequestParams::new("kronika_get_context");
    request.arguments = Some(serde_json::Map::from_iter([(
        "data_budget_bytes".to_owned(),
        json!(STRUCTURED_CONTENT_BYTES),
    )]));
    let dispatched = crate::mcp::tools::dispatch(fixture.state(), request, || false)
        .await
        .expect("maximum MCP Context dispatch");
    let structured = dispatched
        .structured_content
        .as_ref()
        .expect("maximum Context structured content");
    assert_eq!(structured.get("status"), Some(&json!("ok")));
    assert!(
        serde_json::to_vec(structured)
            .expect("maximum Context JSON")
            .len()
            <= STRUCTURED_CONTENT_BYTES
    );

    fixture.fill_finished_segments(1);

    let prepared = api::prepare(
        fixture.root(),
        SOURCE_OS | SOURCE_POSTGRESQL,
        Route::Catalog(Window::default()),
        None,
    )
    .expect("bounded HTTP catalog");
    let mut records = Vec::new();
    let http = prepared
        .stream_values(
            &mut |record| {
                records.push(record);
                true
            },
            &|| false,
        )
        .expect_err("over-bound HTTP context");
    assert_eq!(http.code(), "segment_limit_exceeded");
    assert!(records.is_empty());

    let over_bound = crate::mcp::tools::dispatch(
        fixture.state(),
        CallToolRequestParams::new("kronika_get_context"),
        || false,
    )
    .await
    .expect("over-bound MCP Context dispatch");
    assert_eq!(
        over_bound
            .structured_content
            .as_ref()
            .and_then(|value| value.pointer("/error/code")),
        Some(&json!("segment_limit_exceeded"))
    );
}

struct Fixture {
    directory: tempfile::TempDir,
    writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary context data root");
        let root = DataRoot::open(directory.path()).expect("open context data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire context writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open context journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            writer,
            journal,
            address,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn state(&self) -> State {
        State {
            data_root: self.root().to_owned(),
            sources: SOURCE_OS | SOURCE_POSTGRESQL,
            synthetic_demo: false,
            heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
        }
    }

    fn append_load(&mut self, timestamp: i64) {
        let mut buffers = SectionBuffers::new();
        buffers
            .push(OsLoadavg {
                ts: Ts(timestamp),
                load1: 1.5,
                load5: 1.0,
                load15: 0.5,
                running: 2,
                total: 345,
                scope: 0,
            })
            .expect("Loadavg row fits");
        let part = buffers
            .flush(&[])
            .expect("encode context fixture")
            .expect("nonempty context fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append context fixture");
    }

    fn fill_finished_segments(&mut self, count: usize) {
        for _index in 0..count {
            self.append_load(self.address.id.get() + 10);
            write_segment(&self.journal, &self.writer, self.address)
                .expect("finish bounded context segment");
            self.journal.reset().expect("reset context journal");
            self.address = SegmentAddress::new(
                SegmentId::new(self.address.id.get() + 100).expect("next segment id"),
            )
            .expect("next segment address");
        }
    }
}
