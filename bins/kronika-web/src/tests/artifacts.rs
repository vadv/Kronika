use std::cell::Cell;
use std::io::Read as _;
use std::path::Path;

use flate2::read::GzDecoder;
use http_body_util::BodyExt as _;
use hyper::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, ETAG, HeaderValue, VARY};
use hyper::{HeaderMap, StatusCode};
use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::InstanceMetadata;
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_netdev::OsNetdev;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::pg_log::PgLogErrors;
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::Value;

use crate::api::{ApiError, CachePolicy, Prepared};
use crate::encoding::AcceptedEncodings;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SOURCES: u32 = 0b11;

struct Fixture {
    directory: tempfile::TempDir,
    writer: WriterOwner,
    journal: Journal,
    address: SegmentAddress,
}

impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().expect("temporary data root");
        let root = DataRoot::open(directory.path()).expect("open data root");
        let writer = root
            .acquire_writer(LayoutLimits::default())
            .expect("acquire writer");
        let journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");
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

    fn position(&self) -> u64 {
        u64::try_from(self.journal.bytes()).expect("journal length fits u64")
    }

    fn append_diskstats(&mut self, rows: &[(i64, i32, i64)]) {
        let mut buffers = SectionBuffers::new();
        for &(ts, minor, reads) in rows {
            buffers
                .push(diskstats(ts, minor, reads))
                .expect("diskstats row fits");
        }
        self.append(buffers);
    }

    fn append_blob_diskstats(&mut self, bytes: &[u8], reads: i64) {
        let mut interner =
            Interner::new(DictLimits::new(8, 16).expect("fixture dictionary limits"));
        let device = StrId(interner.intern(bytes).expect("intern blob").get());
        let dictionary = dict::encode(interner.window()).expect("encode blob dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(diskstats_with_device(100, 0, reads, device))
            .expect("diskstats row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode blob fixture part")
            .expect("nonempty blob fixture part");
        self.journal
            .append(self.address.id, &part)
            .expect("append blob fixture part");
    }

    fn append_named_then_unreadable_diskstats(&mut self, valid_rows: i32) {
        let mut interner = Interner::new(DictLimits::default());
        let device = StrId(interner.intern(b"nvme0n1").expect("intern device").get());
        let dictionary = dict::encode(interner.window()).expect("encode device dictionary");
        let mut buffers = SectionBuffers::new();
        for minor in 0..valid_rows {
            buffers
                .push(diskstats_with_device(100, minor, i64::from(minor), device))
                .expect("valid row fits");
        }
        buffers
            .push(diskstats_with_device(
                100,
                valid_rows,
                i64::from(valid_rows),
                StrId(999_999),
            ))
            .expect("unreadable row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode unreadable fixture")
            .expect("nonempty unreadable fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append unreadable fixture");
    }

    fn append_ranked_diskstats_with_unreadable_loser(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let device = StrId(interner.intern(b"nvme0n1").expect("intern device").get());
        let dictionary = dict::encode(interner.window()).expect("encode device dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(diskstats_with_device(100, -1, 1, StrId(999_999)))
            .expect("unreadable row fits");
        buffers
            .push(diskstats_with_device(100, 2, 2, device))
            .expect("ranked row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked fixture")
            .expect("nonempty ranked fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked fixture");
    }

    fn append_health(&mut self) {
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(100),
                hostname: StrId(901),
                kernel_version: StrId(902),
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: StrId(903),
                btime: Ts(1),
                postgresql_enabled: false,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: None,
            })
            .expect("metadata row fits");
        for row in [
            psi(100, 0, 0),
            psi(100, 1, 0),
            psi(100, 2, 0),
            psi(200, 0, 50),
            psi(200, 1, 20),
            psi(200, 2, 0),
        ] {
            buffers.push(row).expect("psi row fits");
        }
        self.append(buffers);
    }

    fn append_postgres_health(&mut self, active: u32) {
        self.append_postgres_health_at(100, active);
    }

    fn append_postgres_health_at(&mut self, at: i64, active: u32) {
        let mut interner = Interner::new(DictLimits::default());
        let active_state = StrId(interner.intern(b"active").expect("active state").get());
        let idle_state = StrId(interner.intern(b"idle").expect("idle state").get());
        let query = StrId(
            interner
                .intern(b"QUERY-TEXT-MUST-STAY-OUT-OF-IDX")
                .expect("query text")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("health dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(at),
                hostname: StrId(901),
                kernel_version: StrId(902),
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: StrId(903),
                btime: Ts(1),
                postgresql_enabled: true,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: Some(2),
            })
            .expect("metadata row fits");
        for row in [
            psi(at, 0, 0),
            psi(at, 1, 0),
            psi(at, 2, 0),
            psi(at + 100, 0, 50),
            psi(at + 100, 1, 20),
            psi(at + 100, 2, 0),
        ] {
            buffers.push(row).expect("psi row fits");
        }
        for pid in 0..active {
            buffers
                .push(activity(
                    at + 50,
                    i32::try_from(pid).expect("fixture pid"),
                    active_state,
                    query,
                ))
                .expect("active row fits");
        }
        buffers
            .push(activity(at + 50, 10_000, idle_state, query))
            .expect("idle row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode health fixture")
            .expect("nonempty health fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append health fixture");
    }

    fn append_finding_rows(
        &mut self,
        process_rows: &[(i64, Option<i64>)],
        statement_rows: &[(i64, i64, f64)],
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(interner.intern(b"fixture").expect("intern label").get());
        let dictionary = dict::encode(interner.window()).expect("finding dictionary");
        let mut buffers = SectionBuffers::new();
        for &(ts, read_bytes) in process_rows {
            buffers
                .push(process(ts, read_bytes, label))
                .expect("process row fits");
        }
        for &(ts, calls, total_exec_time) in statement_rows {
            buffers
                .push(statement(ts, calls, total_exec_time, label))
                .expect("statement row fits");
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode finding rows")
            .expect("nonempty finding rows");
        self.journal
            .append(self.address.id, &part)
            .expect("append finding rows");
    }

    fn append_log_error(&mut self, at: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(interner.intern(b"fixture").expect("intern label").get());
        let dictionary = dict::encode(interner.window()).expect("log dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgLogErrors {
                ts: Ts(at),
                system_identifier: None,
                source_file: label,
                severity: 0,
                category: 8,
                sqlstate: None,
                pattern: label,
                count: 1,
                sample: label,
                detail: None,
                hint: None,
                context: None,
                statement: None,
                database: None,
                username: None,
            })
            .expect("log row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode log row")
            .expect("nonempty log row");
        self.journal
            .append(self.address.id, &part)
            .expect("append log row");
    }

    fn finish_and_continue(&mut self, segment_id: i64) {
        write_segment(&self.journal, &self.writer, self.address).expect("finish segment");
        self.journal.reset().expect("reset journal");
        self.address = SegmentAddress::new(SegmentId::new(segment_id).expect("segment id"))
            .expect("segment address");
    }

    fn append(&mut self, mut buffers: SectionBuffers) {
        let part = buffers
            .flush(&[])
            .expect("encode fixture part")
            .expect("nonempty fixture part");
        self.journal
            .append(self.address.id, &part)
            .expect("append fixture part");
    }

    fn finish(&self) {
        write_segment(&self.journal, &self.writer, self.address).expect("finish segment");
    }

    fn prepare(&self, target: &str, if_none_match: Option<&str>) -> Prepared {
        let (path, query) = target
            .split_once('?')
            .map_or((target, None), |(path, query)| (path, Some(query)));
        let route = crate::route::parse(path, query).expect("valid fixture route");
        crate::api::prepare(self.root(), SOURCES, route, if_none_match)
            .expect("prepare fixture resource")
    }
}

