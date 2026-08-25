use std::cell::Cell;
use std::path::Path;

use hyper::StatusCode;
use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::pg_locks::{PgLocksV1, PgLocksV2};
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::{Value, json};

use super::{MAX_LOCK_GRAPH_ROWS, transform};
use crate::api::{self, ApiError, ValueLimits, ValueStopReason};
use crate::config::SOURCE_POSTGRESQL;
use crate::route::{Order, PostgresqlSurface, PostgresqlSurfaceRequest, Route, SnapshotRequest};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SAMPLE_AT: i64 = SEGMENT_ID + 100;

#[derive(Clone, Copy)]
enum LockLayout {
    V1,
    V2,
}

#[test]
fn product_defaults_and_graph_fields_follow_pg10_18_layouts() {
    for (layout, waitstart) in [(LockLayout::V1, false), (LockLayout::V2, true)] {
        let mut fixture = Fixture::new(layout);
        fixture.append_locks(&[(10, Vec::new()), (20, vec![10])]);
        let collected = collect(fixture.root(), request(None, None, Vec::new()), &|| false)
            .expect("layout-aware PostgreSQL lock graph");
        let names = columns(&collected.records)
            .iter()
            .filter_map(|column| column.get("name").and_then(Value::as_str))
            .collect::<Vec<_>>();

        for required in [
            "pid",
            "blocked_by",
            "datname",
            "query",
            "lock_target",
            "lock_tree_parent_pid",
            "lock_tree_depth",
            "lock_tree_order",
            "lock_tree_extra_blockers",
            "lock_tree_waits_on_prepared",
        ] {
            assert!(
                names.contains(&required),
                "missing {required} from {names:?}"
            );
        }
        assert_eq!(names.contains(&"waitstart"), waitstart);
    }

    let mut old = Fixture::new(LockLayout::V1);
    old.append_locks(&[(10, Vec::new())]);
    let error = collect(
        old.root(),
        request(None, None, vec!["waitstart".to_owned()]),
        &|| false,
    )
    .expect_err("PG10-13 has no waitstart");
    assert!(matches!(error, ApiError::NoSuchColumn(name) if name == "waitstart"));
}

#[test]
fn rows_are_parent_first_with_canonical_edges_and_stable_graph_fields() {
    let input = [
        (90, Vec::new()),
        (30, vec![0, 10, 20]),
        (40, vec![30]),
        (20, Vec::new()),
        (10, Vec::new()),
    ];
    let transformed = transform(records(&input), None).expect("valid PostgreSQL lock graph");
    assert_eq!(pids(&transformed), [10, 30, 40, 20, 90]);
    assert_eq!(graph_values(&transformed, 10), (None, 1, 0, vec![], false));
    assert_eq!(
        graph_values(&transformed, 30),
        (Some(10), 2, 1, vec![20], true)
    );
    assert_eq!(
        graph_values(&transformed, 40),
        (Some(30), 3, 2, vec![], false)
    );
    assert_eq!(graph_values(&transformed, 20), (None, 1, 3, vec![], false));
    assert_eq!(graph_values(&transformed, 90), (None, 1, 4, vec![], false));
    assert_eq!(
        field_for_pid(&transformed, 30, "blocked_by"),
        &json!([0, 10, 20])
    );

    let reversed = input.into_iter().rev().collect::<Vec<_>>();
    let stable = transform(records(&reversed), None).expect("stable PostgreSQL lock graph");
    assert_eq!(graph_rows(&stable), graph_rows(&transformed));
}

#[test]
fn component_representatives_keep_the_web_graph_order() {
    let transformed = transform(
        records(&[(1, vec![100]), (50, Vec::new()), (100, Vec::new())]),
        None,
    )
    .expect("valid disconnected PostgreSQL lock graph");

    assert_eq!(pids(&transformed), [50, 100, 1]);
}

#[test]
fn cyclic_blockers_terminate_with_the_lowest_pid_as_the_stable_root() {
    let transformed =
        transform(records(&[(20, vec![10]), (10, vec![20])]), None).expect("cyclic graph");

    assert_eq!(pids(&transformed), [10, 20]);
    assert_eq!(
        graph_values(&transformed, 10),
        (None, 1, 0, vec![20], false)
    );
    assert_eq!(
        graph_values(&transformed, 20),
        (Some(10), 2, 1, vec![], false)
    );
}

