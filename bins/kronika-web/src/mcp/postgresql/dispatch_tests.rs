use std::path::Path;
use std::sync::Arc;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::{Environment, InstanceMetadata};
use kronika_registry::pg_locks::PgLocksV2;
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::pg_stat_database::PgStatDatabaseV4;
use kronika_registry::pg_stat_progress_vacuum::PgStatProgressVacuumV2;
use kronika_registry::pg_stat_statements::PgStatStatementsV6;
use kronika_registry::pg_stat_user_indexes::PgStatUserIndexesV2;
use kronika_registry::pg_stat_user_tables::PgStatUserTablesV4;
use kronika_registry::pg_store_plans::PgStorePlansDatasentinelV1;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict};
use rmcp::model::CallToolRequestParams;
use serde_json::{Value, json};
use tokio::sync::Semaphore;

use crate::mcp::{State, tools};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const FROM: i64 = SEGMENT_ID + 10_000_000;
const PRIOR: i64 = SEGMENT_ID + 20_000_000;
const AT: i64 = SEGMENT_ID + 30_000_000;

struct Fixture {
    directory: tempfile::TempDir,
    _writer: WriterOwner,
    journal: Journal,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary PostgreSQL dispatch data root");
        let root = DataRoot::open(directory.path()).expect("open dispatch data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire dispatch fixture writer");
        let mut journal =
            Journal::open(&writer, JournalConfig::default()).expect("open dispatch fixture WAL");
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        append_fixture(&mut journal, address);
        Self {
            directory,
            _writer: writer,
            journal,
        }
    }

    fn root(&self) -> &Path {
        self.directory.path()
    }

    fn state(&self) -> State {
        State {
            data_root: self.root().to_owned(),
            sources: 0b10,
            synthetic_demo: false,
            heavy_scans: Arc::new(Semaphore::new(2)),
        }
    }

    fn append_generation(&mut self) {
        let address = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id"))
            .expect("segment address");
        append_fixture(&mut self.journal, address);
    }
}

#[tokio::test]
async fn continuation_anchor_reports_the_prefix_that_supplied_its_rows() {
    let mut fixture = Fixture::new();
    let first = dispatch(
        &fixture,
        "kronika_find_postgresql_activity",
        json!({
            "at_us": AT.to_string(),
            "include_idle": true,
            "include_system": true,
            "page_size": 1,
        }),
    )
    .await;
    assert_ok(&first, "first Activity page");
    let cursor = first
        .pointer("/page/next_cursor")
        .and_then(Value::as_str)
        .expect("Activity continuation cursor")
        .to_owned();
    let captured = first
        .pointer("/anchor/active_wal_position")
        .and_then(Value::as_str)
        .expect("first Activity source prefix")
        .to_owned();

    fixture.append_generation();
    let second = dispatch(
        &fixture,
        "kronika_find_postgresql_activity",
        json!({
            "at_us": AT.to_string(),
            "include_idle": true,
            "include_system": true,
            "page_size": 1,
            "cursor": cursor,
        }),
    )
    .await;
    assert_ok(&second, "continued Activity page");

    assert_eq!(
        second
            .pointer("/anchor/active_wal_position")
            .and_then(Value::as_str),
        Some(captured.as_str())
    );
}

#[tokio::test]
async fn all_nine_postgresql_tools_execute_through_dispatch_on_one_recorded_fixture() {
    let fixture = Fixture::new();
    let calls = [
        (
            "kronika_get_postgresql_overview",
            json!({"at_us": AT.to_string()}),
            "overview",
        ),
        (
            "kronika_find_postgresql_activity",
            json!({"at_us": AT.to_string()}),
            "activity",
        ),
        (
            "kronika_find_postgresql_locks",
            json!({"at_us": AT.to_string(), "page_size": 2}),
            "locks",
        ),
        (
            "kronika_find_postgresql_vacuum",
            json!({
                "from_us": FROM.to_string(),
                "to_us": AT.to_string(),
                "page_size": 2,
            }),
            "episodes",
        ),
        (
            "kronika_find_postgresql_statements",
            json!({"at_us": AT.to_string(), "lens": "load"}),
            "statements",
        ),
        (
            "kronika_find_postgresql_plans",
            json!({"at_us": AT.to_string(), "lens": "load"}),
            "plans",
        ),
        (
            "kronika_find_postgresql_databases",
            json!({"at_us": AT.to_string()}),
            "databases",
        ),
        (
            "kronika_find_postgresql_tables",
            json!({"at_us": AT.to_string(), "lens": "access"}),
            "tables",
        ),
        (
            "kronika_find_postgresql_indexes",
            json!({"at_us": AT.to_string(), "lens": "usage"}),
            "indexes",
        ),
    ];

    for (name, arguments, key) in calls {
        let response = dispatch(&fixture, name, arguments).await;
        assert_ok(&response, name);
        assert!(
            response
                .pointer(&format!("/data/{key}"))
                .is_some_and(|value| !value.is_null()),
            "{name} did not return its advertised data key: {response}"
        );
        assert!(
            returned(&response) > 0,
            "{name} returned no fixture rows: {response}"
        );
    }
}