fn diskstats(ts: i64, minor: i32, reads: i64) -> OsDiskstats {
    diskstats_with_device(ts, minor, reads, StrId(999))
}

fn diskstats_with_device(ts: i64, minor: i32, reads: i64, device: StrId) -> OsDiskstats {
    OsDiskstats {
        ts: Ts(ts),
        major: 8,
        minor,
        device,
        reads,
        r_merged: 0,
        read_sectors: reads,
        read_time_ms: 0,
        writes: 0,
        w_merged: 0,
        write_sectors: 0,
        write_time_ms: 0,
        io_in_progress: 0,
        io_time_ms: 0,
        io_weighted_time_ms: 0,
        discards: None,
        d_merged: None,
        discard_sectors: None,
        discard_time_ms: None,
        flushes: None,
        flush_time_ms: None,
        scope: 0,
    }
}

fn netdev(ts: i64, iface: StrId, rx_bytes: i64) -> OsNetdev {
    OsNetdev {
        ts: Ts(ts),
        iface,
        rx_bytes,
        rx_packets: 0,
        rx_errs: 0,
        rx_drop: 0,
        rx_fifo: 0,
        rx_frame: 0,
        rx_compressed: 0,
        rx_multicast: 0,
        tx_bytes: 0,
        tx_packets: 0,
        tx_errs: 0,
        tx_drop: 0,
        tx_fifo: 0,
        tx_colls: 0,
        tx_carrier: 0,
        tx_compressed: 0,
        speed_mbit: None,
        duplex: 0,
        scope: 0,
    }
}

fn process(ts: i64, read_bytes: Option<i64>, label: StrId) -> OsProcess {
    OsProcess {
        ts: Ts(ts),
        pid: 41,
        starttime: Ts(SEGMENT_ID - 1_000_000),
        ppid: 1,
        uid: 1_000,
        euid: 1_000,
        gid: 1_000,
        egid: 1_000,
        state: b'S',
        num_threads: 1,
        tty: 0,
        comm: label,
        cmdline: Some(label),
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
        read_bytes,
        write_bytes: None,
        cancelled_write_bytes: None,
        exit_signal: 17,
        scope: 0,
    }
}

fn statement(ts: i64, calls: i64, total_exec_time: f64, label: StrId) -> PgStatStatementsV2 {
    PgStatStatementsV2 {
        ts: Ts(ts),
        queryid: Some(71),
        userid: 72,
        dbid: 73,
        datname: None,
        usename: None,
        query: Some(label),
        calls,
        rows: 0,
        plans: 0,
        total_exec_time,
        total_plan_time: 0.0,
        min_exec_time: 0.0,
        max_exec_time: 0.0,
        mean_exec_time: 0.0,
        stddev_exec_time: 0.0,
        min_plan_time: 0.0,
        max_plan_time: 0.0,
        mean_plan_time: 0.0,
        stddev_plan_time: 0.0,
        shared_blks_hit: 0,
        shared_blks_read: 0,
        shared_blks_dirtied: 0,
        shared_blks_written: 0,
        local_blks_hit: 0,
        local_blks_read: 0,
        local_blks_dirtied: 0,
        local_blks_written: 0,
        temp_blks_read: 0,
        temp_blks_written: 0,
        blk_read_time: 0.0,
        blk_write_time: 0.0,
        wal_records: 0,
        wal_fpi: 0,
        wal_bytes: 0,
    }
}

