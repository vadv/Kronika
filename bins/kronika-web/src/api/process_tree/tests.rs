use std::path::Path;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_process::OsProcess;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Value, json};

use super::{MAX_PROCESS_TREE_ROWS, transform};
use crate::api::{self, ApiError, ValueLimits, ValueStopReason};
use crate::config::SOURCE_OS;
use crate::route::{Order, ProcessLens, ProcessSurfaceRequest, Route, SnapshotRequest};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SAMPLE_AT: i64 = SEGMENT_ID + 100;

#[test]
fn rows_are_parent_first_with_canonical_product_fields() {
    let transformed = transform(records(&[(3, 1), (1, 0), (5, 2), (2, 1), (4, 1)]), None)
        .expect("valid Process tree");

    assert_eq!(
        tree_values(&transformed),
        [
            (1, None, 0, 0),
            (2, Some(1), 1, 1),
            (5, Some(2), 2, 2),
            (3, Some(1), 1, 3),
            (4, Some(1), 1, 4),
        ]
    );
    assert_eq!(
        physical_values(&transformed),
        [
            (1, 0, (SEGMENT_ID - 999_999).to_string()),
            (2, 1, (SEGMENT_ID - 999_998).to_string()),
            (5, 2, (SEGMENT_ID - 999_995).to_string()),
            (3, 1, (SEGMENT_ID - 999_997).to_string()),
            (4, 1, (SEGMENT_ID - 999_996).to_string()),
        ]
    );
}

#[test]
fn product_find_keeps_matches_and_their_ancestors() {
    let mut fixture = Fixture::new();
    fixture.append_processes(&[(1, 0), (2, 1), (3, 2), (4, 1)]);

    let collected = collect(
        fixture.root(),
        request(Some("pid:3"), None, Vec::new()),
        &|| false,
    )
    .expect("ancestor-preserving Process tree");

    assert_eq!(
        tree_values(&collected.records),
        [(1, None, 0, 0), (2, Some(1), 1, 1), (3, Some(2), 2, 2)]
    );
    let page = collected
        .records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("whole-tree page");
    assert_eq!(page["has_more"], false);
    assert_eq!(page["next_cursor"], Value::Null);
    assert_eq!(page["order_by"], json!(["process_tree_order"]));
}

#[test]
fn product_admits_exactly_500_rows_as_one_complete_tree() {
    let mut fixture = Fixture::new();
    let processes = (1_i32..=500).map(|pid| (pid, 0)).collect::<Vec<_>>();
    fixture.append_processes(&processes);

    let collected = collect(
        fixture.root(),
        request(None, None, vec!["pid".to_owned()]),
        &|| false,
    )
    .expect("500 Process rows fit the product bound");

    assert_eq!(
        collected
            .records
            .iter()
            .filter(|record| record["record"] == "row")
            .count(),
        MAX_PROCESS_TREE_ROWS
    );
    assert_eq!(collected.stop_reason, ValueStopReason::Complete);
}

