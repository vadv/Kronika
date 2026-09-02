//! Query-output parity across native and in-memory finished-segment sources.

use std::path::Path;
use std::sync::Arc;

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use base64 as _;
use icu_collator as _;
use icu_locale_core as _;
use kronika_format as _;
use kronika_index as _;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_query::{
    CatalogRequest, FinishedDataset, HeatmapBatchQuery, HeatmapItemQuery, HeatmapView,
    NormalizedRanking, QueryContext, QueryDataset, QueryRequest, QuerySink, TimeRange, execute,
};
use kronika_reader as _;
use kronika_registry::Ts;
use kronika_registry::os_cpu::OsCpu;
use kronika_store::{EmbeddedSource, PosixSource};
use kronika_writer::{Journal, JournalConfig, SectionBuffers, write_segment};
use schemars as _;
use serde as _;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SEGMENT_ID_TEXT: &str = "1709164800000000";
const ZMS: &[u8] = include_bytes!("../../kronika-format/tests/fixtures/minimal.zms");
const HEATMAP_FROM: i64 = SEGMENT_ID;
const HEATMAP_TO: i64 = HEATMAP_FROM + 1_000_000;

#[derive(Default)]
struct Records(Vec<u8>);

impl QuerySink for Records {
    fn record(&mut self, bytes: Vec<u8>) -> bool {
        self.0.extend_from_slice(&bytes);
        true
    }

    fn cancelled(&self) -> bool {
        false
    }
}

fn catalog_bytes(dataset: Arc<dyn QueryDataset>) -> Vec<u8> {
    let context = QueryContext::new(dataset, 0, false);
    let execution = execute(&context, QueryRequest::Catalog(CatalogRequest::default()))
        .expect("prepare catalog query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("stream catalog query");
    records.0
}

fn heatmap_query(top: usize) -> HeatmapBatchQuery {
    HeatmapBatchQuery {
        range: TimeRange::new(HEATMAP_FROM, HEATMAP_TO + 1).expect("heatmap range"),
        items: vec![HeatmapItemQuery {
            ranking: NormalizedRanking {
                section: "os_cpu".to_owned(),
                fields: vec!["user".to_owned()],
                top,
            },
            view: HeatmapView::Grid {
                columns: 1,
                group: Vec::new(),
                type_id: None,
            },
        }],
    }
}

fn heatmap_bytes(dataset: Arc<dyn QueryDataset>) -> Vec<u8> {
    let context = QueryContext::new(dataset, 0, false);
    let query =
        kronika_query::validate_heatmap_request(heatmap_query(1)).expect("validate heatmap query");
    let execution = execute(&context, QueryRequest::Heatmap(query)).expect("prepare heatmap query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("stream heatmap query");
    records.0
}

fn write_posix_fixture(root: &Path, segment_id: SegmentId) {
    let address = SegmentAddress::new(segment_id).expect("segment address");
    let day = root
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component());
    std::fs::create_dir_all(&day).expect("create fixture day");
    std::fs::write(day.join(address.zms_name()), ZMS).expect("write finished ZMS fixture");
}

fn finished_path(root: &Path, segment_id: SegmentId) -> std::path::PathBuf {
    let address = SegmentAddress::new(segment_id).expect("segment address");
    root.join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name())
}

fn write_heatmap_fixture(root: &Path, segment_id: SegmentId) -> Arc<[u8]> {
    let data_root = DataRoot::open(root).expect("open heatmap data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire heatmap writer");
    let mut journal =
        Journal::open(&owner, JournalConfig::default()).expect("open heatmap journal");
    let mut buffers = SectionBuffers::new();
    for (timestamp, aggregate, first, second) in [(HEATMAP_FROM, 0, 0, 0), (HEATMAP_TO, 30, 10, 20)]
    {
        for (cpu_id, user) in [(-1, aggregate), (0, first), (1, second)] {
            buffers
                .push(OsCpu {
                    ts: Ts(timestamp),
                    cpu_id,
                    user,
                    nice: 0,
                    system: 0,
                    idle: 0,
                    iowait: 0,
                    irq: 0,
                    softirq: 0,
                    steal: 0,
                    guest: 0,
                    guest_nice: 0,
                    scope: 0,
                })
                .expect("CPU row fits");
        }
    }
    let part = buffers
        .flush(&[])
        .expect("encode heatmap rows")
        .expect("nonempty heatmap rows");
    journal
        .append(segment_id, &part)
        .expect("append heatmap rows");
    let summary = write_segment(
        &journal,
        &owner,
        SegmentAddress::new(segment_id).expect("segment address"),
    )
    .expect("write heatmap segment");
    journal.reset().expect("reset heatmap journal");
    drop(journal);
    drop(owner);

    let payload: Arc<[u8]> = std::fs::read(finished_path(root, segment_id))
        .expect("read heatmap segment")
        .into();
    assert_eq!(
        u64::try_from(payload.len()).expect("payload length fits u64"),
        summary.bytes,
        "writer byte count must match the published segment"
    );
    payload
}