#[test]
fn absent_positive_blocker_stays_recorded_and_is_not_a_graph_error() {
    let transformed =
        transform(records(&[(5, vec![999])]), None).expect("absent blocker is ordinary data");

    assert_eq!(pids(&transformed), [5]);
    assert_eq!(field_for_pid(&transformed, 5, "blocked_by"), &json!([999]));
    assert_eq!(graph_values(&transformed, 5), (None, 1, 0, vec![], false));
}

#[test]
fn graph_find_keeps_parent_and_extra_blocker_paths() {
    let mut fixture = Fixture::new(LockLayout::V2);
    fixture.append_locks(&[
        (10, Vec::new()),
        (20, Vec::new()),
        (30, vec![0, 10, 20]),
        (40, vec![30]),
        (90, Vec::new()),
    ]);

    let collected = collect(
        fixture.root(),
        request(Some("pid:30"), None, Vec::new()),
        &|| false,
    )
    .expect("path-preserving PostgreSQL lock search");

    assert_eq!(pids(&collected.records), [10, 30, 20]);
    assert_eq!(graph_values(&collected.records, 30).0, Some(10));
    assert_eq!(graph_values(&collected.records, 30).3, vec![20]);
}

#[test]
fn product_admits_exactly_500_rows_as_one_whole_graph() {
    let mut fixture = Fixture::new(LockLayout::V2);
    let locks = (1_i32..=500)
        .map(|pid| (pid, Vec::new()))
        .collect::<Vec<_>>();
    fixture.append_locks(&locks);

    let collected = collect(
        fixture.root(),
        request(None, None, vec!["pid".to_owned()]),
        &|| false,
    )
    .expect("500 PostgreSQL lock rows fit the product bound");

    assert_eq!(
        collected
            .records
            .iter()
            .filter(|record| record["record"] == "row")
            .count(),
        MAX_LOCK_GRAPH_ROWS
    );
    assert_eq!(collected.stop_reason, ValueStopReason::Complete);
    let page = page(&collected.records);
    assert_eq!(page["has_more"], false);
    assert_eq!(page["truncated"], false);
    assert_eq!(page["next_cursor"], Value::Null);
    assert_eq!(page["order_by"], json!(["lock_tree_order"]));
}

#[test]
fn product_rejects_501_rows_before_emitting_any_graph_record() {
    let mut fixture = Fixture::new(LockLayout::V2);
    let locks = (1_i32..=501)
        .map(|pid| (pid, Vec::new()))
        .collect::<Vec<_>>();
    fixture.append_locks(&locks);
    let prepared = api::prepare(
        fixture.root(),
        SOURCE_POSTGRESQL,
        Route::Snapshot(Box::new(request(None, None, vec!["pid".to_owned()]))),
        None,
    )
    .expect("prepare bounded PostgreSQL lock graph");
    let mut emitted = Vec::new();
    let error = prepared
        .stream_values(
            &mut |record| {
                emitted.push(record);
                true
            },
            &|| false,
        )
        .expect_err("501 PostgreSQL lock rows exceed the product bound");

    assert_eq!(error.code(), "lock_graph_bound_exceeded");
    assert!(emitted.is_empty());
}