#[tokio::test]
async fn activity_dispatch_enforces_flags_semantic_orders_and_rejections() {
    let fixture = Fixture::new();
    for (arguments, expected) in [
        (json!({"at_us": AT.to_string()}), 1),
        (json!({"at_us": AT.to_string(), "include_idle": true}), 2),
        (json!({"at_us": AT.to_string(), "include_system": true}), 2),
        (
            json!({
                "at_us": AT.to_string(),
                "include_idle": true,
                "include_system": true,
                "order": "state_duration_ms",
                "direction": "asc",
            }),
            3,
        ),
    ] {
        let response = dispatch(&fixture, "kronika_find_postgresql_activity", arguments).await;
        assert_ok(&response, "Activity flags");
        assert_eq!(returned(&response), expected, "{response}");
    }

    for (arguments, parameter) in [
        (
            json!({"at_us": AT.to_string(), "include_idle": null}),
            "include_idle",
        ),
        (
            json!({"at_us": AT.to_string(), "order": "not_an_order"}),
            "order",
        ),
        (json!({"at_us": AT.to_string(), "find": "pid:101"}), "find"),
    ] {
        let response = dispatch(&fixture, "kronika_find_postgresql_activity", arguments).await;
        assert_input_error(&response, parameter);
    }
}

#[tokio::test]
async fn statement_lenses_find_and_rejections_run_through_dispatch() {
    let fixture = Fixture::new();
    for lens in ["load", "per_call", "io", "resources", "stability"] {
        let response = dispatch(
            &fixture,
            "kronika_find_postgresql_statements",
            json!({
                "at_us": AT.to_string(),
                "lens": lens,
                "find": "query_id:71",
            }),
        )
        .await;
        assert_ok(&response, lens);
        assert!(returned(&response) > 0, "Statement lens {lens}: {response}");
    }
    for (parameter, value) in [("lens", "unknown"), ("order", "unknown")] {
        let mut arguments = json!({"at_us": AT.to_string()});
        arguments
            .as_object_mut()
            .expect("Statement arguments")
            .insert(parameter.to_owned(), json!(value));
        let response = dispatch(&fixture, "kronika_find_postgresql_statements", arguments).await;
        assert_input_error(&response, parameter);
    }
}

#[tokio::test]
async fn plan_lenses_find_and_rejections_run_through_dispatch() {
    let fixture = Fixture::new();
    for lens in ["load", "timing", "io", "identity"] {
        let response = dispatch(
            &fixture,
            "kronika_find_postgresql_plans",
            json!({
                "at_us": AT.to_string(),
                "lens": lens,
                "find": "plan_id:991",
            }),
        )
        .await;
        assert_ok(&response, lens);
        assert!(returned(&response) > 0, "Plan lens {lens}: {response}");
    }
    for (parameter, value) in [("lens", "unknown"), ("order", "unknown")] {
        let mut arguments = json!({"at_us": AT.to_string()});
        arguments
            .as_object_mut()
            .expect("Plan arguments")
            .insert(parameter.to_owned(), json!(value));
        let response = dispatch(&fixture, "kronika_find_postgresql_plans", arguments).await;
        assert_input_error(&response, parameter);
    }
}

