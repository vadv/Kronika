use std::cell::Cell;
use std::collections::BTreeMap;
use std::io::Read as _;
use std::path::Path;

use flate2::read::GzDecoder;
use http_body_util::BodyExt as _;
use hyper::header::{ACCEPT_ENCODING, CONTENT_ENCODING, CONTENT_TYPE, ETAG, HeaderValue, VARY};
use hyper::{HeaderMap, StatusCode};
use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::InstanceMetadata;
use kronika_registry::os_cgroup_cpu::OsCgroupCpu;
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_netdev::OsNetdev;
use kronika_registry::os_process::OsProcess;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::pg_log::{PgLogErrors, PgLogTempFiles};
use kronika_registry::pg_stat_activity::PgStatActivityV3;
use kronika_registry::pg_stat_statements::PgStatStatementsV2;
use kronika_registry::pg_stat_user_indexes::{PgStatUserIndexesV1, PgStatUserIndexesV2};
use kronika_registry::pg_stat_user_tables::PgStatUserTablesV1;
use kronika_registry::pg_store_plans::PgStorePlansOsscV1;
use kronika_registry::{Section, StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::Value;

use crate::api::{ApiError, CachePolicy, Prepared, context_operations, reset_context_operations};
use crate::config::SOURCE_OS;
use crate::encoding::AcceptedEncodings;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SOURCES: u32 = 0b11;

type NamedIndexSnapshot<'a> = (
    i64,
    u32,
    u32,
    i64,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
    &'a str,
);

