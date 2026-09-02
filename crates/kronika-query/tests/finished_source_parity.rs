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
use kronika_layout::{SegmentAddress, SegmentId};
use kronika_query::{
    CatalogRequest, FinishedDataset, QueryContext, QueryDataset, QueryRequest, QuerySink, execute,
};
use kronika_reader as _;
use kronika_registry as _;
use kronika_store::{EmbeddedSource, PosixSource};
use schemars as _;
use serde as _;

const SEGMENT_ID: i64 = 1_709_164_800_000_000;
const SEGMENT_ID_TEXT: &str = "1709164800000000";
const ZMS: &[u8] = include_bytes!("../../kronika-format/tests/fixtures/minimal.zms");

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
    let execution = execute(&context, &QueryRequest::Catalog(CatalogRequest::default()))
        .expect("prepare catalog query");
    let mut records = Records::default();
    execution
        .stream(&mut records)
        .expect("stream catalog query");
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