#[test]
fn real_rows_keep_js_unsafe_integers_and_truncated_blob_metadata_lossless() {
    let mut fixture = Fixture::new();
    fixture.append_blob_diskstats(&[0xff; 17], i64::MAX);

    let records = stream(fixture.prepare(&target("history", "field=device&field=reads"), None))
        .expect("blob history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][1], i64::MAX.to_string());
    let blob = &rows[0]["values"][0];
    assert_eq!(blob["representation"], "bytes");
    assert_eq!(blob["full_len"], "17");
    assert_eq!(blob["truncated"], true);
    assert_eq!(
        blob["stored_base64"].as_str().map(str::len),
        Some(24),
        "all 16 stored bytes survive in base64"
    );
    assert_eq!(
        blob["sha256"].as_str().map(str::len),
        Some(64),
        "the full-value SHA-256 survives"
    );
}

#[test]
fn cancellation_stops_a_history_scan_before_dictionary_work() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    let prepared = fixture.prepare(&target("history", "field=reads"), None);
    let checks = Cell::new(0_u8);
    let mut records = Vec::new();

    prepared
        .stream(
            &mut |record| {
                records.push(record);
                true
            },
            &|| {
                let next = checks.get().saturating_add(1);
                checks.set(next);
                next >= 3
            },
        )
        .expect("cancelled scan exits normally");

    assert_eq!(records.len(), 2, "only the header and layout are emitted");
    assert!(
        records
            .iter()
            .all(|record| !record.starts_with(b"{\"record\":\"row\""))
    );
}

fn psi(ts: i64, resource: u8, some_total: i64) -> OsPsi {
    OsPsi {
        ts: Ts(ts),
        resource,
        some_avg10: 0.0,
        some_avg60: 0.0,
        some_avg300: 0.0,
        some_total,
        full_avg10: None,
        full_avg60: None,
        full_avg300: None,
        full_total: None,
        scope: 0,
    }
}

fn activity(ts: i64, pid: i32, state: StrId, query: StrId) -> PgStatActivityV3 {
    PgStatActivityV3 {
        ts: Ts(ts),
        pid,
        leader_pid: None,
        datname: None,
        usename: None,
        application_name: state,
        client_addr: state,
        backend_type: state,
        state: Some(state),
        wait_event_type: Some(state),
        wait_event: Some(state),
        query: Some(query),
        query_id: None,
        backend_xid_age: None,
        backend_xmin_age: None,
        backend_start: Ts(1),
        xact_start: None,
        query_start: None,
        state_change: None,
    }
}

fn stream(prepared: Prepared) -> Result<Vec<Value>, ApiError> {
    let mut records = Vec::new();
    prepared.stream(
        &mut |record| {
            records.push(record);
            true
        },
        &|| false,
    )?;
    Ok(records
        .iter()
        .map(|record| serde_json::from_slice(record).expect("JSON record"))
        .collect())
}

fn row_records(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect()
}

fn target(resource: &str, query: &str) -> String {
    format!("/api/segments/{SEGMENT_ID}/sections/os_diskstats/{resource}?{query}")
}

fn accepted(value: &str) -> AcceptedEncodings {
    let mut headers = HeaderMap::new();
    headers.insert(
        ACCEPT_ENCODING,
        HeaderValue::from_str(value).expect("valid Accept-Encoding"),
    );
    AcceptedEncodings::from_headers(&headers).expect("acceptable representation")
}

#[tokio::test]
async fn large_ndjson_gzip_round_trips_to_the_identity_representation() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..256)
            .map(|minor| (100, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    let resource = target("history", "field=reads");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), AcceptedEncodings::default()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    assert_eq!(
        response.headers().get(VARY),
        Some(&HeaderValue::from_static("Authorization, Accept-Encoding"))
    );
    let mut compressed = Vec::new();
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        let bytes = frame
            .expect("gzip body frame")
            .into_data()
            .expect("data frame");
        assert!(bytes.len() <= 8 * 1_024, "bounded gzip frame");
        compressed.extend_from_slice(&bytes);
    }
    assert_eq!(compressed.get(4..8), Some([0, 0, 0, 0].as_slice()));
    let mut decoded = Vec::new();
    GzDecoder::new(compressed.as_slice())
        .read_to_end(&mut decoded)
        .expect("decode response gzip");

    let prepared = fixture.prepare(&resource, None);
    let identity = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    assert_eq!(identity.status(), StatusCode::OK);
    assert!(!identity.headers().contains_key(CONTENT_ENCODING));
    let identity = identity
        .into_body()
        .collect()
        .await
        .expect("identity body")
        .to_bytes();
    assert!(identity.len() > 8 * 1_024, "fixture crosses the threshold");
    assert_eq!(decoded, identity);
}