#[test]
fn prepared_graph_stays_on_its_captured_active_prefix() {
    let mut fixture = Fixture::new(LockLayout::V2);
    fixture.append_locks(&[(10, Vec::new())]);
    let prepared = api::prepare(
        fixture.root(),
        SOURCE_POSTGRESQL,
        Route::Snapshot(Box::new(request(None, None, vec!["pid".to_owned()]))),
        None,
    )
    .expect("prepare pinned PostgreSQL lock graph");
    fixture.append_locks(&[(20, vec![10])]);

    let collected = prepared
        .collect_values(
            ValueLimits {
                records: 32,
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect("read pinned PostgreSQL lock graph");

    assert_eq!(pids(&collected.records), [10]);
    assert!(collected.records[0]["segment"]["active_wal_position"].is_string());
}

#[test]
fn graph_find_pins_both_passes_to_the_requested_active_prefix() {
    let mut fixture = Fixture::new(LockLayout::V2);
    fixture.append_locks(&[(10, Vec::new()), (20, vec![10])]);
    let initial = collect(fixture.root(), request(None, None, Vec::new()), &|| false)
        .expect("initial PostgreSQL lock graph");
    let active_position = initial.records[0]["segment"]["active_wal_position"]
        .as_str()
        .and_then(|value| value.parse().ok())
        .expect("captured active WAL position");
    fixture.append_locks(&[(30, Vec::new())]);
    let current = collect(fixture.root(), request(None, None, Vec::new()), &|| false)
        .expect("current PostgreSQL lock graph");
    assert_eq!(pids(&current.records), [10, 20, 30]);

    let mut pinned = request(Some("pid:20"), None, Vec::new());
    pinned.active_position = Some(active_position);
    let collected = collect(fixture.root(), pinned, &|| false)
        .expect("searched PostgreSQL lock graph at the captured prefix");

    assert_eq!(pids(&collected.records), [10, 20]);
    assert_eq!(
        collected.records[0]["segment"]["active_wal_position"],
        json!(active_position.to_string())
    );
}

#[test]
fn explicit_http_segment_and_validator_share_the_lock_graph_source() {
    let mut fixture = Fixture::new(LockLayout::V2);
    fixture.append_locks(&[(10, Vec::new())]);
    fixture.finish_and_continue(SEGMENT_ID + 1);
    fixture.append_locks(&[(20, Vec::new())]);
    fixture.finish();

    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    let query = format!("at={SAMPLE_AT}&section=pg_locks&lens=graph");
    let route = crate::route::parse(&path, Some(&query)).expect("HTTP Locks graph route");
    let prepared = api::prepare(fixture.root(), SOURCE_POSTGRESQL, route, None)
        .expect("prepare explicit HTTP Locks graph");
    let meta = prepared.meta();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, api::CachePolicy::Revalidate);
    let etag = meta.etag.expect("finished Locks graph ETag");
    let collected = prepared
        .collect_values(
            ValueLimits {
                records: MAX_LOCK_GRAPH_ROWS.saturating_add(32),
                ndjson_bytes: usize::MAX,
            },
            &|| false,
        )
        .expect("read explicit HTTP Locks graph");

    assert_eq!(pids(&collected.records), [10]);
    let snapshot = collected
        .records
        .iter()
        .find(|record| record["record"] == "snapshot")
        .expect("Locks graph snapshot anchor");
    assert_eq!(snapshot["segment"]["id"], json!(SEGMENT_ID.to_string()));

    let route = crate::route::parse(&path, Some(&query)).expect("HTTP Locks graph route");
    let not_modified = api::prepare(fixture.root(), SOURCE_POSTGRESQL, route, Some(&etag))
        .expect("validate explicit HTTP Locks graph");
    assert_eq!(not_modified.meta().status, StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.meta().etag.as_deref(), Some(etag.as_str()));
}

#[test]
fn cancellation_during_collection_emits_no_partial_product() {
    let mut fixture = Fixture::new(LockLayout::V2);
    fixture.append_locks(&[(10, Vec::new()), (20, vec![10])]);
    let checks = Cell::new(0_usize);
    let cancelled = || {
        let current = checks.get().saturating_add(1);
        checks.set(current);
        current >= 4
    };
    let collected = collect(fixture.root(), request(None, None, Vec::new()), &cancelled)
        .expect("cancelled PostgreSQL lock graph read");

    assert!(checks.get() >= 4);
    assert!(collected.records.is_empty());
    assert_eq!(collected.stop_reason, ValueStopReason::Cancelled);
}

fn collect(
    root: &Path,
    request: SnapshotRequest,
    cancelled: &impl Fn() -> bool,
) -> Result<api::ValueCollection, ApiError> {
    api::prepare(
        root,
        SOURCE_POSTGRESQL,
        Route::Snapshot(Box::new(request)),
        None,
    )?
    .collect_values(
        ValueLimits {
            records: MAX_LOCK_GRAPH_ROWS.saturating_add(32),
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
        sections: vec!["pg_locks".to_owned()],
        fields,
        by: Vec::new(),
        direction: Order::Desc,
        group: None,
        postgresql: Some(PostgresqlSurfaceRequest {
            surface: PostgresqlSurface::Locks,
            order: None,
        }),
        process: None,
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

fn records(locks: &[(i64, Vec<i64>)]) -> Vec<Value> {
    let mut records = vec![
        json!({"record": "snapshot", "segment": {"id": SEGMENT_ID.to_string()}, "at": SAMPLE_AT.to_string()}),
        json!({
            "record": "layout",
            "layout": {
                "logical_name": "pg_locks",
                "physical_name": "pg_locks",
                "type_id": "1011002",
                "columns": [
                    {"name": "pid", "type": "i32", "available": true},
                    {"name": "blocked_by", "type": "list_i32", "available": true},
                ],
            },
        }),
    ];
    records.extend(
        locks
            .iter()
            .enumerate()
            .map(|(ordinal, (pid, blocked_by))| {
                json!({
                    "record": "row",
                    "type_id": "1011002",
                    "ordinal": ordinal.to_string(),
                    "segment_id": SEGMENT_ID.to_string(),
                    "timestamp": SAMPLE_AT.to_string(),
                    "values": [pid, blocked_by],
                })
            }),
    );
    records
}

fn columns(records: &[Value]) -> &[Value] {
    records
        .iter()
        .find(|record| record["record"] == "layout")
        .and_then(|record| record.pointer("/layout/columns"))
        .and_then(Value::as_array)
        .expect("PostgreSQL lock layout columns")
}

fn field_for_pid<'a>(records: &'a [Value], pid: i64, name: &str) -> &'a Value {
    let row = records
        .iter()
        .filter(|record| record["record"] == "row")
        .find(|row| {
            let columns = columns_for_row(records, row);
            row["values"][index(columns, "pid")].as_i64() == Some(pid)
        })
        .expect("PostgreSQL lock row PID");
    let columns = columns_for_row(records, row);
    &row["values"][index(columns, name)]
}

fn columns_for_row<'a>(records: &'a [Value], row: &Value) -> &'a [Value] {
    let type_id = row["type_id"].as_str().expect("row type id");
    records
        .iter()
        .find(|record| {
            record["record"] == "layout"
                && record.pointer("/layout/type_id").and_then(Value::as_str) == Some(type_id)
        })
        .and_then(|record| record.pointer("/layout/columns"))
        .and_then(Value::as_array)
        .expect("matching PostgreSQL lock layout")
}