#[test]
fn product_rejects_501_rows_without_emitting_a_partial_tree() {
    let mut fixture = Fixture::new();
    let processes = (1_i32..=501).map(|pid| (pid, 0)).collect::<Vec<_>>();
    fixture.append_processes(&processes);
    let prepared = api::prepare(
        fixture.root(),
        SOURCE_OS,
        Route::Snapshot(Box::new(request(None, None, vec!["pid".to_owned()]))),
        None,
    )
    .expect("prepare bounded tree");
    let error = prepared
        .collect_values(
            ValueLimits {
                records: MAX_PROCESS_TREE_ROWS.saturating_add(32),
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect_err("501 Process rows exceed the product bound");

    assert_eq!(error.code(), "tree_bound_exceeded");
}

#[test]
fn prepared_tree_stays_on_its_captured_active_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_processes(&[(1, 0)]);
    let prepared = api::prepare(
        fixture.root(),
        SOURCE_OS,
        Route::Snapshot(Box::new(request(None, None, vec!["pid".to_owned()]))),
        None,
    )
    .expect("prepare pinned tree");
    fixture.append_processes(&[(2, 1)]);

    let collected = prepared
        .collect_values(
            ValueLimits {
                records: 32,
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect("read pinned tree");

    assert_eq!(tree_values(&collected.records), [(1, None, 0, 0)]);
    assert!(collected.records[0]["segment"]["active_wal_position"].is_string());
}

#[test]
fn cancelled_tree_emits_no_partial_product() {
    let mut fixture = Fixture::new();
    fixture.append_processes(&[(1, 0), (2, 1)]);
    let collected = collect(fixture.root(), request(None, None, Vec::new()), &|| true)
        .expect("cancelled tree read");

    assert!(collected.records.is_empty());
    assert_eq!(collected.stop_reason, ValueStopReason::Cancelled);
}

fn collect(
    root: &Path,
    request: SnapshotRequest,
    cancelled: &impl Fn() -> bool,
) -> Result<api::ValueCollection, ApiError> {
    api::prepare(root, SOURCE_OS, Route::Snapshot(Box::new(request)), None)?.collect_values(
        ValueLimits {
            records: MAX_PROCESS_TREE_ROWS.saturating_add(32),
            ndjson_bytes: usize::MAX,
        },
        cancelled,
    )
}

fn request(search: Option<&str>, cursor: Option<&str>, fields: Vec<String>) -> SnapshotRequest {
    SnapshotRequest {
        segment_id: SEGMENT_ID,
        active_position: None,
        at: SAMPLE_AT,
        sections: vec!["os_process".to_owned()],
        fields,
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        postgresql: None,
        process: Some(ProcessSurfaceRequest {
            lens: ProcessLens::Tree,
            order: None,
            direction: None,
        }),
        page_size: None,
        cursor: cursor.map(str::to_owned),
        search: search.map(str::to_owned),
        first_match: false,
        text: None,
        filters: Vec::new(),
        activity_visibility: None,
        type_id: None,
        row_ordinal: None,
    }
}

fn records(processes: &[(i64, i64)]) -> Vec<Value> {
    let mut records = vec![
        json!({"record": "snapshot", "segment": {"id": SEGMENT_ID.to_string()}, "at": SAMPLE_AT.to_string()}),
        layout(),
    ];
    records.extend(
        processes
            .iter()
            .enumerate()
            .map(|(ordinal, &(pid, ppid))| row(pid, ppid, ordinal)),
    );
    records
}

fn layout() -> Value {
    json!({
        "record": "layout",
        "layout": {
            "logical_name": "os_process",
            "physical_name": "os_process",
            "type_id": "1100001",
            "columns": [
                {"name": "pid", "type": "i32", "available": true},
                {"name": "ppid", "type": "i32", "available": true},
                {"name": "starttime", "type": "timestamp_us", "available": true},
            ],
        },
    })
}

#[expect(
    clippy::similar_names,
    reason = "pid and ppid are the canonical distinct process identifier fields under test"
)]
fn row(pid: i64, ppid: i64, ordinal: usize) -> Value {
    json!({
        "record": "row",
        "type_id": "1100001",
        "ordinal": ordinal.to_string(),
        "segment_id": SEGMENT_ID.to_string(),
        "timestamp": SAMPLE_AT.to_string(),
        "values": [pid, ppid, (SEGMENT_ID - 1_000_000 + pid).to_string()],
    })
}

fn tree_values(records: &[Value]) -> Vec<(i64, Option<i64>, u64, u64)> {
    let columns = columns(records);
    let pid = index(columns, "pid");
    let parent = index(columns, "process_tree_parent_pid");
    let depth = index(columns, "process_tree_depth");
    let order = index(columns, "process_tree_order");
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|row| {
            let values = row["values"].as_array().expect("row values");
            (
                values[pid].as_i64().expect("PID"),
                values[parent].as_i64(),
                values[depth].as_u64().expect("depth"),
                values[order].as_u64().expect("tree order"),
            )
        })
        .collect()
}

#[expect(
    clippy::similar_names,
    reason = "pid and ppid are the canonical distinct process identifier fields under test"
)]
fn physical_values(records: &[Value]) -> Vec<(i64, i64, String)> {
    let columns = columns(records);
    let pid = index(columns, "pid");
    let ppid = index(columns, "ppid");
    let starttime = index(columns, "starttime");
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|row| {
            let values = row["values"].as_array().expect("row values");
            (
                values[pid].as_i64().expect("PID"),
                values[ppid].as_i64().expect("PPID"),
                values[starttime]
                    .as_str()
                    .expect("lossless start time")
                    .to_owned(),
            )
        })
        .collect()
}

fn columns(records: &[Value]) -> &[Value] {
    records
        .iter()
        .find(|record| record["record"] == "layout")
        .and_then(|record| record.pointer("/layout/columns"))
        .and_then(Value::as_array)
        .expect("tree layout columns")
}

fn index(columns: &[Value], name: &str) -> usize {
    columns
        .iter()
        .position(|column| column["name"] == name)
        .expect("tree field")
}

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary Process tree data root");
        let root = DataRoot::open(directory.path()).expect("open Process tree data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire Process tree writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open Process tree journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            _writer: writer,
            journal,
            address,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn append_processes(&mut self, processes: &[(i32, i32)]) {
        let mut interner = Interner::new(DictLimits::default());
        let command = StrId(
            interner
                .intern(b"tree-process")
                .expect("intern Process command")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("encode Process dictionary");
        let mut buffers = SectionBuffers::new();
        for &(pid, ppid) in processes {
            buffers
                .push(process(pid, ppid, command))
                .expect("Process row fits");
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode Process tree fixture")
            .expect("nonempty Process tree fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append Process tree fixture");
    }
}

#[expect(
    clippy::similar_names,
    reason = "pid and ppid are the canonical distinct process identifier fields under test"
)]
fn process(pid: i32, ppid: i32, command: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(SAMPLE_AT),
        pid,
        starttime: Ts(SEGMENT_ID - 1_000_000 + i64::from(pid)),
        ppid,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'S',
        num_threads: 1,
        tty: 0,
        comm: command,
        cmdline: Some(command),
        utime: 0,
        stime: 0,
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
        vmem_kb: 0,
        rmem_kb: 0,
        vswap_kb: 0,
        syscr: None,
        syscw: None,
        rchar: None,
        wchar: None,
        read_bytes: None,
        write_bytes: None,
        cancelled_write_bytes: None,
        exit_signal: 17,
        scope: 0,
    }
}