#[tokio::test]
async fn relation_groups_and_lens_defaults_execute_and_orders_are_strict() {
    let fixture = Fixture::new();
    for group in ["object", "database", "schema", "tablespace"] {
        let table = dispatch(
            &fixture,
            "kronika_find_postgresql_tables",
            json!({"at_us": AT.to_string(), "group": group}),
        )
        .await;
        assert_ok(&table, group);
        assert!(returned(&table) > 0, "Table group {group}: {table}");

        let index = dispatch(
            &fixture,
            "kronika_find_postgresql_indexes",
            json!({"at_us": AT.to_string(), "group": group}),
        )
        .await;
        assert_ok(&index, group);
        assert!(returned(&index) > 0, "Index group {group}: {index}");
    }

    for lens in ["access", "changes", "maintenance", "size_buffers", "freeze"] {
        for group in ["object", "database"] {
            let response = dispatch(
                &fixture,
                "kronika_find_postgresql_tables",
                json!({"at_us": AT.to_string(), "group": group, "lens": lens}),
            )
            .await;
            assert_ok(&response, lens);
            assert!(returned(&response) > 0, "Table {group}/{lens}: {response}");
        }
    }
    for lens in ["usage", "low_activity", "size_buffers", "state"] {
        for group in ["object", "database"] {
            let response = dispatch(
                &fixture,
                "kronika_find_postgresql_indexes",
                json!({"at_us": AT.to_string(), "group": group, "lens": lens}),
            )
            .await;
            assert_ok(&response, lens);
            assert!(returned(&response) > 0, "Index {group}/{lens}: {response}");
        }
    }

    for name in [
        "kronika_find_postgresql_tables",
        "kronika_find_postgresql_indexes",
    ] {
        let response = dispatch(
            &fixture,
            name,
            json!({
                "at_us": AT.to_string(),
                "group": "database",
                "order": "not_an_order",
            }),
        )
        .await;
        assert_input_error(&response, "order");
    }
}

#[tokio::test]
async fn locks_vacuum_and_database_reject_advertised_but_unsupported_inputs() {
    let fixture = Fixture::new();
    let locks = dispatch(
        &fixture,
        "kronika_find_postgresql_locks",
        json!({"at_us": AT.to_string(), "page_size": 1}),
    )
    .await;
    assert_error(&locks, "whole_set_bound_exceeded", Some("page_size"));

    for (parameter, value) in [("find", json!("pid:20")), ("cursor", json!("cursor"))] {
        let mut arguments = json!({"at_us": AT.to_string()});
        arguments
            .as_object_mut()
            .expect("Lock arguments")
            .insert(parameter.to_owned(), value);
        let response = dispatch(&fixture, "kronika_find_postgresql_locks", arguments).await;
        assert_input_error(&response, parameter);
    }
    for (parameter, value) in [("find", json!("phase:scan")), ("cursor", json!("cursor"))] {
        let mut arguments = json!({
            "from_us": FROM.to_string(),
            "to_us": AT.to_string(),
        });
        arguments
            .as_object_mut()
            .expect("Vacuum arguments")
            .insert(parameter.to_owned(), value);
        let response = dispatch(&fixture, "kronika_find_postgresql_vacuum", arguments).await;
        assert_input_error(&response, parameter);
    }
    let database_find = dispatch(
        &fixture,
        "kronika_find_postgresql_databases",
        json!({"at_us": AT.to_string(), "find": "database:inventory"}),
    )
    .await;
    assert_input_error(&database_find, "find");
    let database_order = dispatch(
        &fixture,
        "kronika_find_postgresql_databases",
        json!({"at_us": AT.to_string(), "order": "not_an_order"}),
    )
    .await;
    assert_input_error(&database_order, "order");
}

async fn dispatch(fixture: &Fixture, name: &str, arguments: Value) -> Value {
    let mut request = CallToolRequestParams::new(name.to_owned());
    let Value::Object(arguments) = arguments else {
        panic!("{name} test arguments are an object");
    };
    request.arguments = Some(arguments);
    tools::dispatch(fixture.state(), request, || false)
        .await
        .unwrap_or_else(|error| panic!("{name} reached dispatch: {error:?}"))
        .structured_content
        .unwrap_or_else(|| panic!("{name} returned structured content"))
}

fn assert_ok(response: &Value, context: &str) {
    assert_eq!(
        response.get("status"),
        Some(&json!("ok")),
        "{context}: {response}"
    );
}

fn returned(response: &Value) -> u64 {
    response
        .pointer("/page/returned")
        .and_then(Value::as_u64)
        .unwrap_or_else(|| panic!("response has a numeric returned count: {response}"))
}

fn assert_input_error(response: &Value, parameter: &str) {
    assert_error(response, "invalid_input", Some(parameter));
}