fn index(columns: &[Value], name: &str) -> usize {
    columns
        .iter()
        .position(|column| column["name"] == name)
        .expect("PostgreSQL lock graph field")
}

fn pids(records: &[Value]) -> Vec<i64> {
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|row| {
            let fields = columns_for_row(records, row);
            row["values"][index(fields, "pid")].as_i64().expect("PID")
        })
        .collect()
}

fn graph_values(records: &[Value], pid: i64) -> (Option<i64>, u64, u64, Vec<i64>, bool) {
    (
        field_for_pid(records, pid, "lock_tree_parent_pid").as_i64(),
        field_for_pid(records, pid, "lock_tree_depth")
            .as_u64()
            .expect("tree depth"),
        field_for_pid(records, pid, "lock_tree_order")
            .as_u64()
            .expect("tree order"),
        field_for_pid(records, pid, "lock_tree_extra_blockers")
            .as_array()
            .expect("extra blockers")
            .iter()
            .map(|value| value.as_i64().expect("extra blocker PID"))
            .collect(),
        field_for_pid(records, pid, "lock_tree_waits_on_prepared")
            .as_bool()
            .expect("prepared wait flag"),
    )
}

type GraphRow = (i64, Option<i64>, u64, u64, Vec<i64>, bool);

fn graph_rows(records: &[Value]) -> Vec<GraphRow> {
    pids(records)
        .into_iter()
        .map(|pid| {
            let (parent, depth, order, extra, prepared) = graph_values(records, pid);
            (pid, parent, depth, order, extra, prepared)
        })
        .collect()
}

fn page(records: &[Value]) -> &Value {
    records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("PostgreSQL lock graph page")
}

struct Fixture {
    directory: tempfile::TempDir,
    writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
    layout: LockLayout,
}

