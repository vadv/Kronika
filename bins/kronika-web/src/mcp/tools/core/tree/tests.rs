use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_process::OsProcess;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Value, json};

use super::{prepare, transform};
use crate::config::SOURCE_OS;
use crate::mcp::{STRUCTURED_CONTENT_BYTES, State};
use crate::route::{Order, SnapshotRequest};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SAMPLE_AT: i64 = SEGMENT_ID + 100;

#[test]
fn transform_returns_parent_first_rows_with_stable_machine_fields() {
    let transformed = transform(records(&[(3, 1), (1, 0), (5, 2), (2, 1), (4, 1)]), None)
        .expect("valid Process tree");

    assert_eq!(
        tree_values(&transformed.records),
        [
            (1, None, 0, 0),
            (2, Some(1), 1, 1),
            (5, Some(2), 2, 2),
            (3, Some(1), 1, 3),
            (4, Some(1), 1, 4),
        ]
    );
}

#[test]
fn transform_keeps_matching_rows_and_their_ancestors() {
    let full = records(&[(1, 0), (2, 1), (3, 2), (4, 1)]);
    let matched = vec![row(3, 2, 2)];
    let transformed = transform(full, Some(&matched)).expect("ancestor-preserving tree search");

    assert_eq!(
        tree_values(&transformed.records),
        [(1, None, 0, 0), (2, Some(1), 1, 1), (3, Some(2), 2, 2)]
    );
}

#[test]
fn transform_keeps_every_row_when_recorded_parents_form_a_cycle() {
    let transformed =
        transform(records(&[(2, 1), (1, 2), (3, 99)]), None).expect("bounded cyclic tree");

    assert_eq!(
        tree_values(&transformed.records),
        [(3, None, 0, 0), (1, None, 0, 1), (2, Some(1), 1, 2)]
    );
}

#[test]
fn transform_rejects_more_than_the_complete_tree_bound() {
    let rows = (0..=super::super::MAX_TREE_ROWS)
        .map(|pid| (i64::try_from(pid).expect("test PID fits i64"), -1))
        .collect::<Vec<_>>();
    let error = transform(records(&rows), None).expect_err("501 rows exceed the tree bound");

    assert_eq!(error.code, "tree_bound_exceeded");
}

#[test]
fn prepare_makes_tree_reads_complete_and_keeps_shared_search_separate() {
    let prepared = prepare(request(Some("pid:3"), None)).expect("valid tree request");

    assert_eq!(
        (
            prepared.complete.fields,
            prepared.complete.by,
            prepared.complete.direction,
            prepared.complete.page_size,
            prepared.complete.search,
            prepared.matched.and_then(|request| request.search),
        ),
        (
            vec![
                "comm".to_owned(),
                "pid".to_owned(),
                "ppid".to_owned(),
                "starttime".to_owned(),
            ],
            vec!["pid".to_owned()],
            Order::Asc,
            None,
            None,
            Some("pid:3".to_owned()),
        )
    );
}

#[test]
fn prepare_rejects_a_partial_tree_cursor() {
    let error = prepare(request(None, Some("opaque"))).expect_err("tree cursor is partial");

    assert_eq!(error.parameter.as_deref(), Some("cursor"));
}

#[test]
fn process_handler_reads_and_transforms_one_complete_recorded_snapshot() {
    let mut fixture = Fixture::new();
    fixture.append_processes(&[(1, 0), (2, 1), (3, 1)]);
    let state = State {
        data_root: fixture.root().to_owned(),
        sources: SOURCE_OS,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    let args = json!({
        "at_us": SAMPLE_AT.to_string(),
        "lens": "tree",
        "data_budget_bytes": STRUCTURED_CONTENT_BYTES,
    })
    .as_object()
    .expect("object arguments")
    .clone();

    let payload = super::super::execute(
        &state,
        "kronika_find_processes",
        &args,
        STRUCTURED_CONTENT_BYTES,
        &|| false,
    )
    .expect("recorded Process tree");
    let records = payload
        .data
        .get("processes")
        .and_then(Value::as_array)
        .expect("Process records");
    let rows = records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("row"))
        .collect::<Vec<_>>();

    assert_eq!(
        (
            tree_values(records),
            physical_values(records),
            rows.iter().all(|row| {
                ["segment_id", "type_id", "ordinal", "timestamp"]
                    .iter()
                    .all(|field| row.get(field).is_some_and(|value| !value.is_null()))
            }),
        ),
        (
            vec![(1, None, 0, 0), (2, Some(1), 1, 1), (3, Some(1), 1, 2)],
            vec![
                (1, 0, (SEGMENT_ID - 999_999).to_string()),
                (2, 1, (SEGMENT_ID - 999_998).to_string()),
                (3, 1, (SEGMENT_ID - 999_997).to_string()),
            ],
            true,
        )
    );
}

