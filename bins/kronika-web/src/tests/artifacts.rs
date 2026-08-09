use std::cell::Cell;
use std::path::Path;

use hyper::StatusCode;
use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::instance_metadata::InstanceMetadata;
use kronika_registry::os_diskstats::OsDiskstats;
use kronika_registry::os_psi::OsPsi;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use serde_json::Value;

use crate::api::{ApiError, CachePolicy, Prepared};

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
    fixture.finish();

    let index_target = format!("/api/segments/{SEGMENT_ID}/sections/os_diskstats/index");
    let prepared = fixture.prepare(&index_target, None);
    let meta = prepared.meta();
    assert_eq!(meta.status, StatusCode::OK);
    assert_eq!(meta.cache, CachePolicy::Revalidate);
    let etag = meta.etag.expect("finished index ETag");
    let index = stream(prepared).expect("finished index body");
    assert!(index.iter().any(|record| record["record"] == "object"));

    let offered = format!("\"stale\", W/{etag}");
    let not_modified = fixture.prepare(&index_target, Some(&offered));
    assert_eq!(not_modified.meta().status, StatusCode::NOT_MODIFIED);
    assert_eq!(not_modified.meta().cache, CachePolicy::Revalidate);
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
        record["record"] == "object"
            && record["type_id"] == "0"
            && record["observations"][0]["last"]["value"] == 50
    }));
}
