//! Query-output parity across native and in-memory finished-segment sources.

use std::path::Path;
use std::sync::Arc;

// Dependencies of other targets of this crate; anchored for the
// `unused_crate_dependencies` lint, which checks each target separately.
use base64 as _;
use icu_collator as _;
use icu_locale_core as _;
use kronika_format::DictLimits;
use kronika_index as _;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_query::{
    CatalogRequest, EventsQuery, EventsRepresentation, EventsResult, FinishedDataset,
    HeatmapBatchQuery, HeatmapBatchResult, HeatmapItemQuery, HeatmapView, NormalizedRanking,
    QueryContext, QueryDataset, QueryRequest, QuerySink, TimeRange, execute, execute_events,
    execute_heatmap_batch,
};
use kronika_reader as _;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::pg_log::{PgLogErrors, PgLogTempFiles};
use kronika_registry::{StrId, Ts};
use kronika_store::{EmbeddedSource, PosixSource};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
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

fn ranking_only_query() -> HeatmapBatchQuery {
    let top_one = HeatmapItemQuery {
        ranking: NormalizedRanking {
            section: "os_cpu".to_owned(),
            fields: vec!["user".to_owned()],
            top: 1,
        },
        view: HeatmapView::RankingOnly,
    };
    let mut top_two = top_one.clone();
    top_two.ranking.top = 2;
    HeatmapBatchQuery {
        range: TimeRange::new(HEATMAP_FROM, HEATMAP_TO + 1).expect("ranking range"),
        items: vec![top_one.clone(), top_two, top_one],
    }
}

fn ranking_only_result(
    dataset: Arc<dyn QueryDataset>,
    query: HeatmapBatchQuery,
) -> HeatmapBatchResult {
    let context = QueryContext::new(dataset, 0, false);
    execute_heatmap_batch(&context, query, &Records::default())
        .expect("execute typed ranking batch")
}

fn events_query() -> EventsQuery {
    EventsQuery::normalize(
        TimeRange::new(SEGMENT_ID, SEGMENT_ID + 100).expect("events range"),
        Some(vec![
            "pg_log_temp_files".to_owned(),
            "pg_log_errors".to_owned(),
        ]),
        EventsRepresentation::Occurrences,
        3,
    )
    .expect("events query")
}

fn events_result(dataset: Arc<dyn QueryDataset>, query: EventsQuery) -> EventsResult {
    let context = QueryContext::new(dataset, 0, false);
    execute_events(&context, query, &Records::default()).expect("execute typed events query")
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

fn write_events_fixture(root: &Path, segment_id: SegmentId) -> Arc<[u8]> {
    let data_root = DataRoot::open(root).expect("open events data root");
    let owner = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("acquire events writer");
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open events journal");
    let mut interner = Interner::new(DictLimits::default());
    let source_file = StrId(
        interner
            .intern(b"postgresql.log")
            .expect("intern source file")
            .get(),
    );
    let mut buffers = SectionBuffers::new();
    for (timestamp, pattern) in [
        (SEGMENT_ID + 10, b"error-a".as_slice()),
        (SEGMENT_ID + 20, b"error-b".as_slice()),
    ] {
        let pattern = StrId(interner.intern(pattern).expect("intern pattern").get());
        buffers
            .push(PgLogErrors {
                ts: Ts(timestamp),
                system_identifier: Some(42),
                source_file,
                severity: 0,
                category: 8,
                sqlstate: None,
                pattern,
                count: 1,
                sample: pattern,
                detail: None,
                hint: None,
                context: None,
                statement: None,
                database: None,
                username: None,
            })
            .expect("error row fits");
    }
    for (timestamp, size_bytes) in [(SEGMENT_ID + 10, 100), (SEGMENT_ID + 30, 300)] {
        buffers
            .push(PgLogTempFiles {
                ts: Ts(timestamp),
                system_identifier: Some(42),
                source_file,
                path: None,
                size_bytes,
                statement: None,
            })
            .expect("temporary-file row fits");
    }
    let dictionary = dict::encode(interner.window()).expect("encode events dictionary");
    let part = buffers
        .flush(&dictionary)
        .expect("encode event rows")
        .expect("nonempty event rows");
    journal
        .append(segment_id, &part)
        .expect("append event rows");
    let summary = write_segment(
        &journal,
        &owner,
        SegmentAddress::new(segment_id).expect("segment address"),
    )
    .expect("write events segment");
    journal.reset().expect("reset events journal");
    drop(journal);
    drop(owner);

    let payload: Arc<[u8]> = std::fs::read(finished_path(root, segment_id))
        .expect("read events segment")
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

#[test]
fn ranking_only_batch_is_typed_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_heatmap_fixture(directory.path(), segment_id);
    let query = ranking_only_query();

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    assert_eq!(posix.retained_segment_bytes(), 0);
    let posix_result =
        ranking_only_result(Arc::new(FinishedDataset::new(posix.clone())), query.clone());
    assert_eq!(posix.retained_segment_bytes(), 0);

    let embedded = EmbeddedSource::from_shared(
        segment_id,
        Arc::clone(&payload),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), payload.len());
    let embedded_result =
        ranking_only_result(Arc::new(FinishedDataset::new(embedded.clone())), query);
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), payload.len());

    assert_eq!(posix_result, embedded_result);
    let results = &embedded_result.results;
    assert_eq!(results.len(), 3);
    assert_eq!(
        results[0], results[2],
        "exact duplicate items stay in place"
    );
    assert_eq!(
        results
            .iter()
            .map(|result| result.ranking.top)
            .collect::<Vec<_>>(),
        [1, 2, 1]
    );
    assert!(results.iter().all(|result| result.grid.is_none()));
    assert_eq!(results[0].entity_count, 2);
    assert_eq!(results[0].totals_total, Some(30.0));
    assert_eq!(results[0].others_total, Some(10.0));
    assert_eq!(results[0].entities.len(), 1);
    assert_eq!(
        results[0].entities[0].identity["cpu_id"],
        serde_json::json!(1)
    );
    assert_eq!(results[0].entities[0].total, Some(20.0));
    assert_eq!(results[1].entity_count, 2);
    assert_eq!(results[1].others_total, None);
    assert_eq!(results[1].entities.len(), 2);
    assert_eq!(
        results[1].entities[0].identity["cpu_id"],
        serde_json::json!(1)
    );
    assert_eq!(results[1].entities[0].total, Some(20.0));
    assert_eq!(
        results[1].entities[1].identity["cpu_id"],
        serde_json::json!(0)
    );
    assert_eq!(results[1].entities[1].total, Some(10.0));
}