impl Fixture {
    fn new(layout: LockLayout) -> Self {
        let directory = tempfile::tempdir().expect("temporary PostgreSQL lock data root");
        let root = DataRoot::open(directory.path()).expect("open PostgreSQL lock data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire PostgreSQL lock writer");
        let journal =
            Journal::open(&writer, JournalConfig::default()).expect("open PostgreSQL lock journal");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        Self {
            directory,
            writer,
            journal,
            address,
            layout,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn append_locks(&mut self, locks: &[(i32, Vec<i32>)]) {
        let mut interner = Interner::new(DictLimits::default());
        let labels = Labels::new(&mut interner);
        let dictionary =
            dict::encode(interner.window()).expect("encode PostgreSQL lock dictionary");
        let mut buffers = SectionBuffers::new();
        for (pid, blocked_by) in locks {
            match self.layout {
                LockLayout::V1 => buffers
                    .push(lock_v1(*pid, blocked_by.clone(), labels))
                    .expect("PostgreSQL lock V1 row fits"),
                LockLayout::V2 => buffers
                    .push(lock_v2(*pid, blocked_by.clone(), labels))
                    .expect("PostgreSQL lock V2 row fits"),
            }
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode PostgreSQL lock fixture")
            .expect("nonempty PostgreSQL lock fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append PostgreSQL lock fixture");
    }

    fn finish_and_continue(&mut self, segment_id: i64) {
        write_segment(&self.journal, &self.writer, self.address)
            .expect("finish PostgreSQL lock fixture segment");
        self.journal
            .reset()
            .expect("reset PostgreSQL lock fixture journal");
        self.address = SegmentAddress::new(SegmentId::new(segment_id).expect("segment id"))
            .expect("segment address");
    }

    fn finish(&self) {
        write_segment(&self.journal, &self.writer, self.address)
            .expect("finish PostgreSQL lock fixture segment");
    }
}

#[derive(Clone, Copy)]
struct Labels {
    database: StrId,
    role: StrId,
    application: StrId,
    client: StrId,
    backend: StrId,
    active: StrId,
    query: StrId,
    locktype: StrId,
    mode: StrId,
    target: StrId,
}

impl Labels {
    fn new(interner: &mut Interner) -> Self {
        Self {
            database: label(interner, "inventory"),
            role: label(interner, "analyst"),
            application: label(interner, "worker"),
            client: label(interner, "127.0.0.1"),
            backend: label(interner, "client backend"),
            active: label(interner, "active"),
            query: label(interner, "update items set seen = true"),
            locktype: label(interner, "transactionid"),
            mode: label(interner, "ShareLock"),
            target: label(interner, "transaction 42"),
        }
    }
}

fn lock_v2(pid: i32, blocked_by: Vec<i32>, labels: Labels) -> PgLocksV2 {
    PgLocksV2 {
        ts: Ts(SAMPLE_AT),
        pid,
        blocked_by,
        datid: 16_384,
        datname: labels.database,
        usename: Some(labels.role),
        application_name: labels.application,
        client_addr: labels.client,
        backend_type: labels.backend,
        state: Some(labels.active),
        wait_event_type: Some(labels.locktype),
        wait_event: Some(labels.mode),
        query: labels.query,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Some(Ts(SAMPLE_AT - 60_000_000)),
        xact_start: Some(Ts(SAMPLE_AT - 5_000_000)),
        query_start: Some(Ts(SAMPLE_AT - 1_000_000)),
        state_change: Some(Ts(SAMPLE_AT - 1_000_000)),
        lock_locktype: Some(labels.locktype),
        lock_mode: Some(labels.mode),
        lock_database: Some(16_384),
        lock_relation: None,
        lock_relname: None,
        lock_page: None,
        lock_tuple: None,
        lock_virtualxid: None,
        lock_transactionid: Some(42),
        lock_classid: None,
        lock_objid: None,
        lock_objsubid: None,
        lock_target: Some(labels.target),
        waitstart: Some(Ts(SAMPLE_AT - 100_000)),
    }
}

fn lock_v1(pid: i32, blocked_by: Vec<i32>, labels: Labels) -> PgLocksV1 {
    let row = lock_v2(pid, blocked_by, labels);
    PgLocksV1 {
        ts: row.ts,
        pid: row.pid,
        blocked_by: row.blocked_by,
        datid: row.datid,
        datname: row.datname,
        usename: row.usename,
        application_name: row.application_name,
        client_addr: row.client_addr,
        backend_type: row.backend_type,
        state: row.state,
        wait_event_type: row.wait_event_type,
        wait_event: row.wait_event,
        query: row.query,
        backend_xid_age: row.backend_xid_age,
        backend_xmin_age: row.backend_xmin_age,
        backend_start: row.backend_start,
        xact_start: row.xact_start,
        query_start: row.query_start,
        state_change: row.state_change,
        lock_locktype: row.lock_locktype,
        lock_mode: row.lock_mode,
        lock_database: row.lock_database,
        lock_relation: row.lock_relation,
        lock_relname: row.lock_relname,
        lock_page: row.lock_page,
        lock_tuple: row.lock_tuple,
        lock_virtualxid: row.lock_virtualxid,
        lock_transactionid: row.lock_transactionid,
        lock_classid: row.lock_classid,
        lock_objid: row.lock_objid,
        lock_objsubid: row.lock_objsubid,
        lock_target: row.lock_target,
    }
}

fn label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern PostgreSQL lock fixture label")
            .get(),
    )
}
