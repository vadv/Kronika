use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::pg_prepared_xacts::PgPreparedXacts;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use super::{admit_complete_graph, build_graph, execute};
use crate::mcp::State;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const AT: i64 = SEGMENT_ID + 100;

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary lock data root");
        let root = DataRoot::open(directory.path()).expect("open lock data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire fixture writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open fixture WAL");
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

    fn append_lock_graph(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let datname = label(&mut interner, "inventory");
        let user = label(&mut interner, "analyst");
        let application = label(&mut interner, "worker");
        let backend = label(&mut interner, "client backend");
        let active = label(&mut interner, "active");
        let query = label(&mut interner, "update items set seen = true");
        let locktype = label(&mut interner, "transactionid");
        let mode = label(&mut interner, "ShareLock");
        let target = label(&mut interner, "transaction 42");

        let mut buffers = SectionBuffers::new();
        buffers
            .push(lock_row(
                10,
                Vec::new(),
                datname,
                user,
                application,
                backend,
                active,
                query,
                locktype,
                mode,
                target,
            ))
            .expect("root lock row fits");
        buffers
            .push(lock_row(
                20,
                vec![0, 10],
                datname,
                user,
                application,
                backend,
                active,
                query,
                locktype,
                mode,
                target,
            ))
            .expect("waiting lock row fits");
        buffers
            .push(PgPreparedXacts {
                ts: Ts(AT),
                datname,
                prepared_count: 1,
                max_age_us: 2_000_000,
                max_xid_age_tx: 17,
            })
            .expect("prepared transaction row fits");
        let dictionary = dict::encode(interner.window()).expect("encode lock dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode lock fixture")
            .expect("nonempty lock fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append lock fixture");
    }

    fn state(&self) -> State {
        State {
            data_root: self.root().to_owned(),
            sources: 0b10,
            synthetic_demo: false,
            heavy_scans: Arc::new(Semaphore::new(2)),
        }
    }
}

#[test]
fn graph_is_parent_first_and_preserves_extra_and_prepared_edges() {
    let rows = vec![
        projected_row(10, &[], "inventory", "root"),
        projected_row(20, &[], "inventory", "other root"),
        projected_row(30, &[0, 10, 20], "inventory", "waiter"),
        projected_row(40, &[30], "inventory", "deep waiter"),
        projected_row(90, &[], "other", "disconnected"),
    ];
    let prepared = vec![prepared_row("inventory")];

    let (locks, components) = build_graph(rows, &prepared).expect("complete graph");
    assert_eq!(
        locks
            .iter()
            .filter_map(|row| row.pointer("/values/pid").and_then(Value::as_i64))
            .collect::<Vec<_>>(),
        [10, 30, 40, 20, 90]
    );
    let waiter = locks
        .iter()
        .find(|row| row.pointer("/values/pid") == Some(&json!(30)))
        .expect("waiter row");
    assert_eq!(
        waiter.pointer("/values/lock_tree_parent_pid"),
        Some(&json!(10))
    );
    assert_eq!(waiter.pointer("/values/lock_tree_depth"), Some(&json!(2)));
    assert_eq!(
        waiter.pointer("/values/lock_tree_extra_blockers"),
        Some(&json!([20]))
    );
    assert_eq!(
        waiter.pointer("/values/lock_tree_waits_on_prepared"),
        Some(&json!(true))
    );
    assert_eq!(
        waiter.pointer("/accepted_finding/source"),
        Some(&json!("kronika_index"))
    );
    assert_eq!(components.len(), 2);
    assert_eq!(components[0]["root_pids"], json!([10, 20]));
    assert_eq!(components[0]["member_pids"], json!([10, 30, 40, 20]));
    assert_eq!(components[0]["prepared_waiter_pids"], json!([30]));
    assert_eq!(
        components[0]["prepared_transactions"]
            .as_array()
            .map(Vec::len),
        Some(1)
    );
}

#[test]
fn graph_refuses_missing_positive_blocker_rows() {
    let failure = build_graph(vec![projected_row(30, &[10], "inventory", "waiter")], &[])
        .expect_err("incomplete graph");
    assert_eq!(failure.code, "incomplete_lock_graph");
}

