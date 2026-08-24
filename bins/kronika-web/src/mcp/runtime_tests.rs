use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::os_process::OsProcess;
use kronika_registry::pg_log::PgLogLifecycle;
use kronika_registry::{Section as _, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::{STRUCTURED_CONTENT_BYTES, State, tools};
use crate::config::{SOURCE_OS, SOURCE_POSTGRESQL};

const SEGMENT_ID: i64 = 1_710_000_000_000_000;
const METADATA_AT: i64 = SEGMENT_ID + 10;
const FIRST_PROCESS_AT: i64 = SEGMENT_ID + 20;
const EVENT_AT: i64 = SEGMENT_ID + 25;
const LAST_PROCESS_AT: i64 = SEGMENT_ID + 30;

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    _journal: Journal,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary MCP runtime data root");
        let root = DataRoot::open(directory.path()).expect("open MCP runtime data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire MCP runtime writer");
        let mut journal =
            Journal::open(&writer, JournalConfig::default()).expect("open MCP runtime journal");
        append_fixture(&mut journal);
        Self {
            directory,
            _writer: writer,
            _journal: journal,
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
            heavy_scans: Arc::new(Semaphore::new(2)),
        }
    }
}

#[tokio::test]
async fn all_ten_core_and_expert_tools_execute_through_dispatch_on_recorded_data() {
    let fixture = Fixture::new();
    for (name, arguments, data_pointer) in runtime_calls() {
        let response = dispatch(&fixture, name, arguments).await;
        assert_eq!(
            response.get("status"),
            Some(&json!("ok")),
            "{name}: {response}"
        );
        let data = response.pointer(data_pointer).unwrap_or_else(|| {
            panic!("{name} returned no advertised data at {data_pointer}: {response}")
        });
        assert!(
            nonempty(data),
            "{name} returned no recorded data: {response}"
        );
        assert!(
            response
                .pointer("/page/returned")
                .and_then(Value::as_u64)
                .is_some_and(|returned| returned > 0),
            "{name} did not report a recorded result: {response}"
        );
        if name == "kronika_get_context" {
            assert_eq!(
                response.pointer("/data/context/recorded/segments/0/segment_id"),
                Some(&json!(SEGMENT_ID.to_string())),
                "Context did not discover the recorded fixture: {response}"
            );
        }
    }
}

#[tokio::test]
async fn metric_history_shares_the_envelope_budget_across_identities() {
    let fixture = Fixture::new();
    let one = dispatch(
        &fixture,
        "kronika_get_metric_history",
        json!({
            "from_us": FIRST_PROCESS_AT.to_string(),
            "to_us": LAST_PROCESS_AT.to_string(),
            "section": "os_process",
            "identities": [{"pid": 42}],
            "fields": ["utime"],
            "sample_limit": 10,
        }),
    )
    .await;
    assert_eq!(one["status"], "ok", "single history: {one}");

    let arguments = json!({
        "from_us": FIRST_PROCESS_AT.to_string(),
        "to_us": LAST_PROCESS_AT.to_string(),
        "section": "os_process",
        "identities": [{"pid": 42}, {"pid": 42}],
        "fields": ["utime"],
        "sample_limit": 10,
    });
    let complete = dispatch(&fixture, "kronika_get_metric_history", arguments.clone()).await;
    assert_eq!(complete["status"], "ok", "complete history: {complete}");
    let complete_bytes = serde_json::to_vec(&complete)
        .expect("complete history envelope")
        .len();
    assert!(complete_bytes > 1_024);

    let bounded = dispatch_with_budget(
        &fixture,
        "kronika_get_metric_history",
        arguments,
        complete_bytes - 1,
    )
    .await;
    assert_eq!(bounded["status"], "ok", "bounded history: {bounded}");
    assert!(
        serde_json::to_vec(&bounded)
            .expect("bounded history envelope")
            .len()
            <= complete_bytes - 1
    );
    assert!(
        bounded["page"]["returned"]
            .as_u64()
            .is_some_and(|returned| (1..4).contains(&returned)),
        "bounded history did not retain a sample prefix: {bounded}"
    );
    assert_eq!(bounded["page"]["stop_reason"], "byte_limit");
}