#[tokio::test]
async fn below_threshold_ndjson_stays_identity_when_it_is_allowed() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    let prepared = fixture.prepare(&target("history", "field=reads"), None);
    let response = crate::blocking_stream(move || Ok(prepared), AcceptedEncodings::default()).await;
    assert_eq!(response.status(), StatusCode::OK);
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("small identity body")
        .to_bytes();
    assert!(body.len() < 8 * 1_024);
    let first = body
        .split(|byte| *byte == b'\n')
        .next()
        .expect("history header");
    assert_eq!(
        serde_json::from_slice::<Value>(first).expect("history JSON")["record"],
        "history"
    );

    let prepared = fixture.prepare(&target("history", "field=reads"), None);
    let response =
        crate::blocking_stream(move || Ok(prepared), accepted("gzip, identity;q=0")).await;
    assert_eq!(
        response.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    let compressed = response
        .into_body()
        .collect()
        .await
        .expect("forced gzip body")
        .to_bytes();
    let mut decoded = Vec::new();
    GzDecoder::new(compressed.as_ref())
        .read_to_end(&mut decoded)
        .expect("decode forced gzip");
    assert_eq!(decoded, body);
}

#[tokio::test]
async fn a_small_real_read_failure_returns_500_before_success_headers() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);
    let prepared = fixture.prepare(&target("history", "field=device"), None);
    let response = crate::blocking_stream(move || Ok(prepared), AcceptedEncodings::default()).await;
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    assert_eq!(
        response.headers().get(CONTENT_TYPE),
        Some(&HeaderValue::from_static("application/json"))
    );
    assert!(!response.headers().contains_key(CONTENT_ENCODING));
    let body = response
        .into_body()
        .collect()
        .await
        .expect("ordinary error body")
        .to_bytes();
    assert_eq!(
        serde_json::from_slice::<Value>(&body).expect("error JSON"),
        serde_json::json!({"error": "unreadable"})
    );
}

#[tokio::test]
async fn a_real_read_failure_after_a_valid_prefix_fails_the_body_without_a_trailer() {
    let mut fixture = Fixture::new();
    fixture.append_named_then_unreadable_diskstats(160);
    let resource = target("history", "field=device");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let error = response
        .into_body()
        .collect()
        .await
        .expect_err("late read failure reaches the body");
    assert_eq!(error.to_string(), "response body failed");

    let prepared = fixture.prepare(&resource, None);
    let response =
        crate::blocking_stream(move || Ok(prepared), accepted("gzip, identity;q=0")).await;
    assert_eq!(
        response.headers().get(CONTENT_ENCODING),
        Some(&HeaderValue::from_static("gzip"))
    );
    response
        .into_body()
        .collect()
        .await
        .expect_err("late read failure also aborts gzip");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    let mut prefix = Vec::new();
    let mut failure = None;
    let mut body = response.into_body();
    while let Some(frame) = body.frame().await {
        match frame {
            Ok(frame) => prefix.extend_from_slice(&frame.into_data().expect("data frame")),
            Err(error) => {
                failure = Some(error);
                break;
            }
        }
    }
    assert!(failure.is_some(), "body ends with an error");
    assert!(prefix.len() >= 8 * 1_024, "a valid prefix was committed");
    assert!(
        !prefix
            .windows(b"\"record\":\"error\"".len())
            .any(|window| { window == b"\"record\":\"error\"" })
    );
    for record in prefix
        .split(|byte| *byte == b'\n')
        .filter(|line| !line.is_empty())
    {
        serde_json::from_slice::<Value>(record).expect("prefix contains complete NDJSON records");
    }
}

#[test]
fn explicit_history_projection_keeps_coordinates_without_resolving_unused_text() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7)]);

    let projected = fixture.prepare(&target("history", "field=reads"), None);
    assert_eq!(projected.meta().cache, CachePolicy::NoStore);
    let records = stream(projected).expect("projected history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["timestamp"], "100");
    assert_eq!(rows[0]["identity"], serde_json::json!([8, 0]));
    assert_eq!(rows[0]["values"], serde_json::json!(["7"]));

    let requested_text = fixture.prepare(&target("history", "field=device"), None);
    let error = requested_text
        .stream(&mut |_record| true, &|| false)
        .expect_err("requested corrupt dictionary id must fail");
    assert!(matches!(error, ApiError::Unreadable(_)));
    assert!(error.to_string().contains("unresolved dictionary id 999"));

    let requested_filter =
        fixture.prepare(&target("history", "field=reads&where.device=sda"), None);
    let error = requested_filter
        .stream(&mut |_record| true, &|| false)
        .expect_err("corrupt dictionary id in a filter must fail");
    assert!(matches!(error, ApiError::Unreadable(_)));
    assert!(error.to_string().contains("unresolved dictionary id 999"));
}