fn assert_error(response: &Value, code: &str, parameter: Option<&str>) {
    assert_eq!(
        response.pointer("/status"),
        Some(&json!("error")),
        "{response}"
    );
    assert_eq!(
        response.pointer("/error/code"),
        Some(&json!(code)),
        "{response}"
    );
    match parameter {
        Some(parameter) => assert_eq!(
            response.pointer("/error/parameter"),
            Some(&json!(parameter)),
            "{response}"
        ),
        None => assert_eq!(
            response.pointer("/error/parameter"),
            Some(&Value::Null),
            "{response}"
        ),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "one fixture visibly records every PostgreSQL MCP source in one atomic WAL part"
)]
fn append_fixture(journal: &mut Journal, address: SegmentAddress) {
    let mut interner = Interner::new(DictLimits::default());
    let inventory = label(&mut interner, "inventory");
    let public = label(&mut interner, "public");
    let items = label(&mut interner, "items");
    let items_index = label(&mut interner, "items_pkey");
    let tablespace = label(&mut interner, "pg_default");
    let btree = label(&mut interner, "btree");
    let indexdef = label(&mut interner, "CREATE UNIQUE INDEX items_pkey ON items(id)");
    let role = label(&mut interner, "app");
    let application = label(&mut interner, "fixture");
    let client = label(&mut interner, "127.0.0.1");
    let client_backend = label(&mut interner, "client backend");
    let system_backend = label(&mut interner, "autovacuum worker");
    let active = label(&mut interner, "active");
    let idle = label(&mut interner, "idle");
    let query = label(&mut interner, "select * from items where id = 1");
    let idle_query = label(&mut interner, "select 1");
    let lock_target = label(&mut interner, "transaction 42");
    let locktype = label(&mut interner, "transactionid");
    let lockmode = label(&mut interner, "ShareLock");
    let statement_text = label(&mut interner, "select * from items");
    let plan_text = label(&mut interner, "Seq Scan on items");
    let relids = label(&mut interner, "{20000}");
    let command = label(&mut interner, "SELECT");
    let phase = label(&mut interner, "scanning heap");
    let hostname = label(&mut interner, "db-01");
    let kernel = label(&mut interner, "6.12");
    let boot = label(&mut interner, "fixture-boot");

    let mut buffers = SectionBuffers::new();
    buffers
        .push(InstanceMetadata {
            ts: Ts(SEGMENT_ID + 1),
            hostname,
            kernel_version: kernel,
            environment: Environment::Machine.as_u8(),
            clock_ticks_per_sec: 100,
            page_size_bytes: 4096,
            boot_id: boot,
            btime: Ts(SEGMENT_ID - 1_000_000),
            postgresql_enabled: true,
            postgresql_interval_seconds: 10,
            postgresql_effective_cpus: Some(4),
        })
        .expect("metadata row fits");
    for row in [
        activity_row(
            101,
            client_backend,
            Some(active),
            Some(query),
            application,
            client,
        ),
        activity_row(
            102,
            client_backend,
            Some(idle),
            Some(idle_query),
            application,
            client,
        ),
        activity_row(103, system_backend, None, None, application, client),
    ] {
        buffers.push(row).expect("Activity row fits");
    }
    for (timestamp, step) in [(PRIOR, 1), (AT, 2)] {
        buffers
            .push(statement_row(
                timestamp,
                step,
                inventory,
                role,
                statement_text,
            ))
            .expect("Statement row fits");
        buffers
            .push(plan_row(
                timestamp, step, inventory, role, plan_text, relids, command,
            ))
            .expect("Plan row fits");
        buffers
            .push(database_row(timestamp, step, inventory))
            .expect("Database row fits");
        buffers
            .push(table_row(
                timestamp, step, inventory, public, items, tablespace,
            ))
            .expect("Table row fits");
        buffers
            .push(index_row(
                timestamp,
                inventory,
                public,
                items,
                items_index,
                tablespace,
                btree,
                indexdef,
            ))
            .expect("Index row fits");
    }
    for row in [
        lock_row(
            10,
            Vec::new(),
            inventory,
            role,
            application,
            client,
            client_backend,
            active,
            query,
            locktype,
            lockmode,
            lock_target,
        ),
        lock_row(
            20,
            vec![0, 10],
            inventory,
            role,
            application,
            client,
            client_backend,
            active,
            query,
            locktype,
            lockmode,
            lock_target,
        ),
    ] {
        buffers.push(row).expect("Lock row fits");
    }
    for (timestamp, scanned) in [(FROM, 100), (PRIOR, 200), (AT, 300)] {
        buffers
            .push(vacuum_row(
                timestamp, scanned, inventory, public, items, phase,
            ))
            .expect("Vacuum row fits");
    }

    let dictionary = dict::encode(interner.window()).expect("encode dispatch dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode dispatch fixture")
        .expect("nonempty dispatch fixture");
    journal
        .append(address.id, &part)
        .expect("append dispatch fixture");
}