type DmlTableSnapshot<'a> = (i64, u32, u32, [i64; 4], &'a str, &'a str, &'a str);

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

    fn append_named_diskstats(&mut self, rows: &[(i32, &str)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(minor, device) in rows {
            let device = StrId(
                interner
                    .intern(device.as_bytes())
                    .expect("intern device")
                    .get(),
            );
            buffers
                .push(diskstats_with_device(100, minor, 1, device))
                .expect("named diskstats row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode device dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode named diskstats fixture")
            .expect("nonempty named diskstats fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append named diskstats fixture");
    }

    fn append_large_cgroup_cpu(&mut self, rows: usize, selected: usize) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for index in 0..rows {
            let path = if index == selected {
                "/collector".to_owned()
            } else {
                format!("/tree/cgroup-{index}")
            };
            let cgroup_path = StrId(
                interner
                    .intern(path.as_bytes())
                    .expect("intern large cgroup path")
                    .get(),
            );
            buffers
                .push(OsCgroupCpu {
                    ts: Ts(200),
                    cgroup_path,
                    usage_usec: i64::try_from(index).expect("fixture index fits i64"),
                    user_usec: 0,
                    system_usec: 0,
                    throttled_usec: 0,
                    nr_throttled: 0,
                    quota_usec: -1,
                    period_usec: 100_000,
                    scope: 3,
                })
                .expect("large cgroup CPU row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode large cgroup dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode large cgroup CPU fixture")
            .expect("nonempty large cgroup CPU fixture");
        self.journal
            .append(self.address.id, &part)
            .expect("append large cgroup CPU fixture");
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

    fn append_process_summary_snapshot(
        &mut self,
        ts: i64,
        counter: i64,
        changed_starttime_pid: Option<i32>,
        activity_ts: i64,
        activity_pids: std::ops::Range<i32>,
        future_activity: Option<(i64, std::ops::Range<i32>)>,
    ) {
        let mut interner = Interner::new(DictLimits::default());
        let label = StrId(
            interner
                .intern(b"fixture")
                .expect("intern process label")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("process summary dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(InstanceMetadata {
                ts: Ts(ts),
                hostname: label,
                kernel_version: label,
                environment: 0,
                clock_ticks_per_sec: 100,
                page_size_bytes: 4_096,
                boot_id: label,
                btime: Ts(1),
                postgresql_enabled: true,
                postgresql_interval_seconds: 30,
                postgresql_effective_cpus: Some(2),
            })
            .expect("summary metadata fits");
        for pid in 0..205 {
            let mut row = process_for_summary(ts, pid, counter, label);
            if changed_starttime_pid == Some(pid) {
                row.starttime = Ts(row.starttime.0 + 1);
            }
            buffers.push(row).expect("summary process row fits");
        }
        for pid in activity_pids {
            buffers
                .push(activity(activity_ts, pid, label, label))
                .expect("past activity row fits");
        }
        if let Some((future_ts, pids)) = future_activity {
            for pid in pids {
                buffers
                    .push(activity(future_ts, pid, label, label))
                    .expect("future activity row fits");
            }
        }
        let part = buffers
            .flush(&dictionary)
            .expect("encode process summary snapshot")
            .expect("nonempty process summary snapshot");
        self.journal
            .append(self.address.id, &part)
            .expect("append process summary snapshot");
    }

    fn append_statement_universe(&mut self, rows: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for queryid in 0..rows {
            let text = if queryid == 0 {
                "owner blocker-needle outside page one".to_owned()
            } else {
                format!("fixture statement {queryid}")
            };
            let query = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern statement text")
                    .get(),
            );
            let mut row = statement(100, 1, 1.0, query);
            row.queryid = Some(queryid);
            buffers.push(row).expect("statement row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("statement dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode statements")
            .expect("nonempty statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append statements");
    }

    fn append_statement_snapshots(&mut self, rows: &[(i64, i64, i64, f64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(ts, queryid, calls, total_exec_time) in rows {
            let text = format!("boundary statement {queryid}");
            let query = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern boundary statement")
                    .get(),
            );
            let mut row = statement(ts, calls, total_exec_time, query);
            row.queryid = Some(queryid);
            buffers.push(row).expect("boundary statement row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("boundary statement dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode boundary statements")
            .expect("nonempty boundary statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append boundary statements");
    }

    fn append_plan_universe(&mut self, rows: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for queryid in 0..rows {
            let text = if queryid == 0 {
                "owner plan-needle outside page one".to_owned()
            } else {
                format!("fixture plan {queryid}")
            };
            let plan_text = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern plan text")
                    .get(),
            );
            buffers
                .push(store_plan(100, queryid, plan_text))
                .expect("plan row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode plans")
            .expect("nonempty plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append plans");
    }

    fn append_plan_snapshots(&mut self, rows: &[(i64, i64, i64, f64)]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        for &(ts, queryid, calls, total_time) in rows {
            let text = format!("boundary plan {queryid}");
            let plan_text = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern boundary plan")
                    .get(),
            );
            let mut row = store_plan(ts, queryid, plan_text);
            row.calls = calls;
            row.total_time = total_time;
            buffers.push(row).expect("boundary plan row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("boundary plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode boundary plans")
            .expect("nonempty boundary plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append boundary plans");
    }

    fn append_ranked_statements(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        let readings = [
            (1, 10, 100.0, 100, 80, 20, 0, 0, 100, 100.0, 10.0, 9.0),
            (2, 2, 30.0, 60, 9, 1, 40, 0, 60, 40.0, 1.0, 2.0),
            (3, 1, 5.0, 1, 1, 2, 0, 0, 5, 1.0, 5.0, 1.0),
        ];
        for (
            queryid,
            calls,
            execution,
            rows,
            hit,
            read,
            local_hit,
            local_read,
            wal,
            planning,
            mean,
            deviation,
        ) in readings
        {
            let text = format!("ranked statement {queryid}");
            let query = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern ranked statement")
                    .get(),
            );
            for ts in [100, 200] {
                let current = ts == 200;
                let mut row = statement(
                    ts,
                    if current { calls } else { 0 },
                    if current { execution } else { 0.0 },
                    query,
                );
                row.queryid = Some(queryid);
                row.rows = if current { rows } else { 0 };
                row.shared_blks_hit = if current { hit } else { 0 };
                row.shared_blks_read = if current { read } else { 0 };
                row.local_blks_hit = if current { local_hit } else { 0 };
                row.local_blks_read = if current { local_read } else { 0 };
                row.wal_bytes = if current { wal } else { 0 };
                row.total_plan_time = if current { planning } else { 0.0 };
                row.mean_exec_time = mean;
                row.stddev_exec_time = deviation;
                buffers.push(row).expect("ranked statement row fits");
            }
        }
        let dictionary = dict::encode(interner.window()).expect("ranked statement dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked statements")
            .expect("nonempty ranked statements");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked statements");
    }

    fn append_ranked_plans(&mut self) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        let readings = [
            (1, 10, 100.0, 100, 80, 20, 0, 0),
            (2, 2, 30.0, 60, 9, 1, 40, 0),
            (3, 1, 5.0, 1, 1, 2, 0, 0),
        ];
        for (queryid, calls, execution, rows, hit, read, local_hit, local_read) in readings {
            let text = format!("ranked plan {queryid}");
            let plan_text = StrId(
                interner
                    .intern(text.as_bytes())
                    .expect("intern ranked plan")
                    .get(),
            );
            for ts in [100, 200] {
                let current = ts == 200;
                let mut row = store_plan(ts, queryid, plan_text);
                row.calls = if current { calls } else { 0 };
                row.total_time = if current { execution } else { 0.0 };
                row.rows = if current { rows } else { 0 };
                row.shared_blks_hit = if current { hit } else { 0 };
                row.shared_blks_read = if current { read } else { 0 };
                row.local_blks_hit = if current { local_hit } else { 0 };
                row.local_blks_read = if current { local_read } else { 0 };
                buffers.push(row).expect("ranked plan row fits");
            }
        }
        let dictionary = dict::encode(interner.window()).expect("ranked plan dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode ranked plans")
            .expect("nonempty ranked plans");
        self.journal
            .append(self.address.id, &part)
            .expect("append ranked plans");
    }

    fn append_relation_snapshots(
        &mut self,
        tables: &[(i64, u32, u32, i64)],
        indexes_v1: &[(i64, u32, u32, i64)],
        indexes_v2: &[(i64, u32, u32, i64)],
    ) {
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, relid, seq_scan) in tables {
            buffers
                .push(user_table(ts, datid, relid, seq_scan))
                .expect("table snapshot row fits");
        }
        for &(ts, datid, indexrelid, idx_scan) in indexes_v1 {
            buffers
                .push(user_index_v1(ts, datid, indexrelid, idx_scan))
                .expect("V1 index snapshot row fits");
        }
        for &(ts, datid, indexrelid, idx_scan) in indexes_v2 {
            buffers
                .push(user_index_v2(ts, datid, indexrelid, idx_scan))
                .expect("V2 index snapshot row fits");
        }
        self.append(buffers);
    }

    fn append_named_table_snapshots(&mut self, rows: &[(i64, u32, u32, i64, &str, &str, &str)]) {
        let mut interner = Interner::new(DictLimits::default());
        let tablespace = StrId(
            interner
                .intern(b"pg_default")
                .expect("intern tablespace")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, relid, seq_scan, datname, schemaname, relname) in rows {
            let mut row = user_table(ts, datid, relid, seq_scan);
            row.datname = StrId(
                interner
                    .intern(datname.as_bytes())
                    .expect("intern database")
                    .get(),
            );
            row.schemaname = StrId(
                interner
                    .intern(schemaname.as_bytes())
                    .expect("intern schema")
                    .get(),
            );
            row.relname = StrId(
                interner
                    .intern(relname.as_bytes())
                    .expect("intern table")
                    .get(),
            );
            row.tablespace = tablespace;
            buffers.push(row).expect("named table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode relation dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode named table snapshots")
            .expect("nonempty named table snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append named table snapshots");
    }

    fn append_dml_table_snapshots(&mut self, rows: &[DmlTableSnapshot<'_>]) {
        let mut interner = Interner::new(DictLimits::default());
        let tablespace = StrId(
            interner
                .intern(b"pg_default")
                .expect("intern tablespace")
                .get(),
        );
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, relid, [inserted, updated, deleted, hot], datname, schema, table) in rows {
            let mut row = user_table(ts, datid, relid, 0);
            row.n_tup_ins = inserted;
            row.n_tup_upd = updated;
            row.n_tup_del = deleted;
            row.n_tup_hot_upd = hot;
            row.datname = StrId(
                interner
                    .intern(datname.as_bytes())
                    .expect("intern database")
                    .get(),
            );
            row.schemaname = StrId(
                interner
                    .intern(schema.as_bytes())
                    .expect("intern schema")
                    .get(),
            );
            row.relname = StrId(
                interner
                    .intern(table.as_bytes())
                    .expect("intern table")
                    .get(),
            );
            row.tablespace = tablespace;
            buffers.push(row).expect("DML table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode DML relation dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode DML table snapshots")
            .expect("nonempty DML table snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append DML table snapshots");
    }

    fn append_buffered_table_snapshots(&mut self, rows: &[(i64, u32, u32, [i64; 8])]) {
        let mut interner = Interner::new(DictLimits::default());
        let mut buffers = SectionBuffers::new();
        let labels = ["fixture_db", "public", "buffered_table", "pg_default"].map(|label| {
            StrId(
                interner
                    .intern(label.as_bytes())
                    .expect("intern buffered table label")
                    .get(),
            )
        });
        for &(ts, datid, relid, counters) in rows {
            let mut row = buffered_user_table(ts, datid, relid, counters);
            [row.datname, row.schemaname, row.relname, row.tablespace] = labels;
            buffers.push(row).expect("buffered table row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode buffered table dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode buffered table snapshots")
            .expect("nonempty buffered table snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append buffered table snapshots");
    }

    fn append_named_index_snapshots(&mut self, rows: &[NamedIndexSnapshot<'_>]) {
        let mut interner = Interner::new(DictLimits::default());
        let tablespace = StrId(
            interner
                .intern(b"pg_default")
                .expect("intern tablespace")
                .get(),
        );
        let amname = StrId(interner.intern(b"btree").expect("intern AM").get());
        let mut buffers = SectionBuffers::new();
        for &(ts, datid, indexrelid, idx_scan, datname, schema, table, index, definition) in rows {
            let mut row = user_index_v2(ts, datid, indexrelid, idx_scan);
            row.datname = StrId(
                interner
                    .intern(datname.as_bytes())
                    .expect("intern database")
                    .get(),
            );
            row.schemaname = StrId(
                interner
                    .intern(schema.as_bytes())
                    .expect("intern schema")
                    .get(),
            );
            row.relname = StrId(
                interner
                    .intern(table.as_bytes())
                    .expect("intern table")
                    .get(),
            );
            row.indexrelname = StrId(
                interner
                    .intern(index.as_bytes())
                    .expect("intern index")
                    .get(),
            );
            row.tablespace = tablespace;
            row.amname = amname;
            row.indexdef = Some(StrId(
                interner
                    .intern(definition.as_bytes())
                    .expect("intern index definition")
                    .get(),
            ));
            buffers.push(row).expect("named index row fits");
        }
        let dictionary = dict::encode(interner.window()).expect("encode index dictionary");
        let part = buffers
            .flush(&dictionary)
            .expect("encode named index snapshots")
            .expect("nonempty named index snapshots");
        self.journal
            .append(self.address.id, &part)
            .expect("append named index snapshots");
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

    fn append_log_temp_file(&mut self, at: i64) {
        let mut interner = Interner::new(DictLimits::default());
        let source_file = StrId(
            interner
                .intern(b"postgresql.log")
                .expect("intern source file")
                .get(),
        );
        let path = StrId(
            interner
                .intern(b"base/pgsql_tmp/pgsql_tmp42.0")
                .expect("intern temporary-file path")
                .get(),
        );
        let statement = StrId(
            interner
                .intern(b"select fixture spill")
                .expect("intern statement")
                .get(),
        );
        let dictionary = dict::encode(interner.window()).expect("temporary-file dictionary");
        let mut buffers = SectionBuffers::new();
        buffers
            .push(PgLogTempFiles {
                ts: Ts(at),
                system_identifier: Some(42),
                source_file,
                path: Some(path),
                size_bytes: 1_048_576,
                statement: Some(statement),
            })
            .expect("temporary-file row fits");
        let part = buffers
            .flush(&dictionary)
            .expect("encode temporary-file row")
            .expect("nonempty temporary-file row");
        self.journal
            .append(self.address.id, &part)
            .expect("append temporary-file row");
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
        self.prepare_with_sources(target, if_none_match, SOURCES)
    }

    fn prepare_with_sources(
        &self,
        target: &str,
        if_none_match: Option<&str>,
        sources: u32,
    ) -> Prepared {
        let (path, query) = target
            .split_once('?')
            .map_or((target, None), |(path, query)| (path, Some(query)));
        let route = crate::route::parse(path, query).expect("valid fixture route");
        crate::api::prepare(self.root(), sources, route, if_none_match)
            .expect("prepare fixture resource")
    }
}

fn diskstats(ts: i64, minor: i32, reads: i64) -> OsDiskstats {
    diskstats_with_device(ts, minor, reads, StrId(999))
}

fn user_table(ts: i64, datid: u32, relid: u32, seq_scan: i64) -> PgStatUserTablesV1 {
    PgStatUserTablesV1 {
        ts: Ts(ts),
        datid,
        datname: StrId(901),
        relid,
        schemaname: StrId(902),
        relname: StrId(903),
        tablespace: StrId(904),
        seq_scan,
        seq_tup_read: 0,
        idx_scan: None,
        idx_tup_fetch: None,
        n_tup_ins: 0,
        n_tup_upd: 0,
        n_tup_del: 0,
        n_tup_hot_upd: 0,
        n_live_tup: 0,
        n_dead_tup: 0,
        n_mod_since_analyze: 0,
        vacuum_count: 0,
        autovacuum_count: 0,
        analyze_count: 0,
        autoanalyze_count: 0,
        last_vacuum: None,
        last_autovacuum: None,
        last_analyze: None,
        last_autoanalyze: None,
        main_fork_bytes: 0,
        toast_bytes: None,
        toast_n_live_tup: None,
        toast_n_dead_tup: None,
        toast_last_autovacuum: None,
        xid_age: None,
        mxid_age: None,
        reltuples: 0,
        heap_blks_read: 0,
        heap_blks_hit: 0,
        idx_blks_read: None,
        idx_blks_hit: None,
        toast_blks_read: None,
        toast_blks_hit: None,
        tidx_blks_read: None,
        tidx_blks_hit: None,
    }
}

fn buffered_user_table(ts: i64, datid: u32, relid: u32, buffers: [i64; 8]) -> PgStatUserTablesV1 {
    let mut row = user_table(ts, datid, relid, 0);
    let [
        heap_read,
        heap_hit,
        index_read,
        index_hit,
        toast_read,
        toast_hit,
        toast_index_read,
        toast_index_hit,
    ] = buffers;
    row.heap_blks_read = heap_read;
    row.heap_blks_hit = heap_hit;
    row.idx_blks_read = Some(index_read);
    row.idx_blks_hit = Some(index_hit);
    row.toast_blks_read = Some(toast_read);
    row.toast_blks_hit = Some(toast_hit);
    row.tidx_blks_read = Some(toast_index_read);
    row.tidx_blks_hit = Some(toast_index_hit);
    row
}

fn user_index_v1(ts: i64, datid: u32, indexrelid: u32, idx_scan: i64) -> PgStatUserIndexesV1 {
    PgStatUserIndexesV1 {
        ts: Ts(ts),
        datid,
        datname: StrId(901),
        indexrelid,
        relid: indexrelid - 1,
        schemaname: StrId(902),
        relname: StrId(903),
        indexrelname: StrId(905),
        tablespace: StrId(904),
        idx_scan,
        idx_tup_read: 0,
        idx_tup_fetch: 0,
        main_fork_bytes: 0,
        indisunique: false,
        indisprimary: false,
        indisvalid: true,
        indisexclusion: false,
        indisready: true,
        amname: StrId(906),
        indexdef: None,
        idx_blks_read: 0,
        idx_blks_hit: 0,
    }
}

fn user_index_v2(ts: i64, datid: u32, indexrelid: u32, idx_scan: i64) -> PgStatUserIndexesV2 {
    let base = user_index_v1(ts, datid, indexrelid, idx_scan);
    PgStatUserIndexesV2 {
        ts: base.ts,
        datid: base.datid,
        datname: base.datname,
        indexrelid: base.indexrelid,
        relid: base.relid,
        schemaname: base.schemaname,
        relname: base.relname,
        indexrelname: base.indexrelname,
        tablespace: base.tablespace,
        idx_scan: base.idx_scan,
        idx_tup_read: base.idx_tup_read,
        idx_tup_fetch: base.idx_tup_fetch,
        main_fork_bytes: base.main_fork_bytes,
        last_idx_scan: None,
        indisunique: base.indisunique,
        indisprimary: base.indisprimary,
        indisvalid: base.indisvalid,
        indisexclusion: base.indisexclusion,
        indisready: base.indisready,
        amname: base.amname,
        indexdef: base.indexdef,
        idx_blks_read: base.idx_blks_read,
        idx_blks_hit: base.idx_blks_hit,
    }
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

fn process_for_summary(ts: i64, pid: i32, counter: i64, label: StrId) -> OsProcess {
    let mut row = process(ts, Some(counter + i64::from(pid)), label);
    row.pid = pid;
    row.starttime = Ts(SEGMENT_ID - 1_000_000 + i64::from(pid));
    row.state = if pid % 2 == 0 { b'R' } else { b'S' };
    row.num_threads = 2;
    row.utime = counter + i64::from(pid);
    row.stime = counter * 2 + i64::from(pid);
    row.rundelay_ns = counter * 1_000_000 + i64::from(pid);
    row.nvcsw = counter + i64::from(pid);
    row.nivcsw = counter * 2 + i64::from(pid);
    row.rmem_kb = 10 + i64::from(pid);
    row.vmem_kb = 20 + i64::from(pid);
    row.vswap_kb = i64::from(pid % 3);
    row.majflt = counter + i64::from(pid);
    row.syscr = Some(counter + i64::from(pid));
    row.syscw = Some(counter * 2 + i64::from(pid));
    row.write_bytes = None;
    row
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

fn store_plan(ts: i64, queryid: i64, plan: StrId) -> PgStorePlansOsscV1 {
    PgStorePlansOsscV1 {
        ts: Ts(ts),
        queryid,
        planid: -7,
        userid: 10,
        dbid: 11,
        datname: None,
        usename: None,
        plan: Some(plan),
        calls: 4,
        total_time: 99.5,
        min_time: 1.0,
        max_time: 50.0,
        mean_time: 24.9,
        stddev_time: 2.2,
        rows: 40,
        shared_blks_hit: 1,
        shared_blks_read: 2,
        shared_blks_dirtied: 3,
        shared_blks_written: 4,
        local_blks_hit: 5,
        local_blks_read: 6,
        local_blks_dirtied: 7,
        local_blks_written: 8,
        temp_blks_read: 9,
        temp_blks_written: 10,
        shared_blk_read_time: 1.5,
        shared_blk_write_time: 2.5,
        local_blk_read_time: 3.5,
        local_blk_write_time: 4.5,
        temp_blk_read_time: 5.5,
        temp_blk_write_time: 6.5,
        first_call: Ts(ts - 1_000),
        last_call: Ts(ts - 1),
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

fn relation_records(records: &[Value]) -> Vec<&Value> {
    records
        .iter()
        .filter(|record| record["record"] == "relation")
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
        Some(&HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
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
    assert_eq!(os["metrics_present"], true);
    let postgresql = families
        .iter()
        .find(|family| family["name"] == "postgresql")
        .expect("PostgreSQL source family");
    assert_eq!(postgresql["configured"], true);
    assert_eq!(postgresql["present"], false);
    assert_eq!(postgresql["metrics_present"], false);

    let history = fixture.prepare(&target("history", "field=reads"), None);
    assert_eq!(history.meta().cache, CachePolicy::Immutable);
    assert_eq!(
        row_records(&stream(history).expect("finished history")).len(),
        2
    );
}

#[test]
fn hour_source_presence_uses_only_rows_inside_the_requested_window() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health_at(100, 0);
    fixture.finish();

    for (target, expected) in [
        ("/api/hour?from=100&to=199", true),
        ("/api/hour?from=200&to=300", false),
    ] {
        let records = stream(fixture.prepare_with_sources(target, None, SOURCE_OS))
            .expect("bounded hour response");
        let family = records
            .iter()
            .find(|record| record["record"] == "catalog")
            .and_then(|record| record["source_families"].as_array())
            .and_then(|families| {
                families
                    .iter()
                    .find(|family| family["name"] == "postgresql")
            })
            .expect("PostgreSQL source family");
        assert_eq!(family["configured"], false);
        assert_eq!(family["present"], expected);
        assert_eq!(family["metrics_present"], expected);
    }
}

#[test]
fn postgres_log_rows_do_not_claim_selected_hour_metrics() {
    let mut fixture = Fixture::new();
    fixture.append_log_error(100);
    fixture.finish();

    let records =
        stream(fixture.prepare_with_sources("/api/hour?from=100&to=199", None, SOURCE_OS))
            .expect("log-only hour response");
    let family = records
        .iter()
        .find(|record| record["record"] == "catalog")
        .and_then(|record| record["source_families"].as_array())
        .and_then(|families| {
            families
                .iter()
                .find(|family| family["name"] == "postgresql")
        })
        .expect("PostgreSQL source family");
    assert_eq!(family["configured"], false);
    assert_eq!(family["present"], true);
    assert_eq!(family["metrics_present"], false);
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
        Some(&HeaderValue::from_static(
            "Authorization, Cookie, Accept-Encoding"
        ))
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
            Some(&HeaderValue::from_static(
                "Authorization, Cookie, Accept-Encoding"
            ))
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
fn a_multi_section_snapshot_applies_the_shared_projection_per_section() {
    let mut fixture = Fixture::new();
    fixture.append_postgres_health(3);
    fixture.append_diskstats(&[(200, 0, 7)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_activity&section=os_diskstats&section=not_recorded&field=pid&field=minor"
    );
    let records = stream(fixture.prepare(&target, None)).expect("projected snapshot");
    let layouts = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| {
            (
                record["layout"]["logical_name"].clone(),
                record["layout"]["columns"]
                    .as_array()
                    .expect("layout columns")
                    .iter()
                    .map(|column| column["name"].clone())
                    .collect::<Vec<_>>(),
            )
        })
        .collect::<Vec<_>>();
    assert_eq!(
        layouts,
        [
            (
                serde_json::json!("pg_stat_activity"),
                vec![serde_json::json!("pid")]
            ),
            (
                serde_json::json!("os_diskstats"),
                vec![serde_json::json!("minor")]
            ),
        ]
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 5);
    assert_eq!(rows[0]["values"], serde_json::json!([0]));
    assert_eq!(rows[4]["values"], serde_json::json!([0]));
}

#[test]
fn a_multi_section_snapshot_keeps_partitioned_and_shared_predecessors_independent() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(&[(100, 1, 77, 10), (200, 1, 77, 20)], &[], &[]);
    fixture.append_diskstats(&[(200, 0, 7)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&section=os_diskstats&field=datid&field=minor"
    );
    let records = stream(fixture.prepare(&target, None)).expect("mixed snapshot");
    let layouts = records
        .iter()
        .filter(|record| record["record"] == "layout")
        .map(|record| record["layout"]["logical_name"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        layouts,
        [
            serde_json::json!("pg_stat_user_tables"),
            serde_json::json!("os_diskstats"),
        ]
    );
    let rows = row_records(&records);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["values"], serde_json::json!([1]));
    assert_eq!(rows[1]["values"], serde_json::json!([0]));
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
fn an_hour_keeps_generic_rows_and_lanes_inside_its_inclusive_window() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(99, 0, 9), (100, 0, 0), (200, 0, 2), (201, 0, 1)]);

    let series = stream(fixture.prepare(
        "/api/hour?from=100&to=200&section=os_diskstats&field=reads&field=discards",
        None,
    ))
    .expect("bounded generic series");
    let rows = row_records(&series);
    assert_eq!(
        rows.iter()
            .map(|row| row["timestamp"].clone())
            .collect::<Vec<_>>(),
        [serde_json::json!("100"), serde_json::json!("200")]
    );
    assert_eq!(rows[0]["values"], serde_json::json!(["0", null]));
    assert_eq!(rows[1]["values"], serde_json::json!(["2", null]));

    let hour =
        stream(fixture.prepare("/api/hour?from=100&to=200", None)).expect("bounded timeline lanes");
    let lanes = hour
        .iter()
        .filter(|record| record["record"] == "lane")
        .collect::<Vec<_>>();
    assert!(!lanes.is_empty());
    assert!(
        lanes
            .iter()
            .all(|lane| matches!(lane["ts"].as_str(), Some("100" | "200")))
    );
    for timestamp in ["100", "200"] {
        assert!(lanes.iter().any(|lane| lane["ts"] == timestamp));
    }
}

#[test]
fn an_hour_reads_one_index_resource_per_segment_without_statistical_noise() {
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

    let current_at = SEGMENT_ID + 6 * STEP;
    fixture.finish_and_continue(current_at);
    fixture.append_finding_rows(&[(current_at, Some(301_500))], &[(current_at, 6, 10_500.0)]);
    fixture.append_postgres_health_at(current_at, 5);
    fixture.append_log_error(current_at + 60);
    fixture.append_log_temp_file(current_at + 61);
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
        [SEGMENT_ID.to_string(), current_at.to_string()].map(Value::from)
    );

    let findings = records
        .iter()
        .filter(|record| record["record"] == "finding")
        .collect::<Vec<_>>();
    for (logical_name, kind) in [
        ("pg_stat_activity", "known_bad"),
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
    assert!(findings.iter().all(|finding| {
        finding["logical_name"] != "os_process"
            && finding["logical_name"] != "pg_stat_statements"
            && finding["kind"] != "spike"
    }));
    assert!(records.iter().all(|record| {
        !matches!(record["record"].as_str(), Some("findings" | "finding"))
            || record["type_id"] != "2007001"
    }));
}

#[test]
fn an_hour_filters_first_and_last_segment_findings_to_its_exact_bounds() {
    let from = SEGMENT_ID + 100;
    let to = SEGMENT_ID + 200;
    let mut fixture = Fixture::new();
    fixture.append_log_error(from - 1);
    fixture.append_log_error(from);
    fixture.finish_and_continue(to);
    fixture.append_log_error(to);
    fixture.append_log_error(to + 1);
    fixture.finish();

    let records = stream(fixture.prepare(&format!("/api/hour?from={from}&to={to}"), None))
        .expect("bounded hour");
    let summaries = records
        .iter()
        .filter(|record| record["record"] == "findings")
        .collect::<Vec<_>>();
    assert_eq!(
        summaries
            .iter()
            .map(|record| record["total_hits"].as_u64().expect("finding count"))
            .sum::<u64>(),
        2
    );
    assert!(summaries.iter().all(|record| record["truncated"] == false));
    assert_eq!(
        records
            .iter()
            .filter(|record| record["record"] == "finding")
            .map(|record| record["ts"].clone())
            .collect::<Vec<_>>(),
        [from.to_string(), to.to_string()].map(Value::from)
    );
}

#[test]
fn process_and_statement_rows_remain_available_without_findings() {
    let mut fixture = Fixture::new();
    fixture.append_finding_rows(&[(100, Some(4_096))], &[(100, 1, 2.5)]);
    fixture.finish();

    for target in [
        format!("/api/segments/{SEGMENT_ID}/sections/os_process/history?field=ts&field=read_bytes"),
        format!(
            "/api/segments/{SEGMENT_ID}/sections/os_process/rows?page_size=10&order=asc&field=ts&field=read_bytes"
        ),
        format!(
            "/api/segments/{SEGMENT_ID}/sections/pg_stat_statements/history?field=ts&field=calls&field=total_exec_time"
        ),
        format!(
            "/api/segments/{SEGMENT_ID}/sections/pg_stat_statements/rows?page_size=10&order=asc&field=ts&field=calls&field=total_exec_time"
        ),
    ] {
        let records = stream(fixture.prepare(&target, None)).expect("raw metric read");
        assert_eq!(row_records(&records).len(), 1, "{target}");
        assert!(
            records.iter().all(|record| {
                !matches!(record["record"].as_str(), Some("findings" | "finding"))
            })
        );
    }
}

#[test]
fn process_summary_series_uses_the_complete_set_and_previous_segment() {
    crate::api::reset_process_summary_operations();
    let mut fixture = Fixture::new();
    fixture.append_process_summary_snapshot(1_000_000, 1_000, None, 900_000, 0..3, None);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_process_summary_snapshot(
        6_000_000,
        1_100,
        Some(0),
        5_500_000,
        0..10,
        Some((6_500_000, 0..205)),
    );
    fixture.finish();

    let fields = [
        "processes",
        "threads",
        "runnable",
        "postgresql",
        "user_cores",
        "system_cores",
        "run_delay_ms_per_second",
        "context_switches_per_second",
        "resident_kib",
        "virtual_kib",
        "swap_kib",
        "major_faults_per_second",
        "read_bytes_per_second",
        "write_bytes_per_second",
        "read_calls_per_second",
        "write_calls_per_second",
    ];
    let query = fields
        .iter()
        .map(|field| format!("field={field}"))
        .collect::<Vec<_>>()
        .join("&");
    let records = stream(fixture.prepare(
        &format!("/api/hour?from=6000000&to=6000000&section=os_process_summary&{query}"),
        None,
    ))
    .expect("process summary history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["timestamp"], "6000000");
    let values = rows[0]["values"].as_array().expect("summary values");
    assert_eq!(values.len(), fields.len());
    assert_eq!(values[0], 205.0, "the server does not use a 200-row page");
    assert_eq!(values[1], 410.0);
    assert_eq!(values[2], 103.0);
    assert_eq!(values[3], 10.0, "a future activity snapshot is not joined");
    assert_eq!(values[4], 41.0, "starttime does not split PID counters");
    assert_eq!(values[5], 82.0);
    assert_eq!(values[6], 4_100.0);
    assert_eq!(values[7], 12_300.0);
    assert_eq!(values[8], 22_960.0);
    assert_eq!(values[9], 25_010.0);
    assert_eq!(values[10], 204.0);
    assert_eq!(values[11], 4_100.0);
    assert_eq!(values[12], 4_100.0);
    assert_eq!(values[13], Value::Null, "all unavailable values stay null");
    assert_eq!(values[14], 4_100.0);
    assert_eq!(values[15], 8_200.0);
    assert_eq!(
        crate::api::process_summary_operations(),
        (4, 2),
        "each segment gets two numeric process passes and one activity pass"
    );
}

#[test]
fn process_snapshot_counter_history_is_pid_only() {
    let mut fixture = Fixture::new();
    fixture.append_process_summary_snapshot(1_000_000, 1_000, None, 900_000, 0..0, None);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_process_summary_snapshot(6_000_000, 1_100, Some(0), 5_500_000, 0..0, None);
    fixture.finish();

    let records = stream(fixture.prepare(
        &format!("/api/segments/{current_segment}/snapshot?at=6000000&section=os_process&field=pid&field=utime&where.pid=0"),
        None,
    ))
    .expect("PID-scoped process snapshot");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([0, 20.0]));
}

#[test]
fn process_summary_rates_across_active_wal_parts() {
    let mut fixture = Fixture::new();
    fixture.append_process_summary_snapshot(1_000_000, 1_000, None, 900_000, 0..3, None);
    fixture.append_process_summary_snapshot(6_000_000, 1_100, None, 5_500_000, 0..3, None);

    let records = stream(fixture.prepare(
        "/api/hour?from=6000000&to=6000000&section=os_process_summary&field=user_cores&field=read_bytes_per_second",
        None,
    ))
    .expect("active process summary history");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!([41.0, 4_100.0]));
}

#[test]
fn temporary_file_rows_remain_available_through_generic_reads() {
    let mut fixture = Fixture::new();
    fixture.append_log_temp_file(100);
    fixture.finish();

    for resource in ["rows?page_size=10&order=asc&", "history?"] {
        let records = stream(fixture.prepare(
            &format!(
                "/api/segments/{SEGMENT_ID}/sections/pg_log_temp_files/{resource}field=ts&field=system_identifier&field=source_file&field=path&field=size_bytes&field=statement"
            ),
            None,
        ))
        .expect("generic temporary-file read");
        let layout = records
            .iter()
            .find(|record| record["record"] == "layout")
            .expect("temporary-file layout");
        assert_eq!(layout["layout"]["type_id"], "2007001");
        let rows = row_records(&records);
        assert_eq!(rows.len(), 1);
        assert_eq!(
            rows[0]["values"],
            serde_json::json!([
                "100",
                "42",
                "postgresql.log",
                "base/pgsql_tmp/pgsql_tmp42.0",
                "1048576",
                "select fixture spill"
            ])
        );
    }
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
fn a_snapshot_orders_by_a_column_and_returns_one_page() {
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
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=major&field=minor&by=minor&page_size=2"
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
    let selection = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("snapshot row counts");
    assert_eq!(selection["eligible"], "3");
    assert_eq!(selection["returned"], "2");
    assert_eq!(selection["truncated"], true);
    assert_eq!(selection["page_size"], 2);
    assert_eq!(selection["has_more"], true);
    assert!(selection["next_cursor"].is_string());
    assert_eq!(selection["order_by"], serde_json::json!(["minor"]));
    assert_eq!(selection["order_direction"], "desc");
    assert_eq!(selection["from"], "100");
    assert_eq!(selection["to"], "200");
}

#[test]
fn a_snapshot_orders_stored_text_lexicographically_and_breaks_ties_by_ordinal() {
    let mut fixture = Fixture::new();
    fixture.append_named_diskstats(&[(0, "beta"), (1, "alpha"), (2, "gamma"), (3, "gamma")]);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&field=device&by=device&page_size=3"
    );
    let records = stream(fixture.prepare(&target, None)).expect("text-ranked snapshot");
    let values = row_records(&records)
        .into_iter()
        .map(|row| row["values"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            serde_json::json!([2, "gamma"]),
            serde_json::json!([3, "gamma"]),
            serde_json::json!([0, "beta"]),
        ]
    );
    let selection = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("snapshot row counts");
    assert_eq!(selection["from"], Value::Null);
    assert_eq!(selection["to"], "100");
}

#[test]
fn a_snapshot_ranks_counter_rates_before_slicing_a_page() {
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
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&by=reads&page_size=2"
    );
    let records = stream(fixture.prepare(&target, None)).expect("counter-ranked snapshot");
    let values = row_records(&records)
        .into_iter()
        .map(|row| row["values"].clone())
        .collect::<Vec<_>>();
    assert_eq!(
        values,
        [
            serde_json::json!([1, 210_000.0]),
            serde_json::json!([2, 60_000.0])
        ]
    );
}

#[test]
fn statement_pages_rank_each_displayed_composite_before_slicing() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();

    for (token, semantic) in [
        ("derived.mean_exec_ms_per_call", "mean_exec_ms_per_call"),
        ("derived.rows_per_call", "rows_per_call"),
        ("derived.blocks_per_call", "blocks_per_call"),
        ("derived.hit_pct", "hit_pct"),
        ("derived.wal_per_call", "wal_per_call"),
        ("derived.plan_time_pct", "plan_time_pct"),
        ("derived.cv", "cv"),
    ] {
        let target = format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=queryid&by={token}&page_size=1"
        );
        let records = stream(fixture.prepare(&target, None)).expect("composite statement page");
        assert_eq!(row_records(&records)[0]["values"], serde_json::json!(["2"]));
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("composite statement trailer");
        assert_eq!(page["eligible"], "3");
        assert_eq!(page["has_more"], true);
        assert_eq!(page["order_by"], serde_json::json!([semantic]));
        assert_eq!(page["order_direction"], "desc");
    }
}

#[test]
fn plan_pages_rank_each_displayed_composite_before_slicing() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_plans();
    fixture.finish();

    for (token, semantic) in [
        ("derived.mean_exec_ms_per_call", "mean_exec_ms_per_call"),
        ("derived.rows_per_call", "rows_per_call"),
        ("derived.blocks_per_call", "blocks_per_call"),
        ("derived.hit_pct", "hit_pct"),
    ] {
        let target = format!(
            "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_store_plans&field=queryid&by={token}&page_size=1"
        );
        let records = stream(fixture.prepare(&target, None)).expect("composite plan page");
        assert_eq!(row_records(&records)[0]["values"], serde_json::json!(["2"]));
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("composite plan trailer");
        assert_eq!(page["eligible"], "3");
        assert_eq!(page["order_by"], serde_json::json!([semantic]));
    }
}

#[test]
fn composite_statement_cursor_keeps_the_exact_global_order() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_statements();
    fixture.finish();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_statements&field=queryid&by=derived.mean_exec_ms_per_call&page_size=1"
    );
    let mut cursor = None;
    let mut queryids = Vec::new();
    loop {
        let target = cursor
            .as_ref()
            .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
        let records = stream(fixture.prepare(&target, None)).expect("composite statement cursor");
        queryids.push(row_records(&records)[0]["values"][0].clone());
        cursor = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .and_then(|page| page["next_cursor"].as_str())
            .map(ToOwned::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(
        queryids,
        [
            serde_json::json!("2"),
            serde_json::json!("1"),
            serde_json::json!("3"),
        ]
    );
}

#[test]
fn snapshot_pages_with_tied_rates_have_no_duplicates_or_omissions() {
    let mut fixture = Fixture::new();
    let mut rows = Vec::new();
    for minor in 0..5 {
        rows.push((100, minor, 10));
        rows.push((200, minor, 11));
    }
    fixture.append_diskstats(&rows);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&by=reads&page_size=2"
    );
    let mut cursor = None;
    let mut seen = Vec::new();
    let mut pages = 0;
    loop {
        let target = cursor
            .as_ref()
            .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
        let records = stream(fixture.prepare(&target, None)).expect("snapshot page");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("page trailer");
        let page_rows = row_records(&records);
        assert_eq!(page["eligible"], "5");
        assert_eq!(page["returned"], page_rows.len().to_string());
        assert_eq!(page["truncated"], page_rows.len() < 5);
        for row in page_rows {
            let ordinal = row["ordinal"].as_str().expect("ordinal").to_owned();
            assert!(!seen.contains(&ordinal), "a cursor must not repeat a row");
            seen.push(ordinal);
        }
        pages += 1;
        cursor = page["next_cursor"].as_str().map(ToOwned::to_owned);
        assert_eq!(page["has_more"], cursor.is_some());
        if cursor.is_none() {
            break;
        }
    }
    assert_eq!(pages, 3);
    assert_eq!(seen, ["1", "3", "5", "7", "9"]);
}

#[test]
fn active_snapshot_cursor_pins_the_original_wal_prefix() {
    let mut fixture = Fixture::new();
    fixture.append_diskstats(&[(100, 0, 1), (100, 1, 1), (100, 2, 1)]);
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&by=minor&page_size=2"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first active page");
    let cursor = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .and_then(|page| page["next_cursor"].as_str())
        .expect("active cursor")
        .to_owned();

    fixture.append_diskstats(&[(100, 3, 1), (100, 4, 1)]);
    let continued = stream(fixture.prepare(&format!("{base}&cursor={cursor}"), None))
        .expect("pinned active page");
    assert_eq!(row_records(&continued)[0]["values"], serde_json::json!([0]));
    let page = continued
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("continued page trailer");
    assert_eq!(page["eligible"], "3");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);

    let fresh = stream(fixture.prepare(&base, None)).expect("fresh active page");
    let page = fresh
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("fresh page trailer");
    assert_eq!(page["eligible"], "5");
}

#[test]
fn exact_large_deltas_resets_and_missing_predecessors_survive_pages() {
    let mut fixture = Fixture::new();
    let base = 1_i64 << 53;
    fixture.append_diskstats(&[
        (100, 0, base),
        (200, 0, base + 1),
        (100, 1, base),
        (200, 1, base + 2),
        (100, 2, 20),
        (200, 2, 10),
        (200, 3, 7),
    ]);
    fixture.finish();

    let base_target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&field=reads&by=reads&page_size=2"
    );
    let first = stream(fixture.prepare(&base_target, None)).expect("first exact page");
    assert_eq!(
        row_records(&first)
            .into_iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [
            serde_json::json!([1, 20_000.0]),
            serde_json::json!([0, 10_000.0]),
        ]
    );
    let cursor = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .and_then(|page| page["next_cursor"].as_str())
        .expect("second page cursor");
    let second = stream(fixture.prepare(&format!("{base_target}&cursor={cursor}"), None))
        .expect("second exact page");
    assert_eq!(
        row_records(&second)
            .into_iter()
            .map(|row| row["values"].clone())
            .collect::<Vec<_>>(),
        [serde_json::json!([2, null]), serde_json::json!([3, null])]
    );
}

#[test]
fn statement_search_finds_a_match_outside_the_old_first_two_hundred() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(205);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&field=query&by=queryid&page_size=200"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first statement page");
    let rows = row_records(&first);
    assert_eq!(rows.len(), 200);
    assert!(rows.iter().all(|row| row["values"][0] != "0"));
    let page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("statement page trailer");
    assert_eq!(page["eligible"], "205");
    assert_eq!(page["has_more"], true);

    let searched = stream(fixture.prepare(&format!("{base}&search=OWNER*BLOCKER%3FNEEDLE"), None))
        .expect("server-side statement search");
    let rows = row_records(&searched);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "0");
    let page = searched
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("search page trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
    assert_eq!(page["truncated"], false);
}

#[test]
fn numeric_statement_page_scans_the_source_once_without_candidate_dictionary_reads() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(205);
    fixture.finish();

    crate::api::reset_page_operations();
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_stat_statements&field=queryid&field=query&field=calls&field=total_exec_time&by=total_exec_time&page_size=200&text=160"
    );
    let records = stream(fixture.prepare(&target, None)).expect("numeric statement page");
    assert_eq!(row_records(&records).len(), 200);
    assert_eq!(crate::api::page_operations(), (1, 0, 0));
}