#[test]
fn active_pages_pin_valid_len_and_preserve_equal_timestamp_ordinals() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 10), (100, 1, 11)]);
    let first_position = fixture.position();

    let first = stream(fixture.prepare(&target("rows", "field=reads&page_size=1&order=asc"), None))
        .expect("first page");
    assert_eq!(
        first[0]["segment"]["cursor"]["wal_position"],
        first_position.to_string()
    );
    let first_rows = row_records(&first);
    assert_eq!(first_rows.len(), 1);
    assert_eq!(first_rows[0]["ordinal"], "0");
    let cursor = first
        .iter()
        .find(|record| record["record"] == "page")
        .and_then(|record| record["next_cursor"].as_str())
        .expect("page cursor")
        .to_owned();

    fixture.append_diskstats(&[(100, 2, 12)]);

    let resumed = stream(fixture.prepare(
        &target(
            "rows",
            &format!("field=reads&page_size=1&order=asc&cursor={cursor}"),
        ),
        None,
    ))
    .expect("resume pinned page");
    let resumed_rows = row_records(&resumed);
    assert_eq!(resumed_rows.len(), 1);
    assert_eq!(resumed_rows[0]["ordinal"], "1");
    assert_eq!(
        resumed
            .iter()
            .find(|record| record["record"] == "page")
            .expect("page trailer")["next_cursor"],
        Value::Null
    );

    let fresh =
        stream(fixture.prepare(&target("rows", "field=reads&page_size=100&order=asc"), None))
            .expect("fresh page");
    assert_eq!(
        row_records(&fresh)
            .iter()
            .map(|row| row["ordinal"].as_str().expect("ordinal"))
            .collect::<Vec<_>>(),
        ["0", "1", "2"]
    );

    let tail = stream(fixture.prepare(
        &target(
            "history",
            &format!("field=reads&after={SEGMENT_ID},{first_position}"),
        ),
        None,
    ))
    .expect("active history tail");
    let tail_rows = row_records(&tail);
    assert_eq!(tail_rows.len(), 1);
    assert_eq!(tail_rows[0]["ordinal"], "2");
    assert_eq!(tail_rows[0]["timestamp"], "100");
    assert_eq!(tail_rows[0]["identity"], serde_json::json!([8, 2]));
}

#[test]
fn finished_index_and_catalog_have_revalidation_contracts_and_source_facts() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 0, 9)]);
    fixture.append_health();
    fixture.finish();

    let index_target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    let prepared = fixture.prepare(&index_target, None);
    let meta = prepared.meta();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::Immutable);
    let etag = meta.etag.expect("finished index ETag");
    let index = stream(prepared).expect("finished index body");
    assert!(index.iter().any(|record| record["record"] == "point"));

    assert!(etag.starts_with("W/\""));
    let strong = etag.strip_prefix("W/").expect("weak validator");
    let offered = format!("\"stale\", {strong}");
    let not_modified = fixture.prepare(&index_target, Some(&offered));
    assert_eq!(not_modified.meta().status, StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.meta().cache, CachePolicy::Immutable);
    assert_eq!(not_modified.meta().etag.as_deref(), Some(etag.as_str()));
    assert!(matches!(not_modified, Prepared::Empty(_)));

    let catalog = fixture.prepare("/api/catalog", None);
    assert_eq!(catalog.meta().cache, CachePolicy::Revalidate);
    assert_eq!(catalog.meta().etag, None);
    let catalog = stream(catalog).expect("catalog body");
    let header = catalog
        .iter()
        .find(|record| record["record"] == "catalog")
        .expect("catalog header");
    let families = header["source_families"]
        .as_array()
        .expect("source families");
    let os = families
        .iter()
        .find(|family| family["name"] == "os")
        .expect("OS source family");
    assert_eq!(os["configured"], true);
    assert_eq!(os["present"], true);
    let postgresql = families
        .iter()
        .find(|family| family["name"] == "postgresql")
        .expect("PostgreSQL source family");
    assert_eq!(postgresql["configured"], true);
    assert_eq!(postgresql["present"], false);

    let history = fixture.prepare(&target("history", "field=reads"), None);
    assert_eq!(history.meta().cache, CachePolicy::Immutable);
    assert_eq!(
        row_records(&stream(history).expect("finished history")).len(),
        2
    );
}

#[tokio::test]
async fn weak_index_etag_revalidates_both_representations_without_a_304_body() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    fixture.finish();
    let resource = format!("/api/segments/{SEGMENT_ID}/sections/health/index");

    let prepared = fixture.prepare(&resource, None);
    let response = crate::blocking_stream(move || Ok(prepared), accepted("identity")).await;
    assert_eq!(response.status(), StatusCode::OK);
    let etag = response
        .headers()
        .get(ETAG)
        .expect("ETag")
        .to_str()
        .expect("ASCII ETag")
        .to_owned();
    assert!(etag.starts_with("W/\""));
    assert_eq!(
        response.headers().get(VARY),
        Some(&HeaderValue::from_static("Authorization, Accept-Encoding"))
    );
    assert!(
        !response
            .into_body()
            .collect()
            .await
            .expect("index body")
            .to_bytes()
            .is_empty()
    );

    let strong = etag.strip_prefix("W/").expect("weak validator");
    for offered in [format!("\"stale\", {strong}"), "*".to_owned()] {
        let prepared = fixture.prepare(&resource, Some(&offered));
        let response =
            crate::blocking_stream(move || Ok(prepared), accepted("gzip, identity;q=0")).await;
        assert_eq!(response.status(), StatusCode::NOT_MODIFIED, "{offered}");
        assert_eq!(
            response
                .headers()
                .get(ETAG)
                .and_then(|value| value.to_str().ok()),
            Some(etag.as_str())
        );
        assert_eq!(
            response.headers().get(VARY),
            Some(&HeaderValue::from_static("Authorization, Accept-Encoding"))
        );
        assert!(!response.headers().contains_key(CONTENT_ENCODING));
        assert!(
            response
                .into_body()
                .collect()
                .await
                .expect("304 body")
                .to_bytes()
                .is_empty()
        );
    }
}