fn activity_row(
    pid: i32,
    backend_type: StrId,
    state: Option<StrId>,
    query: Option<StrId>,
    application: StrId,
    client: StrId,
) -> PgStatActivityV3 {
    PgStatActivityV3 {
        ts: Ts(AT),
        pid,
        leader_pid: None,
        datid: Some(16_384),
        datname: None,
        usename: None,
        application_name: application,
        client_addr: client,
        backend_type,
        state,
        wait_event_type: None,
        wait_event: None,
        query,
        query_id: Some(71),
        backend_xid_age: Some(10),
        backend_xmin_age: Some(20),
        backend_start: Ts(AT - 60_000_000),
        xact_start: state.is_some().then_some(Ts(AT - 10_000_000)),
        query_start: (state.is_some() && query.is_some()).then_some(Ts(AT - 5_000_000)),
        state_change: state.map(|_| Ts(AT - 2_000_000)),
    }
}

fn statement_row(
    timestamp: i64,
    step: i64,
    datname: StrId,
    usename: StrId,
    query: StrId,
) -> PgStatStatementsV6 {
    let factor = f64::from(i32::try_from(step).expect("small Statement fixture multiplier"));
    PgStatStatementsV6 {
        ts: Ts(timestamp),
        queryid: Some(71),
        userid: 72,
        dbid: 16_384,
        toplevel: true,
        datname: Some(datname),
        usename: Some(usename),
        query: Some(query),
        calls: 100 * step,
        rows: 5_000 * step,
        plans: 90 * step,
        total_exec_time: 1_234.5 * factor,
        total_plan_time: 12.5 * factor,
        min_exec_time: 0.5,
        max_exec_time: 40.0,
        mean_exec_time: 12.3,
        stddev_exec_time: 3.1,
        min_plan_time: 0.1,
        max_plan_time: 1.0,
        mean_plan_time: 0.2,
        stddev_plan_time: 0.05,
        shared_blks_hit: 90_000 * step,
        shared_blks_read: 4_000 * step,
        shared_blks_dirtied: 50 * step,
        shared_blks_written: 30 * step,
        local_blks_hit: 8 * step,
        local_blks_read: 4 * step,
        local_blks_dirtied: 2 * step,
        local_blks_written: step,
        temp_blks_read: 3 * step,
        temp_blks_written: 2 * step,
        shared_blk_read_time: 12.5 * factor,
        shared_blk_write_time: 3.0 * factor,
        local_blk_read_time: factor,
        local_blk_write_time: 0.5 * factor,
        temp_blk_read_time: 0.25 * factor,
        temp_blk_write_time: 0.125 * factor,
        wal_records: 42 * step,
        wal_fpi: 3 * step,
        wal_bytes: 8_192 * step,
        wal_buffers_full: step,
        jit_functions: 0,
        jit_generation_time: 0.0,
        jit_inlining_count: 0,
        jit_inlining_time: 0.0,
        jit_optimization_count: 0,
        jit_optimization_time: 0.0,
        jit_emission_count: 0,
        jit_emission_time: 0.0,
        jit_deform_count: 0,
        jit_deform_time: 0.0,
        parallel_workers_to_launch: 4 * step,
        parallel_workers_launched: 3 * step,
        stats_since: Ts(FROM),
        minmax_stats_since: Ts(FROM),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture spells out the recorded Plan labels"
)]
fn plan_row(
    timestamp: i64,
    step: i64,
    datname: StrId,
    usename: StrId,
    plan: StrId,
    relids: StrId,
    cmd_type: StrId,
) -> PgStorePlansDatasentinelV1 {
    let factor = f64::from(i32::try_from(step).expect("small Plan fixture multiplier"));
    PgStorePlansDatasentinelV1 {
        ts: Ts(timestamp),
        queryid: 71,
        planid: 991,
        userid: 72,
        dbid: 16_384,
        datname: Some(datname),
        usename: Some(usename),
        plan: Some(plan),
        relids: Some(relids),
        cmd_type: Some(cmd_type),
        calls: 10 * step,
        total_time: 99.5 * factor,
        min_time: 1.0,
        max_time: 50.0,
        mean_time: 24.9,
        stddev_time: 2.2,
        rows: 40 * step,
        shared_blks_hit: step,
        shared_blks_read: 2 * step,
        shared_blks_dirtied: 3 * step,
        shared_blks_written: 4 * step,
        local_blks_hit: 5 * step,
        local_blks_read: 6 * step,
        local_blks_dirtied: 7 * step,
        local_blks_written: 8 * step,
        temp_blks_read: 9 * step,
        temp_blks_written: 10 * step,
        shared_blk_read_time: 1.5 * factor,
        shared_blk_write_time: 2.5 * factor,
        local_blk_read_time: 3.5 * factor,
        local_blk_write_time: 4.5 * factor,
        temp_blk_read_time: 5.5 * factor,
        temp_blk_write_time: 6.5 * factor,
        first_call: Some(Ts(FROM)),
        last_call: Some(Ts(timestamp - 1)),
    }
}