fn runtime_calls() -> [(&'static str, Value, &'static str); 10] {
    [
        (
            "kronika_get_context",
            json!({}),
            "/data/context/recorded/segments",
        ),
        (
            "kronika_list_hours",
            json!({
                "from_us": SEGMENT_ID.to_string(),
                "to_us": LAST_PROCESS_AT.to_string(),
                "limit": 10,
            }),
            "/data/hours",
        ),
        (
            "kronika_rank_heatmap",
            json!({
                "from_us": FIRST_PROCESS_AT.to_string(),
                "to_us": LAST_PROCESS_AT.to_string(),
                "surface": "processes",
                "cut": "cpu",
                "columns": 1,
                "top": 10,
            }),
            "/data/rows",
        ),
        (
            "kronika_list_findings",
            json!({
                "from_us": SEGMENT_ID.to_string(),
                "to_us": LAST_PROCESS_AT.to_string(),
                "limit": 10,
            }),
            "/data/findings",
        ),
        (
            "kronika_get_timeline",
            json!({
                "from_us": SEGMENT_ID.to_string(),
                "to_us": LAST_PROCESS_AT.to_string(),
                "limit": 10,
            }),
            "/data/markers",
        ),
        (
            "kronika_get_host_context",
            json!({
                "at_us": LAST_PROCESS_AT.to_string(),
                "lens": "identity",
                "fields": ["clock_ticks_per_sec"],
                "page_size": 10,
            }),
            "/data/rows",
        ),
        (
            "kronika_find_processes",
            json!({
                "at_us": LAST_PROCESS_AT.to_string(),
                "lens": "identity",
                "fields": ["pid", "ppid", "utime"],
                "page_size": 10,
            }),
            "/data/processes",
        ),
        (
            "kronika_get_metric_history",
            json!({
                "from_us": FIRST_PROCESS_AT.to_string(),
                "to_us": LAST_PROCESS_AT.to_string(),
                "section": "os_process",
                "identities": [{"pid": 42}],
                "fields": ["utime"],
                "sample_limit": 10,
            }),
            "/data/series",
        ),
        (
            "kronika_get_snapshot",
            json!({
                "section": "os_process",
                "at_us": LAST_PROCESS_AT.to_string(),
                "fields": ["pid", "ppid", "utime"],
                "page_size": 10,
            }),
            "/data/rows",
        ),
        (
            "kronika_get_row_detail",
            json!({
                "segment_id": SEGMENT_ID.to_string(),
                "type_id": OsProcess::CONTRACT.type_id.get(),
                "row_ordinal": "0",
                "timestamp_us": FIRST_PROCESS_AT.to_string(),
                "fields": ["pid", "ppid", "utime"],
            }),
            "/data/row",
        ),
    ]
}

async fn dispatch(fixture: &Fixture, name: &str, arguments: Value) -> Value {
    dispatch_with_budget(fixture, name, arguments, STRUCTURED_CONTENT_BYTES).await
}

async fn dispatch_with_budget(
    fixture: &Fixture,
    name: &str,
    arguments: Value,
    budget: usize,
) -> Value {
    let Value::Object(mut arguments) = arguments else {
        panic!("{name} test arguments are an object");
    };
    arguments.insert("data_budget_bytes".to_owned(), json!(budget));
    let mut request = CallToolRequestParams::new(name.to_owned());
    request.arguments = Some(arguments);
    tools::dispatch(fixture.state(), request, || false)
        .await
        .unwrap_or_else(|error| panic!("{name} reached central dispatch: {error:?}"))
        .structured_content
        .unwrap_or_else(|| panic!("{name} returned structured content"))
}

fn nonempty(value: &Value) -> bool {
    match value {
        Value::Array(values) => !values.is_empty(),
        Value::Object(values) => !values.is_empty(),
        Value::Null => false,
        _ => true,
    }
}

fn append_fixture(journal: &mut Journal) {
    let mut interner = Interner::new(DictLimits::default());
    let hostname = label(&mut interner, "runtime-host");
    let kernel = label(&mut interner, "6.12");
    let boot = label(&mut interner, "runtime-boot");
    let command = label(&mut interner, "runtime-process");
    let source = label(&mut interner, "postgresql.csv");
    let message = label(&mut interner, "recorded lifecycle event");
    let mut buffers = SectionBuffers::new();

    buffers
        .push(InstanceMetadata {
            ts: Ts(METADATA_AT),
            hostname,
            kernel_version: kernel,
            environment: Environment::Machine.as_u8(),
            clock_ticks_per_sec: 100,
            page_size_bytes: 4_096,
            boot_id: boot,
            btime: Ts(SEGMENT_ID - 1_000_000),
            postgresql_enabled: true,
            postgresql_interval_seconds: 10,
            postgresql_effective_cpus: Some(4),
        })
        .expect("metadata row fits");
    buffers
        .push(process(FIRST_PROCESS_AT, command, 10, 5))
        .expect("first Process row fits");
    buffers
        .push(process(LAST_PROCESS_AT, command, 30, 15))
        .expect("last Process row fits");
    buffers
        .push(PgLogLifecycle {
            ts: Ts(EVENT_AT),
            system_identifier: Some(7),
            source_file: source,
            kind: 0,
            pid: Some(42),
            signal: None,
            shutdown_mode: None,
            message,
            query_detail: None,
        })
        .expect("Event row fits");

    let dictionary = dict::encode(interner.window()).expect("encode runtime dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode runtime fixture")
        .expect("nonempty runtime fixture");
    journal
        .append(SegmentId::new(SEGMENT_ID).expect("segment id"), &part)
        .expect("append runtime fixture");
}

fn process(timestamp: i64, command: StrId, utime: i64, stime: i64) -> OsProcess {
    OsProcess {
        ts: Ts(timestamp),
        pid: 42,
        starttime: Ts(SEGMENT_ID - 500_000),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'R',
        num_threads: 1,
        tty: 0,
        comm: command,
        cmdline: Some(command),
        utime,
        stime,
        nice: 0,
        prio: 20,
        rtprio: 0,
        policy: 0,
        curcpu: 0,
        rundelay_ns: 0,
        blkdelay_ticks: 0,
        nvcsw: 0,
        nivcsw: 0,
        minflt: 0,
        majflt: 0,
        vmem_kb: 1_024,
        rmem_kb: 512,
        vswap_kb: 0,
        syscr: Some(0),
        syscw: Some(0),
        rchar: Some(0),
        wchar: Some(0),
        read_bytes: Some(0),
        write_bytes: Some(0),
        cancelled_write_bytes: Some(0),
        exit_signal: 17,
        scope: 0,
    }
}

fn label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern runtime label")
            .get(),
    )
}