#[test]
fn health_is_streamed_as_an_ordinary_history_series_from_real_sections() {
    let mut fixture = Fixture::new();
    fixture.append_health();

    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/history?field=health");
    let records = stream(fixture.prepare(&target, None)).expect("health history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["timestamp"], "100");
    assert_eq!(rows[0]["values"], serde_json::json!([null]));
    assert_eq!(rows[1]["timestamp"], "200");
    assert_eq!(rows[1]["values"], serde_json::json!([50]));

    let index_target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    let index = fixture.prepare(&index_target, None);
    assert_eq!(index.meta().cache, CachePolicy::NoStore);
    let index = stream(index).expect("health index");
    assert!(index.iter().any(|record| {
        record["record"] == "point"
            && record["series"] == "os_health"
            && record["type_id"] == "0"
            && record["value"] == 50
    }));
}

#[test]
fn postgres_health_counts_every_active_backend_and_adds_its_penalty() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(5);

    let target = format!("/api/segments/{SEGMENT_ID}/sections/health/index");
    let records = stream(fixture.prepare(&target, None)).expect("health index");
    let points = |series: &str| {
        records
            .iter()
            .filter(|record| record["record"] == "point" && record["series"] == series)
            .collect::<Vec<_>>()
    };
    assert_eq!(points("postgres_health")[0]["value"], 80);
    assert_eq!(points("overall_health")[1]["value"], 30);
    assert_eq!(points("active_backends").len(), 0);
}

#[test]
fn a_snapshot_answers_for_the_sections_that_are_there() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(3);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_activity&section=os_diskstats"
    );
    let prepared = fixture.prepare(&target, None);
    assert_eq!(prepared.meta().status, StatusCode::OK);
    let records = stream(prepared).expect("snapshot body");
    let sections = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| record["layout"]["logical_name"].clone())
        .collect::<Vec<_>>();
    assert_eq!(sections, [serde_json::json!("pg_stat_activity")]);
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 4);
    assert!(rows.iter().all(|row| row["timestamp"] == "150"));
}

#[test]
fn snapshot_rows_align_positional_values_with_layout_columns() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(
        &(0..32)
            .map(|minor| (200, minor, i64::from(minor)))
            .collect::<Vec<_>>(),
    );
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=major&field=minor&field=scope&field=io_in_progress"
    );
    let prepared = fixture.prepare(&target, None);
    let mut body = Vec::new();
    prepared
        .stream(
            &mut |record| {
                body.extend_from_slice(&record);
                true
            },
            &|| false,
        )
        .expect("snapshot body");
    let text = std::str::from_utf8(&body).expect("UTF-8 snapshot");
    let records = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("JSON record"))
        .collect::<Vec<_>>();
    let layout = records
        .iter()
        .find(|record| record["record"] == "layout")
        .expect("projected layout");
    assert_eq!(
        layout["layout"]["columns"]
            .as_array()
            .expect("layout columns")
            .iter()
            .map(|column| column["name"].clone())
            .collect::<Vec<_>>(),
        ["major", "minor", "scope", "io_in_progress"].map(Value::from)
    );
    assert_eq!(
        layout["layout"]["identity"],
        serde_json::json!(["major", "minor"])
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 32);
    for (minor, row) in rows.iter().enumerate() {
        assert_eq!(row["values"], serde_json::json!([8, minor, 0, "0"]));
        assert!(row["values"].as_object().is_none());
    }
}

#[test]
fn an_hour_carries_its_segments_and_its_line_in_one_response() {
    let mut fixture = Fixture::new();
    fixture.append_health();
    let active = fixture.prepare("/api/hour?from=0&to=1000", None);
    assert_eq!(active.meta().cache, CachePolicy::NoStore);
    fixture.finish();

    let prepared = fixture.prepare("/api/hour?from=0&to=1000", None);
    assert_eq!(prepared.meta().status, StatusCode::OK);
    assert_eq!(prepared.meta().cache, CachePolicy::Revalidate);
    let records = stream(prepared).expect("hour body");
    let kinds = records
        .iter()
        .map(|record| record["record"].as_str().expect("record kind"))
        .collect::<Vec<_>>();
    assert!(kinds.contains(&"catalog"), "{kinds:?}");
    assert!(kinds.contains(&"finished_segment"), "{kinds:?}");
    assert!(kinds.contains(&"index"), "{kinds:?}");
    let points = records
        .iter()
        .filter(|record| record["record"] == "point")
        .collect::<Vec<_>>();
    assert!(!points.is_empty());
    assert!(points.iter().all(|point| point["series"] != Value::Null));
    let segment = records
        .iter()
        .find(|record| record["record"] == "index")
        .expect("index header");
    assert_eq!(segment["segment"]["id"], SEGMENT_ID.to_string());
    assert_eq!(segment["logical_name"], "health");
}