#[test]
fn catalog_query_is_byte_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    write_posix_fixture(directory.path(), segment_id);

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    assert_eq!(
        posix.retained_segment_bytes(),
        0,
        "the POSIX query source must not copy the complete segment"
    );
    let posix_bytes = catalog_bytes(Arc::new(FinishedDataset::new(posix.clone())));
    assert_eq!(
        posix.retained_segment_bytes(),
        0,
        "the completed POSIX query must retain no segment payload"
    );

    let payload: Arc<[u8]> = Arc::from(ZMS);
    let embedded = EmbeddedSource::from_shared(
        segment_id,
        Arc::clone(&payload),
        u64::try_from(ZMS.len()).expect("fixture length fits u64"),
    )
    .expect("embedded source");
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), ZMS.len());
    let embedded_bytes = catalog_bytes(Arc::new(FinishedDataset::new(embedded.clone())));
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), ZMS.len());

    assert_eq!(posix_bytes, embedded_bytes);
    let records = embedded_bytes
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| serde_json::from_slice::<serde_json::Value>(record).expect("NDJSON record"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    assert_eq!(records[1]["record"], "finished_segment");
    assert_eq!(
        records[1]["id"].as_str(),
        Some(SEGMENT_ID_TEXT),
        "embedded execution must keep the caller-supplied segment identity"
    );
}

#[test]
fn heatmap_query_is_byte_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_heatmap_fixture(directory.path(), segment_id);

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    assert_eq!(posix.retained_segment_bytes(), 0);
    let posix_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(posix.clone()));
    let posix_bytes = heatmap_bytes(Arc::clone(&posix_dataset));
    assert_eq!(posix.retained_segment_bytes(), 0);

    let embedded = EmbeddedSource::from_shared(
        segment_id,
        Arc::clone(&payload),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), payload.len());
    let embedded_dataset: Arc<dyn QueryDataset> = Arc::new(FinishedDataset::new(embedded.clone()));
    let embedded_bytes = heatmap_bytes(Arc::clone(&embedded_dataset));
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), payload.len());

    assert_eq!(posix_bytes, embedded_bytes);
    let records = embedded_bytes
        .split(|byte| *byte == b'\n')
        .filter(|record| !record.is_empty())
        .map(|record| serde_json::from_slice::<serde_json::Value>(record).expect("NDJSON record"))
        .collect::<Vec<_>>();
    assert_eq!(
        records
            .iter()
            .map(|record| record["record"].as_str().expect("record kind"))
            .collect::<Vec<_>>(),
        ["heatmap", "heatmap_row", "heatmap_band", "heatmap_band"]
    );
    assert_eq!(records[0]["from"], HEATMAP_FROM.to_string());
    assert_eq!(records[0]["to"], HEATMAP_TO.to_string());
    assert_eq!(records[0]["entity_count"], 2);
    assert_eq!(records[0]["top"], 1);
    assert_eq!(records[0]["others_count"], 1);
    assert_eq!(records[1]["identity"], serde_json::json!([1]));
    assert_eq!(records[1]["total"], 20.0);
    assert_eq!(records[2]["total"], 30.0);
    assert_eq!(records[3]["total"], 10.0);

    let posix_error = kronika_query::validate_heatmap_request(heatmap_query(0))
        .expect_err("POSIX request rejects zero top");
    let embedded_error = kronika_query::validate_heatmap_request(heatmap_query(0))
        .expect_err("embedded request rejects zero top");
    assert_eq!(posix_error.code(), embedded_error.code());
    assert_eq!(posix_error.parameter(), embedded_error.parameter());
    assert_eq!(posix_error.to_string(), embedded_error.to_string());
}