#[test]
fn statement_page_composes_a_snapshot_split_across_segments() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 100.0), (100, 2, 10, 100.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_statement_snapshots(&[(200, 1, 20, 300.0)]);
    let current_segment = SEGMENT_ID + 2_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_statement_snapshots(&[(200, 2, 20, 200.0)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_stat_statements&field=queryid&field=calls&field=total_exec_time&by=derived.mean_exec_ms_per_call&page_size=1"
    );
    let first = stream(fixture.prepare(&target, None)).expect("split statement page");
    let rows = row_records(&first);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "1");
    assert_eq!(rows[0]["values"][1], 100_000.0);
    assert_eq!(rows[0]["values"][2], 2_000_000.0);
    let page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("split statement page trailer");
    assert_eq!(page["eligible"], "2");
    assert_eq!(page["from"], "100");
    assert_eq!(page["to"], "200");
    assert_eq!(page["has_more"], true);

    let cursor = page["next_cursor"].as_str().expect("split page cursor");
    let second = stream(fixture.prepare(&format!("{target}&cursor={cursor}"), None))
        .expect("second split statement page");
    let rows = row_records(&second);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "2");
    assert_eq!(rows[0]["values"][1], 100_000.0);
    assert_eq!(rows[0]["values"][2], 1_000_000.0);
}