#[test]
fn an_hour_reads_one_index_resource_per_segment_and_carries_every_finding() {
    const STEP: i64 = 5 * 60 * 1_000_000;

    let mut fixture = Fixture::new();
    let process_prior = (0_i64..6)
        .map(|at| (SEGMENT_ID + at * STEP, Some(at * 300)))
        .collect::<Vec<_>>();
    let statement_prior = (0_i32..6)
        .map(|at| {
            (
                SEGMENT_ID + i64::from(at) * STEP,
                i64::from(at),
                f64::from(at) * 100.0,
            )
        })
        .collect::<Vec<_>>();
    fixture.append_finding_rows(&process_prior, &statement_prior);

    let spike_at = SEGMENT_ID + 6 * STEP;
    fixture.finish_and_continue(spike_at);
    fixture.append_finding_rows(&[(spike_at, Some(301_500))], &[(spike_at, 6, 10_500.0)]);
    fixture.append_postgres_health_at(spike_at, 5);
    fixture.append_log_error(spike_at + 60);
    fixture.finish();

    let records = stream(fixture.prepare(
        &format!(
            "/api/hour?from={SEGMENT_ID}&to={}",
            SEGMENT_ID + 3_600_000_000 - 1
        ),
        None,
    ))
    .expect("hour with findings");
    let index_segments = records
        .iter()
        .filter(|record| record["record"] == "index")
        .map(|record| record["segment"]["id"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        index_segments,
        [SEGMENT_ID.to_string(), spike_at.to_string()].map(Value::from)
    );

    let findings = records
        .iter()
        .filter(|record| record["record"] == "finding")
        .collect::<Vec<_>>();
    for (logical_name, kind) in [
        ("os_process", "spike"),
        ("pg_stat_activity", "known_bad"),
        ("pg_stat_statements", "spike"),
        ("pg_log_errors", "event"),
    ] {
        assert!(
            findings.iter().any(|finding| {
                finding["logical_name"] == logical_name && finding["kind"] == kind
            })
        );
    }
    let error = findings
        .iter()
        .find(|finding| finding["logical_name"] == "pg_log_errors")
        .expect("error event");
    assert_eq!(error["category"], 8);
}

#[test]
fn snapshot_cache_policy_tracks_active_and_finished_inputs() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 7), (200, 0, 9)]);
    let resource =
        format!("/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=reads");

    let active = fixture.prepare(&resource, None);
    assert_eq!(active.meta().cache, CachePolicy::NoStore);

    fixture.finish();
    let finished = fixture.prepare(&resource, None);
    assert_eq!(finished.meta().cache, CachePolicy::Revalidate);
}

#[test]
fn a_snapshot_orders_by_a_column_and_returns_only_the_top_of_it() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[
        (100, 0, 1),
        (100, 1, 9),
        (100, 2, 5),
        (200, 0, 2),
        (200, 1, 30),
        (200, 2, 11),
    ]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=major&field=minor&by=minor&top=2"
    );
    let records = stream(fixture.prepare(&target, None)).expect("ordered snapshot");
    let minors = records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|record| record["values"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        minors,
        [serde_json::json!([8, 2]), serde_json::json!([8, 1])]
    );
}

#[test]
fn snapshot_resolves_text_only_after_top_rows_are_selected() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_diskstats_with_unreadable_loser();
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&field=device&by=minor&top=1"
    );
    let records = stream(fixture.prepare(&target, None)).expect("top row snapshot");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([2, "nvme0n1"]));
}

#[test]
fn a_snapshot_projects_one_exact_physical_source_row() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[
        (100, 0, 1),
        (100, 1, 9),
        (100, 2, 5),
        (200, 0, 2),
        (200, 1, 30),
        (200, 2, 11),
    ]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&type_id=1108001&row_ordinal=3"
    );
    let records = stream(fixture.prepare(&target, None)).expect("exact projected snapshot");
    let layout = records
        .iter()
        .find(|record| record["record"] == "layout")
        .expect("layout");
    assert_eq!(layout["layout"]["type_id"], "1108001");
    assert_eq!(
        layout["layout"]["columns"],
        serde_json::json!([
            {"name": "minor", "type": "i32", "class": "label", "unit": "none", "nullable": false, "available": true},
            {"name": "reads", "type": "i64", "class": "cumulative", "unit": "count", "nullable": false, "available": true}
        ])
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["ordinal"], "3");
    assert_eq!(rows[0]["timestamp"], "200");
    assert_eq!(rows[0]["values"], serde_json::json!([1, 210_000.0]));
}

#[test]
fn an_exact_source_pointer_must_name_a_finished_row_at_its_timestamp() {
    let mut active = Fixture::new();
    active.append_diskstats(&[(200, 0, 2)]);
    for query in [
        "at=200&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0",
        "at=199&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0",
    ] {
        let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
        let route = crate::route::parse(&path, Some(query)).expect("exact route");
        assert!(matches!(
            crate::api::prepare(active.root(), SOURCES, route, None),
            Err(ApiError::BadCursor)
        ));
    }

    active.finish();
    for query in [
        "at=199&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0",
        "at=200&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=1",
    ] {
        let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
        let route = crate::route::parse(&path, Some(query)).expect("exact route");
        assert!(matches!(
            crate::api::prepare(active.root(), SOURCES, route, None),
            Err(ApiError::BadCursor)
        ));
    }
}

#[test]
fn a_snapshot_keeps_only_the_rows_a_filter_names() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (100, 1, 9), (200, 0, 2), (200, 1, 30)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&where.minor=1"
    );
    let records = stream(fixture.prepare(&target, None)).expect("filtered snapshot");
    let minors = records
        .iter()
        .filter(|record| record["record"] == "row")
        .map(|record| record["values"][0].clone())
        .collect::<Vec<_>>();
    assert_eq!(minors, [serde_json::json!(1)]);
}