#[test]
fn graph_admission_refuses_partial_or_oversized_sets() {
    let partial = json!({"truncated": true});
    assert_eq!(
        admit_complete_graph(&partial, 10)
            .expect_err("partial graph")
            .code,
        "whole_set_bound_exceeded"
    );
    let complete = json!({"truncated": false});
    assert_eq!(
        admit_complete_graph(&complete, 501)
            .expect_err("oversized graph")
            .code,
        "whole_set_bound_exceeded"
    );
    admit_complete_graph(&complete, 500).expect("complete bounded graph");
}

#[test]
fn handler_returns_complete_components_locators_and_prepared_facts() {
    let mut fixture = Fixture::new();
    fixture.append_lock_graph();
    let arguments = json!({"at_us": AT.to_string()});
    let payload = execute(
        &fixture.state(),
        arguments.as_object().expect("argument object"),
        &|| false,
    )
    .expect("lock handler result");

    let locks = payload.data["locks"].as_array().expect("lock rows");
    assert_eq!(locks.len(), 2);
    assert_eq!(locks[0].pointer("/values/pid"), Some(&json!(10)));
    assert_eq!(locks[1].pointer("/values/pid"), Some(&json!(20)));
    assert_eq!(
        locks[1].pointer("/values/blocked_by"),
        Some(&json!([0, 10]))
    );
    assert_eq!(
        locks[1].pointer("/values/lock_target"),
        Some(&json!("transaction 42"))
    );
    assert_eq!(locks[1]["segment_id"], json!(SEGMENT_ID.to_string()));
    assert_eq!(locks[1]["timestamp"], json!(AT.to_string()));
    assert!(locks[1]["ordinal"].as_str().is_some());
    assert_eq!(
        locks[1].pointer("/accepted_finding/row_ordinal"),
        Some(&locks[1]["ordinal"])
    );

    let components = payload.data["components"]
        .as_array()
        .expect("lock components");
    assert_eq!(components.len(), 1);
    assert_eq!(
        components[0]["edges"],
        json!([
            {"waiter_pid": 20, "blocker_pid": 0},
            {"waiter_pid": 20, "blocker_pid": 10},
        ])
    );
    assert_eq!(
        components[0]["prepared_transactions"][0]["values"]["prepared_count"],
        json!("1")
    );
    assert!(
        payload.data["semantics"]
            .as_array()
            .is_some_and(|semantics| semantics
                .iter()
                .any(|semantic| { semantic.get("source") == Some(&json!("kronika_index")) }))
    );
    assert_eq!(payload.page["truncated"], json!(false));
    assert_eq!(payload.page["stop_reason"], json!("complete"));
}

fn projected_row(pid: i32, blocked_by: &[i32], datname: &str, target: &str) -> Value {
    json!({
        "record": "row",
        "logical_name": "pg_locks",
        "type_id": "1011002",
        "ordinal": pid.to_string(),
        "segment_id": "1",
        "timestamp": "100",
        "values": {
            "pid": pid,
            "blocked_by": blocked_by,
            "datname": datname,
            "lock_target": target,
        },
    })
}

fn prepared_row(datname: &str) -> Value {
    json!({
        "record": "row",
        "logical_name": "pg_prepared_xacts",
        "type_id": "1010001",
        "ordinal": "0",
        "segment_id": "1",
        "timestamp": "100",
        "values": {
            "datname": datname,
            "prepared_count": "1",
            "max_age_us": "2000000",
            "max_xid_age_tx": "17",
        },
    })
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture spells out the recorded lock contract used by the handler"
)]
fn lock_row(
    pid: i32,
    blocked_by: Vec<i32>,
    datname: StrId,
    user: StrId,
    application: StrId,
    backend: StrId,
    active: StrId,
    query: StrId,
    locktype: StrId,
    mode: StrId,
    target: StrId,
) -> PgLocksV2 {
    PgLocksV2 {
        ts: Ts(AT),
        pid,
        blocked_by,
        datid: 16_384,
        datname,
        usename: Some(user),
        application_name: application,
        client_addr: application,
        backend_type: backend,
        state: Some(active),
        wait_event_type: Some(locktype),
        wait_event: Some(mode),
        query,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Some(Ts(AT - 60_000_000)),
        xact_start: Some(Ts(AT - 5_000_000)),
        query_start: Some(Ts(AT - 1_000_000)),
        state_change: Some(Ts(AT - 1_000_000)),
        lock_locktype: Some(locktype),
        lock_mode: Some(mode),
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
        lock_target: Some(target),
        waitstart: Some(Ts(AT - 100_000)),
    }
}

fn label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern fixture label")
            .get(),
    )
}