#[test]
fn statement_page_composes_every_segment_at_both_selected_moments() {
    let mut fixture = Fixture::new();
    fixture.append_statement_snapshots(&[(100, 1, 10, 10.0), (100, 2, 20, 20.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_statement_snapshots(&[(100, 3, 30, 30.0), (100, 4, 40, 40.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_statement_snapshots(&[(200, 1, 20, 50.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 3_000);
    fixture.append_statement_snapshots(&[(200, 2, 30, 80.0)]);
    fixture.finish_and_continue(SEGMENT_ID + 4_000);
    fixture.append_statement_snapshots(&[(200, 3, 40, 120.0)]);
    let current_segment = SEGMENT_ID + 5_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_statement_snapshots(&[(200, 4, 50, 170.0)]);
    fixture.finish();

    let base = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_stat_statements&field=queryid&field=query&field=calls&field=total_exec_time&by=total_exec_time&page_size=2&search=BOUNDARY*"
    );
    let mut cursor = None;
    let mut rows = Vec::new();
    let mut pages = 0;
    loop {
        let target = cursor
            .as_ref()
            .map_or_else(|| base.clone(), |cursor| format!("{base}&cursor={cursor}"));
        let records = stream(fixture.prepare(&target, None)).expect("composed statement page");
        let page = records
            .iter()
            .find(|record| record["record"] == "snapshot_page")
            .expect("composed statement page trailer");
        assert_eq!(page["eligible"], "4");
        assert_eq!(page["returned"], "2");
        assert_eq!(page["from"], "100");
        assert_eq!(page["to"], "200");
        rows.extend(row_records(&records).into_iter().cloned());
        pages += 1;
        cursor = page["next_cursor"].as_str().map(ToOwned::to_owned);
        assert_eq!(page["has_more"], cursor.is_some());
        if cursor.is_none() {
            break;
        }
    }

    assert_eq!(pages, 2);
    assert_eq!(
        rows.iter()
            .map(|row| (
                row["segment_id"]
                    .as_str()
                    .expect("physical segment")
                    .to_owned(),
                row["values"].clone(),
            ))
            .collect::<Vec<_>>(),
        [
            (
                (SEGMENT_ID + 5_000).to_string(),
                serde_json::json!(["4", "boundary statement 4", 100_000.0, 1_300_000.0]),
            ),
            (
                (SEGMENT_ID + 4_000).to_string(),
                serde_json::json!(["3", "boundary statement 3", 100_000.0, 900_000.0]),
            ),
            (
                (SEGMENT_ID + 3_000).to_string(),
                serde_json::json!(["2", "boundary statement 2", 100_000.0, 600_000.0]),
            ),
            (
                (SEGMENT_ID + 2_000).to_string(),
                serde_json::json!(["1", "boundary statement 1", 100_000.0, 400_000.0]),
            ),
        ]
    );
}

#[test]
fn plan_page_keeps_zero_and_rejects_a_cross_segment_counter_decrease() {
    let mut fixture = Fixture::new();
    fixture.append_plan_snapshots(&[(100, 1, 4, 40.0), (100, 2, 10, 100.0)]);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_plan_snapshots(&[(200, 1, 4, 40.0), (200, 2, 5, 50.0)]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=200&section=pg_store_plans&field=queryid&field=calls&field=total_time&by=queryid&page_size=200"
    );
    let records = stream(fixture.prepare(&target, None)).expect("cross-segment plan page");
    let rows = row_records(&records);
    assert_eq!(rows.len(), 2);
    let rows = rows
        .into_iter()
        .map(|row| (row["values"][0].as_str().expect("queryid"), row))
        .collect::<BTreeMap<_, _>>();
    assert_eq!(rows["1"]["values"][1], 0.0);
    assert_eq!(rows["1"]["values"][2], 0.0);
    assert_eq!(rows["2"]["values"][1], Value::Null);
    assert_eq!(rows["2"]["values"][2], Value::Null);
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("cross-segment plan page trailer");
    assert_eq!(page["from"], "100");
    assert_eq!(page["to"], "200");
}

#[test]
fn plan_search_finds_a_match_outside_the_old_first_two_hundred() {
    let mut fixture = Fixture::new();
    fixture.append_plan_universe(205);
    fixture.finish();
    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=pg_store_plans&field=queryid&field=plan&by=queryid&page_size=200"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first plan page");
    assert_eq!(row_records(&first).len(), 200);
    assert!(
        row_records(&first)
            .iter()
            .all(|row| row["values"][0] != "0")
    );

    let searched = stream(fixture.prepare(&format!("{base}&search=PLAN%3FNEEDLE"), None))
        .expect("server-side plan search");
    let rows = row_records(&searched);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"][0], "0");
    let page = searched
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("plan search trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
}

#[test]
fn snapshot_cursor_rejects_every_bound_query_shape_mismatch() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(5);
    fixture.finish();
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    let shape = "at=100&section=pg_stat_statements&field=queryid&field=query&by=queryid&by=userid&page_size=1&search=fixture&search=statement&text=80&where.dbid=73&where.userid=72&type_id=1002002";
    let first = stream(fixture.prepare(&format!("{path}?{shape}"), None)).expect("bound page");
    let cursor = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .and_then(|page| page["next_cursor"].as_str())
        .expect("bound cursor");

    let mismatches = [
        (SEGMENT_ID + 1, shape.to_owned()),
        (SEGMENT_ID, shape.replacen("at=100", "at=99", 1)),
        (
            SEGMENT_ID,
            shape.replacen("section=pg_stat_statements", "section=os_diskstats", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen("field=queryid&field=query", "field=query&field=queryid", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen("by=queryid&by=userid", "by=userid&by=queryid", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen(
                "search=fixture&search=statement",
                "search=statement&search=fixture",
                1,
            ),
        ),
        (SEGMENT_ID, shape.replacen("text=80", "text=81", 1)),
        (
            SEGMENT_ID,
            shape.replacen("where.dbid=73", "where.dbid=74", 1),
        ),
        (
            SEGMENT_ID,
            shape.replacen(
                "where.dbid=73&where.userid=72",
                "where.userid=72&where.dbid=73",
                1,
            ),
        ),
        (
            SEGMENT_ID,
            shape.replacen("type_id=1002002", "type_id=1002001", 1),
        ),
    ];
    for (segment_id, query) in mismatches {
        let path = format!("/api/segments/{segment_id}/snapshot");
        let query = format!("{query}&cursor={cursor}");
        let route = crate::route::parse(&path, Some(&query)).expect("mismatch route");
        assert!(matches!(
            crate::api::prepare(fixture.root(), SOURCES, route, None),
            Err(ApiError::BadCursor)
        ));
    }

    let compatible_size = shape.replacen("page_size=1", "page_size=4", 1);
    let query = format!("{compatible_size}&cursor={cursor}");
    let route = crate::route::parse(&path, Some(&query)).expect("compatible route");
    assert!(crate::api::prepare(fixture.root(), SOURCES, route, None).is_ok());
}

#[test]
fn snapshot_search_uses_the_fixed_section_allowlist_outside_the_projection() {
    let mut fixture = Fixture::new();
    fixture.append_statement_universe(1);
    fixture.append_diskstats(&[(100, 0, 1)]);
    fixture.finish();
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    for query in [
        "at=100&section=pg_stat_statements&field=calls&search=needle",
        "at=100&section=pg_stat_statements&search=needle",
    ] {
        let route = crate::route::parse(&path, Some(query)).expect("search route");
        assert!(crate::api::prepare(fixture.root(), SOURCES, route, None).is_ok());
    }
    let route = crate::route::parse(
        &path,
        Some("at=100&section=os_diskstats&field=device&search=needle"),
    )
    .expect("unsupported search route");
    assert!(matches!(
        crate::api::prepare(fixture.root(), SOURCES, route, None),
        Err(ApiError::BadFilter(parameter)) if parameter == "search"
    ));
}

#[test]
fn snapshot_resolves_text_only_after_page_rows_are_selected() {
    let mut fixture = Fixture::new();
    fixture.append_ranked_diskstats_with_unreadable_loser();
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&field=device&by=minor&page_size=1"
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
fn an_exact_source_pointer_must_name_a_captured_row_at_its_timestamp() {
    let mut active = Fixture::new();
    active.append_diskstats(&[(200, 0, 2)]);
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0"
    );
    let records = stream(active.prepare(&target, None)).expect("captured active row");
    assert_eq!(row_records(&records)[0]["values"], serde_json::json!([0]));

    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    let route = crate::route::parse(
        &path,
        Some("at=199&section=os_diskstats&field=minor&type_id=1108001&row_ordinal=0"),
    )
    .expect("exact route");
    assert!(matches!(
        crate::api::prepare(active.root(), SOURCES, route, None),
        Err(ApiError::BadCursor)
    ));

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
fn a_cgroup_snapshot_applies_the_exact_path_and_scope_filters() {
    const ROWS: usize = 50_000;
    let mut fixture = Fixture::new();
    fixture.append_large_cgroup_cpu(ROWS, ROWS / 2);
    fixture.finish();

    reset_context_operations();
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=os_cgroup_cpu&field=cgroup_path&field=scope&where.cgroup_path=%2Fcollector&where.scope=3"
    );
    let records = stream(fixture.prepare(&target, None)).expect("filtered cgroup snapshot");
    let rows = row_records(&records);

    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"], serde_json::json!(["/collector", 3]));
    let (maximum_chunk, staged, selection_dictionaries) = context_operations();
    assert_eq!(maximum_chunk, 16);
    assert_eq!(staged, 1);
    assert_eq!(selection_dictionaries, 1);
}

#[test]
fn a_snapshot_rejects_a_dangling_exact_filter_id() {
    let mut fixture = Fixture::new();
    let wanted = kronika_format::StrId::of(b"sda")
        .expect("fixture filter id is nonzero")
        .get();
    let mut buffers = SectionBuffers::new();
    buffers
        .push(diskstats_with_device(100, 0, 1, StrId(wanted)))
        .expect("dangling filter row fits");
    fixture.append(buffers);

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=100&section=os_diskstats&field=minor&where.device=sda"
    );
    let error = fixture
        .prepare(&target, None)
        .stream(&mut |_record| true, &|| false)
        .expect_err("dangling requested filter id must fail");

    assert!(matches!(error, ApiError::Unreadable(_)));
    assert!(
        error
            .to_string()
            .contains(&format!("unresolved dictionary id {wanted}"))
    );
}

#[test]
fn table_snapshot_pages_use_each_database_moments_and_elapsed_time() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(
        &[
            (10_000_000, 1, 77, 10),
            (30_000_000, 1, 77, 30),
            (20_000_000, 2, 77, 5),
            (25_000_000, 2, 77, 15),
        ],
        &[],
        &[],
    );
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_tables&field=datid&field=relid&field=seq_scan&by=seq_scan&page_size=1"
    );
    let first = stream(fixture.prepare(&base, None)).expect("first table page");
    assert_eq!(
        row_records(&first)[0]["values"],
        serde_json::json!([2, 77, 2.0])
    );
    let first_page = first
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("first table page trailer");
    assert_eq!(first_page["eligible"], "2");
    assert_eq!(first_page["from"], "10000000");
    assert_eq!(first_page["to"], "30000000");
    let cursor = first_page["next_cursor"].as_str().expect("table cursor");

    let second = stream(fixture.prepare(&format!("{base}&cursor={cursor}"), None))
        .expect("second table page");
    assert_eq!(
        row_records(&second)[0]["values"],
        serde_json::json!([1, 77, 1.0])
    );
    let second_page = second
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("second table page trailer");
    assert_eq!(second_page["eligible"], "2");
    assert_eq!(second_page["has_more"], false);
}

#[test]
fn relation_table_buffer_rates_distinguish_values_zero_and_missing_predecessors() {
    let mut fixture = Fixture::new();
    fixture.append_buffered_table_snapshots(&[
        (10_000_000, 1, 77, [100, 900, 50, 450, 10, 90, 5, 45]),
        (20_000_000, 1, 77, [110, 990, 55, 495, 11, 99, 6, 54]),
        (10_000_000, 2, 78, [8, 16, 4, 12, 2, 6, 1, 3]),
        (20_000_000, 2, 78, [8, 16, 4, 12, 2, 6, 1, 3]),
        (20_000_000, 3, 79, [10, 90, 5, 45, 1, 9, 1, 9]),
    ]);
    fixture.finish();

    let fields = [
        "heap_blks_read",
        "heap_blks_hit",
        "idx_blks_read",
        "idx_blks_hit",
        "toast_blks_read",
        "toast_blks_hit",
        "tidx_blks_read",
        "tidx_blks_hit",
        "heap_buffer_hit_pct",
        "index_buffer_hit_pct",
        "toast_buffer_hit_pct",
        "tidx_buffer_hit_pct",
        "buffer_hit_pct",
    ]
    .map(|field| format!("field={field}"))
    .join("&");
    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&group=object&{fields}"
    );
    let records = stream(fixture.prepare(&target, None)).expect("buffer relation snapshot");
    let rows = relation_records(&records)
        .into_iter()
        .map(|row| (row["key"]["datid"].as_str().unwrap().to_owned(), row))
        .collect::<BTreeMap<_, _>>();

    let assert_rate = |field: &str, expected: f64| {
        let actual = rows["1"]["values"][field]
            .as_f64()
            .expect("numeric buffer rate");
        assert!((actual - expected).abs() < 1e-12, "{field}: {actual}");
    };
    for (field, expected) in [
        ("heap_blks_read", 1.0),
        ("heap_blks_hit", 9.0),
        ("idx_blks_read", 0.5),
        ("idx_blks_hit", 4.5),
        ("toast_blks_read", 0.1),
        ("toast_blks_hit", 0.9),
        ("tidx_blks_read", 0.1),
        ("tidx_blks_hit", 0.9),
        ("heap_buffer_hit_pct", 90.0),
        ("index_buffer_hit_pct", 90.0),
        ("toast_buffer_hit_pct", 90.0),
        ("tidx_buffer_hit_pct", 90.0),
        ("buffer_hit_pct", 90.0),
    ] {
        assert_rate(field, expected);
    }

    for field in fields
        .split('&')
        .map(|field| field.trim_start_matches("field="))
    {
        if !field.ends_with("_pct") {
            assert_eq!(rows["2"]["values"][field], 0.0, "true zero {field}");
            assert_eq!(
                rows["3"]["values"][field],
                Value::Null,
                "missing predecessor {field}"
            );
        }
    }
    for field in [
        "heap_buffer_hit_pct",
        "index_buffer_hit_pct",
        "toast_buffer_hit_pct",
        "tidx_buffer_hit_pct",
        "buffer_hit_pct",
    ] {
        assert_eq!(rows["2"]["values"][field], Value::Null);
        assert_eq!(rows["3"]["values"][field], Value::Null);
    }

    assert_exact_buffer_rows(&fixture);
}

fn assert_exact_buffer_rows(fixture: &Fixture) {
    let exact = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&field=datid&field=heap_blks_read&field=heap_blks_hit&type_id=1013001&row_ordinal=1"
    );
    let exact = stream(fixture.prepare(&exact, None)).expect("exact partitioned buffer row");
    assert_eq!(
        row_records(&exact)[0]["values"],
        serde_json::json!([1, 1.0, 9.0]),
        "the exact row uses its own database predecessor"
    );

    let exact_zero = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&field=datid&field=heap_blks_read&type_id=1013001&row_ordinal=3"
    );
    let exact_zero = stream(fixture.prepare(&exact_zero, None)).expect("exact zero buffer row");
    assert_eq!(
        row_records(&exact_zero)[0]["values"],
        serde_json::json!([2, 0.0])
    );

    let exact_missing = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&field=datid&field=heap_blks_read&type_id=1013001&row_ordinal=4"
    );
    let exact_missing =
        stream(fixture.prepare(&exact_missing, None)).expect("exact missing predecessor row");
    assert_eq!(
        row_records(&exact_missing)[0]["values"],
        serde_json::json!([3, null])
    );

    let version_absent = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=20000000&section=pg_stat_user_tables&group=object&field=n_tup_newpage_upd&where.datid=1"
    );
    let version_absent =
        stream(fixture.prepare(&version_absent, None)).expect("version-absent relation field");
    assert_eq!(
        relation_records(&version_absent)[0]["values"]["n_tup_newpage_upd"],
        Value::Null,
        "a field absent from the physical layout is not a numeric zero"
    );
}

#[test]
fn relation_predecessor_skips_sectionless_segments_and_overlapping_bounds() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(
        100_000_000,
        1,
        77,
        10,
        "fixture_db",
        "public",
        "orders",
    )]);
    fixture.append_diskstats(&[(400_000_000, 0, 1)]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_diskstats(&[(200_000_000, 0, 2)]);
    let current_segment = SEGMENT_ID + 2_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_named_table_snapshots(&[(
        300_000_000,
        1,
        77,
        30,
        "fixture_db",
        "public",
        "orders",
    )]);
    fixture.finish();

    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=300000000&section=pg_stat_user_tables&group=object&field=seq_scan"
    );
    let records = stream(fixture.prepare(&target, None)).expect("cross-segment relation snapshot");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["seq_scan"], 0.1);
    assert_eq!(rows[0]["sample_from"], "100000000");
    assert_eq!(rows[0]["sample_to"], "300000000");
}