#[test]
fn an_order_needs_one_section_to_name_a_column_in() {
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    assert!(crate::route::parse(&path, Some("at=1&section=a&section=b&by=x")).is_err());
    assert!(crate::route::parse(&path, Some("at=1&section=a&section=b&top=5")).is_err());
    assert!(crate::route::parse(&path, Some("at=1&section=a&by=x&top=5")).is_ok());
}

#[test]
fn the_first_moment_of_a_segment_rates_against_the_segment_before_it() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");

    let first = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("first id")).expect("first");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(100, 0, 10)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(first.id, &part).expect("append first");
    write_segment(&journal, &writer, first).expect("finish first");
    journal.reset().expect("close the first segment");

    let second = SegmentAddress::new(SegmentId::new(SEGMENT_ID + 1_000).expect("second id"))
        .expect("second");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(200, 0, 30)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(second.id, &part).expect("append second");
    write_segment(&journal, &writer, second).expect("finish second");

    let target = format!(
        "/api/segments/{}/snapshot?at=200&section=os_diskstats&field=reads",
        SEGMENT_ID + 1_000
    );
    let path = target.split('?').next().expect("path");
    let query = target.split_once('?').expect("query").1;
    let route = crate::route::parse(path, Some(query)).expect("route");
    let prepared =
        crate::api::prepare(directory.path(), SOURCES, route, None).expect("prepare snapshot");
    let records = stream(prepared).expect("snapshot body");
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], serde_json::json!(200_000.0));
}

#[test]
fn string_identity_matches_across_segment_dictionaries() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");

    let first = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("first id")).expect("first");
    let mut first_interner = Interner::new(DictLimits::default());
    let first_iface = StrId(
        first_interner
            .intern(b"eth0")
            .expect("first interface")
            .get(),
    );
    let first_dictionary = dict::encode(first_interner.window()).expect("first dictionary");
    let mut first_buffers = SectionBuffers::new();
    first_buffers
        .push(netdev(100, first_iface, 10))
        .expect("first netdev row");
    let first_part = first_buffers
        .flush(&first_dictionary)
        .expect("encode first")
        .expect("first part");
    journal.append(first.id, &first_part).expect("append first");
    write_segment(&journal, &writer, first).expect("finish first");
    journal.reset().expect("close first");

    let second = SegmentAddress::new(SegmentId::new(SEGMENT_ID + 1_000).expect("second id"))
        .expect("second");
    let mut second_interner = Interner::new(DictLimits::default());
    let _unused = second_interner
        .intern(b"lo")
        .expect("another dictionary value");
    let second_iface = StrId(
        second_interner
            .intern(b"eth0")
            .expect("second interface")
            .get(),
    );
    assert_eq!(
        first_iface, second_iface,
        "str_id is the stable content hash, not a segment-local ordinal"
    );
    let second_dictionary = dict::encode(second_interner.window()).expect("second dictionary");
    let mut second_buffers = SectionBuffers::new();
    second_buffers
        .push(netdev(200, second_iface, 30))
        .expect("second netdev row");
    let second_part = second_buffers
        .flush(&second_dictionary)
        .expect("encode second")
        .expect("second part");
    journal
        .append(second.id, &second_part)
        .expect("append second");
    write_segment(&journal, &writer, second).expect("finish second");

    let target = format!(
        "/api/segments/{}/snapshot?at=200&section=os_netdev&field=iface&field=rx_bytes",
        SEGMENT_ID + 1_000
    );
    let (path, query) = target.split_once('?').expect("snapshot query");
    let route = crate::route::parse(path, Some(query)).expect("snapshot route");
    let prepared = crate::api::prepare(directory.path(), SOURCES, route, None)
        .expect("prepare netdev snapshot");
    let records = stream(prepared).expect("netdev snapshot");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!(["eth0", 200_000.0]));
}

#[test]
fn a_moment_before_the_first_sample_here_is_answered_from_the_segment_before() {
    let directory = tempfile::tempdir().expect("tempdir");
    let root = DataRoot::open(directory.path()).expect("data root");
    let writer = root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("open journal");

    let first = SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("first id")).expect("first");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(100, 0, 7)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(first.id, &part).expect("append first");
    write_segment(&journal, &writer, first).expect("finish first");
    journal.reset().expect("close the first segment");

    let second = SegmentAddress::new(SegmentId::new(SEGMENT_ID + 1_000).expect("second id"))
        .expect("second");
    let mut buffers = SectionBuffers::new();
    buffers.push(diskstats(900, 0, 9)).expect("row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(second.id, &part).expect("append second");
    write_segment(&journal, &writer, second).expect("finish second");

    let path = format!("/api/segments/{}/snapshot", SEGMENT_ID + 1_000);
    let route =
        crate::route::parse(&path, Some("at=400&section=os_diskstats&field=reads")).expect("route");
    let prepared =
        crate::api::prepare(directory.path(), SOURCES, route, None).expect("prepare snapshot");
    let records = stream(prepared).expect("snapshot body");
    let rows = records
        .iter()
        .filter(|record| record["record"] == "row")
        .collect::<Vec<_>>();
    assert_eq!(
        rows.len(),
        1,
        "the earlier segment answers instead of nothing"
    );
    assert_eq!(rows[0]["timestamp"], "100");
}