fn database_row(timestamp: i64, step: i64, datname: StrId) -> PgStatDatabaseV4 {
    let factor = f64::from(i32::try_from(step).expect("small Database fixture multiplier"));
    PgStatDatabaseV4 {
        ts: Ts(timestamp),
        datid: 16_384,
        datname: Some(datname),
        numbackends: Some(3),
        xact_commit: 100 * step,
        xact_rollback: 2 * step,
        blks_read: 4_000 * step,
        blks_hit: 90_000 * step,
        tup_returned: 500 * step,
        tup_fetched: 400 * step,
        tup_inserted: 50 * step,
        tup_updated: 30 * step,
        tup_deleted: 10 * step,
        conflicts: 0,
        temp_files: step,
        temp_bytes: 8_192 * step,
        deadlocks: 0,
        blk_read_time: 12.5 * factor,
        blk_write_time: 3.0 * factor,
        stats_reset: Some(Ts(FROM)),
        frozen_xid_age: Some(150_000_000),
        min_mxid_age: Some(5_000_000),
        datconnlimit: Some(-1),
        datallowconn: Some(true),
        datistemplate: Some(false),
        checksum_failures: Some(0),
        checksum_last_failure: None,
        session_time: 1_000.0 * factor,
        active_time: 250.0 * factor,
        idle_in_transaction_time: 50.0 * factor,
        sessions: 7 * step,
        sessions_abandoned: step,
        sessions_fatal: 0,
        sessions_killed: 0,
        parallel_workers_to_launch: 9 * step,
        parallel_workers_launched: 8 * step,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture spells out the recorded Table labels"
)]
fn table_row(
    timestamp: i64,
    step: i64,
    datname: StrId,
    schemaname: StrId,
    relname: StrId,
    tablespace: StrId,
) -> PgStatUserTablesV4 {
    let factor = f64::from(i32::try_from(step).expect("small Table fixture multiplier"));
    PgStatUserTablesV4 {
        ts: Ts(timestamp),
        datid: 16_384,
        datname,
        relid: 20_000,
        schemaname,
        relname,
        tablespace_oid: Some(1_663),
        tablespace: Some(tablespace),
        seq_scan: 10 * step,
        seq_tup_read: 1_000 * step,
        idx_scan: Some(120 * step),
        idx_tup_fetch: Some(3_000 * step),
        n_tup_ins: 50 * step,
        n_tup_upd: 30 * step,
        n_tup_del: 10 * step,
        n_tup_hot_upd: 5 * step,
        n_tup_newpage_upd: step,
        n_live_tup: 900,
        n_dead_tup: 40,
        n_mod_since_analyze: 70,
        n_ins_since_vacuum: 20,
        vacuum_count: step,
        autovacuum_count: 3 * step,
        analyze_count: step,
        autoanalyze_count: 2 * step,
        last_vacuum: Some(Ts(timestamp - 10)),
        last_autovacuum: Some(Ts(timestamp - 9)),
        last_analyze: Some(Ts(timestamp - 8)),
        last_autoanalyze: Some(Ts(timestamp - 7)),
        last_seq_scan: Some(Ts(timestamp - 6)),
        last_idx_scan: Some(Ts(timestamp - 5)),
        total_vacuum_time: 12.5 * factor,
        total_autovacuum_time: 340.0 * factor,
        total_analyze_time: 7.5 * factor,
        total_autoanalyze_time: 21.0 * factor,
        main_fork_bytes: 8_192,
        toast_bytes: Some(4_096),
        toast_n_live_tup: Some(100),
        toast_n_dead_tup: Some(10),
        toast_last_autovacuum: Some(Ts(timestamp - 4)),
        xid_age: Some(100_000_000),
        mxid_age: Some(5_000_000),
        reltuples: 900,
        heap_blks_read: 400 * step,
        heap_blks_hit: 90_000 * step,
        idx_blks_read: Some(40 * step),
        idx_blks_hit: Some(9_000 * step),
        toast_blks_read: Some(4 * step),
        toast_blks_hit: Some(900 * step),
        tidx_blks_read: Some(2 * step),
        tidx_blks_hit: Some(450 * step),
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture spells out the recorded Index labels"
)]
fn index_row(
    timestamp: i64,
    datname: StrId,
    schemaname: StrId,
    relname: StrId,
    indexrelname: StrId,
    tablespace: StrId,
    amname: StrId,
    indexdef: StrId,
) -> PgStatUserIndexesV2 {
    PgStatUserIndexesV2 {
        ts: Ts(timestamp),
        datid: 16_384,
        datname,
        indexrelid: 20_001,
        relid: 20_000,
        schemaname,
        relname,
        indexrelname,
        tablespace_oid: 1_663,
        tablespace: Some(tablespace),
        idx_scan: 0,
        idx_tup_read: 0,
        idx_tup_fetch: 0,
        main_fork_bytes: 16_384,
        last_idx_scan: None,
        indisunique: true,
        indisprimary: true,
        indisvalid: true,
        indisexclusion: false,
        indisready: true,
        amname,
        indexdef: Some(indexdef),
        idx_blks_read: 40,
        idx_blks_hit: 9_000,
    }
}