#[test]
fn events_result_is_typed_identical_for_posix_and_embedded_finished_zms() {
    let segment_id = SegmentId::new(SEGMENT_ID).expect("explicit segment identity");
    let directory = tempfile::tempdir().expect("temporary POSIX root");
    let payload = write_events_fixture(directory.path(), segment_id);
    let query = events_query();

    let posix = PosixSource::open(directory.path()).expect("POSIX source");
    assert_eq!(posix.retained_segment_bytes(), 0);
    let posix_result = events_result(Arc::new(FinishedDataset::new(posix.clone())), query.clone());
    assert_eq!(posix.retained_segment_bytes(), 0);

    let embedded = EmbeddedSource::from_shared(
        segment_id,
        Arc::clone(&payload),
        u64::try_from(payload.len()).expect("payload length fits u64"),
    )
    .expect("embedded source");
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), payload.len());
    let embedded_result = events_result(Arc::new(FinishedDataset::new(embedded.clone())), query);
    assert_eq!(embedded.retained_segment_ptr(), payload.as_ptr());
    assert_eq!(embedded.retained_segment_bytes(), payload.len());

    assert_eq!(posix_result, embedded_result);
    let EventsResult::Occurrences {
        occurrences,
        truncated,
    } = &embedded_result
    else {
        panic!("occurrence result");
    };
    assert!(*truncated);
    assert_eq!(occurrences.len(), 3);
    let detail_refs = occurrences
        .iter()
        .map(|occurrence| occurrence.detail_ref().expect("valid detail reference"))
        .collect::<Vec<_>>();
    assert!(detail_refs.iter().all(|detail_ref| !detail_ref.is_empty()));
    assert_eq!(detail_refs, {
        let EventsResult::Occurrences { occurrences, .. } = &posix_result else {
            panic!("occurrence result");
        };
        occurrences
            .iter()
            .map(|occurrence| occurrence.detail_ref().expect("valid detail reference"))
            .collect::<Vec<_>>()
    });
    let wire = serde_json::to_value(&embedded_result).expect("serialize typed event result");
    assert_eq!(
        wire["occurrences"]
            .as_array()
            .expect("occurrence array")
            .iter()
            .map(|occurrence| (
                occurrence["source"].as_str().expect("event source"),
                occurrence["detail_locator"]["at"]
                    .as_str()
                    .expect("event timestamp"),
            ))
            .collect::<Vec<_>>(),
        [
            ("pg_log_temp_files", "1709164800000010"),
            ("pg_log_errors", "1709164800000010"),
            ("pg_log_errors", "1709164800000020"),
        ]
    );
}