#[test]
fn relation_predecessor_follows_a_fallback_sample_into_the_second_segment() {
    let mut fixture = Fixture::new();
    fixture.append_buffered_table_snapshots(&[(
        10_000_000,
        1,
        77,
        [100, 900, 50, 450, 10, 90, 5, 45],
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_buffered_table_snapshots(&[(
        20_000_000,
        1,
        77,
        [110, 990, 55, 495, 11, 99, 6, 54],
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_diskstats(&[(25_000_000, 0, 1)]);
    fixture.append_buffered_table_snapshots(&[(
        35_000_000,
        1,
        77,
        [120, 1_080, 60, 540, 12, 108, 7, 63],
    )]);
    let current_segment = SEGMENT_ID + 3_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_buffered_table_snapshots(&[(
        40_000_000,
        1,
        77,
        [120, 1_080, 60, 540, 12, 108, 7, 63],
    )]);
    fixture.finish();

    let fields = [
        "heap_blks_read",
        "heap_blks_hit",
        "idx_blks_read",
        "idx_blks_hit",
        "toast_blks_read",
        "toast_blks_hit",
        "tidx_blks_read",
        "tidx_blks_hit",
        "buffer_hit_pct",
    ]
    .map(|field| format!("field={field}"))
    .join("&");
    let target = format!(
        "/api/segments/{current_segment}/snapshot?at=30000000&section=pg_stat_user_tables&group=object&{fields}"
    );
    let records = stream(fixture.prepare(&target, None)).expect("fallback relation snapshot");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["sample_from"], "10000000");
    assert_eq!(rows[0]["sample_to"], "20000000");
    for (field, expected) in [
        ("heap_blks_read", 1.0),
        ("heap_blks_hit", 9.0),
        ("idx_blks_read", 0.5),
        ("idx_blks_hit", 4.5),
        ("toast_blks_read", 0.1),
        ("toast_blks_hit", 0.9),
        ("tidx_blks_read", 0.1),
        ("tidx_blks_hit", 0.9),
        ("buffer_hit_pct", 90.0),
    ] {
        let actual = rows[0]["values"][field]
            .as_f64()
            .expect("numeric buffer rate");
        assert!((actual - expected).abs() < 1e-12, "{field}: {actual}");
    }
}

fn four_segment_relation_fixture() -> (Fixture, i64) {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[(100_000_000, 1, 11, 10, "first", "public", "orders")]);
    fixture.append_named_index_snapshots(&[(
        100_000_000,
        1,
        101,
        20,
        "first",
        "public",
        "orders",
        "orders_pkey",
        "CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)",
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_named_table_snapshots(&[(120_000_000, 2, 21, 5, "second", "public", "events")]);
    fixture.append_named_index_snapshots(&[(
        120_000_000,
        2,
        201,
        7,
        "second",
        "public",
        "events",
        "events_pkey",
        "CREATE UNIQUE INDEX events_pkey ON public.events USING btree (id)",
    )]);
    fixture.finish_and_continue(SEGMENT_ID + 2_000);
    fixture.append_named_table_snapshots(&[(180_000_000, 2, 21, 17, "second", "public", "events")]);
    fixture.append_named_index_snapshots(&[(
        180_000_000,
        2,
        201,
        19,
        "second",
        "public",
        "events",
        "events_pkey",
        "CREATE UNIQUE INDEX events_pkey ON public.events USING btree (id)",
    )]);
    let current_segment = SEGMENT_ID + 3_000;
    fixture.finish_and_continue(current_segment);
    fixture.append_named_table_snapshots(&[(200_000_000, 1, 11, 30, "first", "public", "orders")]);
    fixture.append_named_index_snapshots(&[(
        200_000_000,
        1,
        101,
        50,
        "first",
        "public",
        "orders",
        "orders_pkey",
        "CREATE UNIQUE INDEX orders_pkey ON public.orders USING btree (id)",
    )]);
    fixture.finish();
    (fixture, current_segment)
}

#[test]
fn relation_object_snapshots_keep_each_database_predecessor_across_segments() {
    let (fixture, current_segment) = four_segment_relation_fixture();

    let tables = stream(fixture.prepare(
        &format!(
            "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_tables&group=object&field=seq_scan"
        ),
        None,
    ))
    .expect("four-segment table snapshot");
    let table_rates = relation_records(&tables)
        .into_iter()
        .map(|row| {
            (
                row["key"]["datid"].as_str().unwrap().to_owned(),
                row["values"]["seq_scan"].clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(table_rates["1"], serde_json::json!(0.2));
    assert_eq!(table_rates["2"], serde_json::json!(0.2));

    let first_database = stream(fixture.prepare(
        &format!(
            "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_tables&group=object&field=seq_scan&where.datid=1"
        ),
        None,
    ))
    .expect("filtered four-segment table snapshot");
    let rows = relation_records(&first_database);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["datid"], "1");
    assert_eq!(rows[0]["values"]["seq_scan"], serde_json::json!(0.2));

    let indexes = stream(fixture.prepare(
        &format!(
            "/api/segments/{current_segment}/snapshot?at=200000000&section=pg_stat_user_indexes&group=object&field=idx_scan"
        ),
        None,
    ))
    .expect("four-segment index snapshot");
    let index_rates = relation_records(&indexes)
        .into_iter()
        .map(|row| {
            (
                row["key"]["datid"].as_str().unwrap().to_owned(),
                row["values"]["idx_scan"].clone(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(index_rates["1"], serde_json::json!(0.3));
    assert_eq!(index_rates["2"], serde_json::json!(0.2));
}

#[test]
fn relation_group_history_finds_the_requested_database_predecessor() {
    let (fixture, _current_segment) = four_segment_relation_fixture();

    let records = stream(fixture.prepare(
        "/api/hour?from=200000000&to=200000000&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=1",
        None,
    ))
    .expect("four-segment database history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["datid"], "1");
    assert_eq!(rows[0]["values"]["seq_scan"], serde_json::json!(0.2));
    assert_eq!(rows[0]["sample_from"], "100000000");
    assert_eq!(rows[0]["sample_to"], "200000000");
}

#[test]
fn index_snapshot_never_borrows_a_database_or_layout_predecessor() {
    let mut fixture = Fixture::new();
    fixture.append_relation_snapshots(
        &[],
        &[
            (10_000_000, 1, 88, 100),
            (30_000_000, 1, 88, 50),
            (20_000_000, 2, 88, 10),
            (25_000_000, 2, 88, 20),
            (27_000_000, 3, 88, 100),
            (10_000_000, 4, 88, 1),
        ],
        &[(30_000_000, 4, 88, 11)],
    );
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_indexes&field=datid&field=indexrelid&field=idx_scan"
    );
    let records = stream(fixture.prepare(&target, None)).expect("index snapshot");
    let rows = row_records(&records)
        .into_iter()
        .map(|row| {
            (
                row["values"][0].as_u64().expect("database oid"),
                (row["type_id"].clone(), row["values"][2].clone()),
            )
        })
        .collect::<BTreeMap<_, _>>();
    assert_eq!(rows.len(), 4);
    assert_eq!(rows[&1].1, Value::Null, "a reset has no rate");
    assert_eq!(rows[&2].1, serde_json::json!(2.0));
    assert_eq!(rows[&3].1, Value::Null, "a missing predecessor stays null");
    assert_eq!(
        rows[&4],
        (
            PgStatUserIndexesV2::CONTRACT
                .type_id
                .get()
                .to_string()
                .into(),
            Value::Null,
        ),
        "an older physical layout is not used as the predecessor"
    );
}

#[test]
fn relation_groups_keep_database_scope_and_sum_staggered_rates() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[
        (10_000_000, 1, 11, 10, "first", "public", "orders"),
        (30_000_000, 1, 11, 30, "first", "public", "orders"),
        (20_000_000, 2, 21, 5, "second", "public", "orders"),
        (25_000_000, 2, 21, 15, "second", "public", "orders"),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=30000000&section=pg_stat_user_tables&field=table_count&field=seq_scan&by=seq_scan&direction=desc"
    );
    let databases =
        stream(fixture.prepare(&format!("{base}&group=database"), None)).expect("database groups");
    let rows = relation_records(&databases);
    assert_eq!(
        rows.len(),
        2,
        "the root has database rows, not a global total"
    );
    assert_eq!(rows[0]["key"]["datid"], "2");
    assert_eq!(rows[0]["values"]["table_count"], "1");
    assert_eq!(rows[0]["values"]["seq_scan"], 2.0);
    assert!(rows.iter().all(|row| row["source"].is_null()));

    let schemas =
        stream(fixture.prepare(&format!("{base}&group=schema"), None)).expect("schema groups");
    let rows = relation_records(&schemas);
    assert_eq!(rows.len(), 2, "same-named schemas stay database-scoped");
    assert!(rows.iter().all(|row| row["key"]["schemaname"] == "public"));

    let objects =
        stream(fixture.prepare(&format!("{base}&group=object"), None)).expect("object groups");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["key"]["relid"], "21");
    assert_eq!(rows[0]["key"]["relname"], "orders");
    assert!(rows[0]["source"]["ordinal"].is_string());
}

#[test]
fn relation_group_history_reuses_exact_reducers_across_segments_and_the_full_set() {
    let mut fixture = Fixture::new();
    let mut previous = (1..=205)
        .map(|relid| (100, 1, relid, [0, 0, 0, 0], "db", "public", "table"))
        .collect::<Vec<_>>();
    previous.push((100, 2, 1, [0, 0, 0, 0], "other", "public", "table"));
    fixture.append_dml_table_snapshots(&previous);
    let current_segment = SEGMENT_ID + 1_000;
    fixture.finish_and_continue(current_segment);

    let mut current = (1..=205)
        .map(|relid| {
            let counters = if relid == 1 {
                [10, 0, 0, 0]
            } else {
                [0, 1, 0, 1]
            };
            (200, 1, relid, counters, "db", "public", "table")
        })
        .collect::<Vec<_>>();
    current.push((200, 2, 1, [9_999, 0, 0, 0], "other", "public", "table"));
    fixture.append_dml_table_snapshots(&current);
    fixture.finish();

    crate::api::reset_history_operations();
    let target = "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=schema&field=table_count&field=dml_total&field=insert_share_pct&where.datid=1&where.schemaname=public";
    let records = stream(fixture.prepare(target, None)).expect("grouped relation history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["datid"], "1");
    assert_eq!(rows[0]["key"]["schemaname"], "public");
    assert_eq!(rows[0]["values"]["table_count"], "205");
    assert_eq!(rows[0]["values"]["dml_total"], 2_140_000.0);
    let insert_share = rows[0]["values"]["insert_share_pct"]
        .as_f64()
        .expect("insert share");
    assert!((insert_share - 500.0 / 107.0).abs() < 1e-12);
    assert_eq!(rows[0]["sample_from"], "100");
    assert_eq!(rows[0]["sample_to"], "200");
    assert!(rows[0]["source"].is_null());
    assert_eq!(
        crate::api::history_operations(),
        (2, 2),
        "one selection and one source visit per physical layout and segment",
    );

    crate::api::reset_history_operations();
    let one_metric = "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=dml_total&where.datid=1";
    let records = stream(fixture.prepare(one_metric, None)).expect("single-metric grouped history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["dml_total"], 2_140_000.0);
    assert_eq!(
        crate::api::history_operations(),
        (2, 2),
        "requested metric count must not multiply physical source visits",
    );
}

#[test]
fn relation_group_history_keeps_zero_reset_and_missing_predecessor_distinct() {
    let mut fixture = Fixture::new();
    fixture.append_named_table_snapshots(&[
        (100, 1, 11, 10, "db", "steady", "steady_table"),
        (100, 1, 12, 20, "db", "reset", "reset_table"),
    ]);
    fixture.finish_and_continue(SEGMENT_ID + 1_000);
    fixture.append_named_table_snapshots(&[
        (200, 1, 11, 10, "db", "steady", "steady_table"),
        (200, 1, 12, 5, "db", "reset", "reset_table"),
        (200, 1, 13, 1, "db", "new", "new_table"),
    ]);
    fixture.finish();

    let load_schema = |schema: &str| {
        let target = format!(
            "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=schema&field=seq_scan&where.datid=1&where.schemaname={schema}"
        );
        let records = stream(fixture.prepare(&target, None)).expect("schema history");
        relation_records(&records)[0]["values"]["seq_scan"].clone()
    };
    assert_eq!(load_schema("steady"), serde_json::json!(0.0));
    assert_eq!(load_schema("reset"), Value::Null);
    assert_eq!(load_schema("new"), Value::Null);

    let database = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=seq_scan&where.datid=1",
        None,
    ))
    .expect("database history");
    assert_eq!(
        relation_records(&database)[0]["values"]["seq_scan"],
        Value::Null,
        "one reset or missing object predecessor makes the exact database aggregate unavailable",
    );
}

#[test]
fn active_wal_relation_history_handles_snapshot_ordered_parts() {
    let mut fixture = Fixture::new();
    fixture.append_dml_table_snapshots(&[
        (100, 1, 11, [0, 0, 0, 0], "db", "public", "first"),
        (100, 1, 12, [0, 0, 0, 0], "db", "public", "second"),
    ]);
    fixture.append_dml_table_snapshots(&[
        (200, 1, 11, [3, 0, 0, 0], "db", "public", "first"),
        (200, 1, 12, [0, 7, 0, 7], "db", "public", "second"),
    ]);

    let records = stream(fixture.prepare(
        "/api/hour?from=200&to=200&section=pg_stat_user_tables&group=database&field=table_count&field=dml_total&field=insert_share_pct&where.datid=1",
        None,
    ))
    .expect("active WAL grouped history");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["table_count"], "2");
    assert_eq!(rows[0]["values"]["dml_total"], 100_000.0);
    assert_eq!(rows[0]["values"]["insert_share_pct"], 30.0);
    assert_eq!(rows[0]["sample_from"], "100");
    assert_eq!(rows[0]["sample_to"], "200");
}

#[test]
fn relation_derivatives_sort_the_full_set_and_recompute_group_ratios() {
    let mut fixture = Fixture::new();
    fixture.append_dml_table_snapshots(&[
        (100, 1, 11, [0, 0, 0, 0], "db", "public", "small"),
        (100, 1, 12, [0, 0, 0, 0], "db", "public", "mostly_insert"),
        (100, 1, 13, [0, 0, 0, 0], "db", "public", "mostly_update"),
        (200, 1, 11, [1, 0, 0, 0], "db", "public", "small"),
        (200, 1, 12, [9, 1, 0, 1], "db", "public", "mostly_insert"),
        (200, 1, 13, [0, 90, 0, 90], "db", "public", "mostly_update"),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&field=dml_total&field=insert_share_pct&by=derived.dml_total&direction=desc"
    );
    let objects = stream(fixture.prepare(&format!("{base}&group=object&page_size=1"), None))
        .expect("derived relation page");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["relid"], "13");
    assert_eq!(rows[0]["values"]["dml_total"], 900_000.0);
    let page = objects
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("derived page trailer");
    assert_eq!(page["eligible"], "3");
    assert_eq!(page["has_more"], true);

    let databases = stream(fixture.prepare(&format!("{base}&group=database"), None))
        .expect("derived database group");
    let rows = relation_records(&databases);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["dml_total"], 1_010_000.0);
    let share = rows[0]["values"]["insert_share_pct"]
        .as_f64()
        .expect("aggregate insert share");
    assert!((share - 1_000.0 / 101.0).abs() < 1e-12);
}

#[test]
fn relation_search_runs_before_group_sort_and_page() {
    let mut fixture = Fixture::new();
    let mut rows = Vec::new();
    for relid in 1..=205 {
        let name = if relid == 1 {
            "needle_outside_unfiltered_page"
        } else {
            "ordinary"
        };
        rows.push((100, 1, relid, 0, "db", "public", name));
        rows.push((200, 1, relid, i64::from(relid), "db", "public", name));
    }
    fixture.append_named_table_snapshots(&rows);
    fixture.finish();

    let target = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_tables&group=object&field=seq_scan&by=seq_scan&page_size=200&search=needle_outside"
    );
    let records = stream(fixture.prepare(&target, None)).expect("searched relation page");
    let rows = relation_records(&records);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["relid"], "1");
    let page = records
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("relation trailer");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["returned"], "1");
    assert_eq!(page["has_more"], false);
}

#[test]
fn index_definition_is_full_search_input_but_never_an_aggregate_value() {
    let mut fixture = Fixture::new();
    let current_segment = SEGMENT_ID + 1_000;
    let definition = "CREATE UNIQUE INDEX exact_fixture_idx ON public.orders USING btree (tenant_id, created_at) WHERE archived_at IS NULL";
    fixture.append_named_index_snapshots(&[
        (
            100,
            1,
            51,
            0,
            "db",
            "public",
            "orders",
            "exact_fixture_idx",
            definition,
        ),
        (
            200,
            1,
            51,
            3,
            "db",
            "public",
            "orders",
            "exact_fixture_idx",
            definition,
        ),
    ]);
    fixture.finish_and_continue(current_segment);
    fixture.append_named_index_snapshots(&[(
        900,
        1,
        51,
        9,
        "db",
        "public",
        "orders",
        "exact_fixture_idx",
        definition,
    )]);
    fixture.finish();

    let aggregate_target = format!(
        "/api/segments/{current_segment}/snapshot?at=400&section=pg_stat_user_indexes&group=database&field=index_count&search=archived_at"
    );
    let aggregate = stream(fixture.prepare(&aggregate_target, None)).expect("definition search");
    let rows = relation_records(&aggregate);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["index_count"], "1");
    assert!(rows[0]["values"].get("indexdef").is_none());
    assert!(rows[0]["source"].is_null());

    let object_target = format!(
        "/api/segments/{current_segment}/snapshot?at=400&section=pg_stat_user_indexes&group=object&field=indexrelname&field=relid&field=relname&field=idx_scan&search=archived_at"
    );
    let object = stream(fixture.prepare(&object_target, None)).expect("index object");
    let rows = relation_records(&object);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["indexrelname"], "exact_fixture_idx");
    assert_eq!(rows[0]["key"]["relid"], "50");
    assert!(rows[0]["values"].get("indexdef").is_none());

    let source = rows[0]["source"].as_object().expect("physical source");
    assert_eq!(source["segment_id"], SEGMENT_ID.to_string());
    let detail_target = format!(
        "/api/segments/{}/snapshot?at={}&section=pg_stat_user_indexes&field=indexdef&type_id={}&row_ordinal={}&text=65536",
        source["segment_id"].as_str().unwrap(),
        source["timestamp"].as_str().unwrap(),
        source["type_id"].as_str().unwrap(),
        source["ordinal"].as_str().unwrap(),
    );
    let detail = stream(fixture.prepare(&detail_target, None)).expect("exact definition detail");
    assert_eq!(row_records(&detail)[0]["values"][0], definition);
}

#[test]
fn low_activity_filters_exact_object_deltas_before_grouping_and_paging() {
    let mut fixture = Fixture::new();
    fixture.append_named_index_snapshots(&[
        (
            100,
            1,
            51,
            3,
            "db",
            "public",
            "orders",
            "inactive_idx",
            "CREATE INDEX inactive_idx ON public.orders (id)",
        ),
        (
            200,
            1,
            51,
            3,
            "db",
            "public",
            "orders",
            "inactive_idx",
            "CREATE INDEX inactive_idx ON public.orders (id)",
        ),
        (
            100,
            1,
            61,
            0,
            "db",
            "public",
            "events",
            "active_idx",
            "CREATE INDEX active_idx ON public.events (id)",
        ),
        (
            200,
            1,
            61,
            8,
            "db",
            "public",
            "events",
            "active_idx",
            "CREATE INDEX active_idx ON public.events (id)",
        ),
    ]);
    fixture.finish();

    let base = format!(
        "/api/segments/{SEGMENT_ID}/snapshot?at=200&section=pg_stat_user_indexes&field=index_count&field=idx_scan&where.no_scans=true&page_size=1"
    );
    let objects = stream(fixture.prepare(&format!("{base}&group=object"), None))
        .expect("low-activity objects");
    let rows = relation_records(&objects);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["key"]["indexrelname"], "inactive_idx");
    assert_eq!(rows[0]["values"]["idx_scan"], 0.0);
    let page = objects
        .iter()
        .find(|record| record["record"] == "snapshot_page")
        .expect("low-activity page");
    assert_eq!(page["eligible"], "1");
    assert_eq!(page["has_more"], false);

    let databases = stream(fixture.prepare(&format!("{base}&group=database"), None))
        .expect("low-activity database rollup");
    let rows = relation_records(&databases);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0]["values"]["index_count"], "1");
}

#[test]
fn an_order_needs_one_section_to_name_a_column_in() {
    let path = format!("/api/segments/{SEGMENT_ID}/snapshot");
    assert!(crate::route::parse(&path, Some("at=1&section=a&section=b&by=x")).is_err());
    assert!(crate::route::parse(&path, Some("at=1&section=a&section=b&page_size=5")).is_err());
    assert!(crate::route::parse(&path, Some("at=1&section=a&by=x&page_size=5")).is_ok());
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