#[allow(
    clippy::too_many_arguments,
    reason = "the fixture spells out the recorded Lock graph"
)]
fn lock_row(
    pid: i32,
    blocked_by: Vec<i32>,
    datname: StrId,
    usename: StrId,
    application_name: StrId,
    client_addr: StrId,
    backend_type: StrId,
    state: StrId,
    query: StrId,
    lock_locktype: StrId,
    lock_mode: StrId,
    lock_target: StrId,
) -> PgLocksV2 {
    PgLocksV2 {
        ts: Ts(AT),
        pid,
        blocked_by,
        datid: 16_384,
        datname,
        usename: Some(usename),
        application_name,
        client_addr,
        backend_type,
        state: Some(state),
        wait_event_type: Some(lock_locktype),
        wait_event: Some(lock_mode),
        query,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Some(Ts(AT - 60_000_000)),
        xact_start: Some(Ts(AT - 5_000_000)),
        query_start: Some(Ts(AT - 1_000_000)),
        state_change: Some(Ts(AT - 1_000_000)),
        lock_locktype: Some(lock_locktype),
        lock_mode: Some(lock_mode),
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
        lock_target: Some(lock_target),
        waitstart: Some(Ts(AT - 100_000)),
    }
}

fn vacuum_row(
    timestamp: i64,
    scanned: i64,
    datname: StrId,
    schemaname: StrId,
    relname: StrId,
    phase: StrId,
) -> PgStatProgressVacuumV2 {
    PgStatProgressVacuumV2 {
        ts: Ts(timestamp),
        pid: 200,
        datid: 16_384,
        datname,
        relid: 20_000,
        schemaname: Some(schemaname),
        relname: Some(relname),
        is_autovacuum: true,
        phase,
        heap_blks_total: 1_000,
        heap_blks_scanned: scanned,
        heap_blks_vacuumed: 50,
        index_vacuum_count: 1,
        max_dead_tuple_bytes: 67_108_864,
        dead_tuple_bytes: 8_192,
        num_dead_item_ids: 512,
        indexes_total: 2,
        indexes_processed: 1,
    }
}

fn label(interner: &mut Interner, value: &str) -> StrId {
    StrId(
        interner
            .intern(value.as_bytes())
            .expect("intern dispatch fixture label")
            .get(),
    )
}