#[test]
fn process_handler_rejects_an_over_bound_snapshot_instead_of_returning_a_partial_tree() {
    let mut fixture = Fixture::new();
    let processes = (1_i32..=501).map(|pid| (pid, 0)).collect::<Vec<_>>();
    fixture.append_processes(&processes);
    let state = State {
        data_root: fixture.root().to_owned(),
        sources: SOURCE_OS,
        synthetic_demo: false,
        heavy_scans: Arc::new(tokio::sync::Semaphore::new(2)),
    };
    let args = json!({
        "at_us": SAMPLE_AT.to_string(),
        "lens": "tree",
        "data_budget_bytes": STRUCTURED_CONTENT_BYTES,
    })
    .as_object()
    .expect("object arguments")
    .clone();

    let error = match super::super::execute(
        &state,
        "kronika_find_processes",
        &args,
        STRUCTURED_CONTENT_BYTES,
        &|| false,
    ) {
        Ok(_payload) => panic!("an over-bound snapshot became a partial Process tree"),
        Err(error) => error,
    };

    assert_eq!(error.code, "tree_bound_exceeded");
}

fn request(search: Option<&str>, cursor: Option<&str>) -> SnapshotRequest {
    SnapshotRequest {
        segment_id: SEGMENT_ID,
        at: SAMPLE_AT,
        sections: vec!["os_process".to_owned()],
        fields: vec!["comm".to_owned(), "tree_order".to_owned()],
        by: vec!["tree".to_owned()],
        direction: Order::Desc,
        group: None,
        page_size: Some(1),
        cursor: cursor.map(str::to_owned),
        search: search.map(str::to_owned),
        first_match: false,
        text: None,
        filters: Vec::new(),
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
    let columns = records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("layout"))
        .and_then(|record| record.pointer("/layout/columns"))
        .and_then(Value::as_array)
        .expect("tree layout columns");
    let index = |name| {
        columns
            .iter()
            .position(|column| column.get("name").and_then(Value::as_str) == Some(name))
            .expect("tree field")
    };
    let pid = index("pid");
    let parent = index("parent_pid");
    let depth = index("depth");
    let order = index("tree_order");
    records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("row"))
        .map(|row| {
            let values = row
                .get("values")
                .and_then(Value::as_array)
                .expect("row values");
            (
                values[pid].as_i64().expect("PID"),
                values[parent].as_i64(),
                values[depth].as_u64().expect("depth"),
                values[order].as_u64().expect("tree order"),
            )
        })
        .collect()
}

fn physical_values(records: &[Value]) -> Vec<(i64, i64, String)> {
    let columns = records
        .iter()
        .find(|record| record.get("record").and_then(Value::as_str) == Some("layout"))
        .and_then(|record| record.pointer("/layout/columns"))
        .and_then(Value::as_array)
        .expect("tree layout columns");
    let index = |name| {
        columns
            .iter()
            .position(|column| column.get("name").and_then(Value::as_str) == Some(name))
            .expect("physical tree field")
    };
    let pid = index("pid");
    let ppid = index("ppid");
    let starttime = index("starttime");
    records
        .iter()
        .filter(|record| record.get("record").and_then(Value::as_str) == Some("row"))
        .map(|row| {
            let values = row
                .get("values")
                .and_then(Value::as_array)
                .expect("row values");
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
