use std::path::Path;

use kronika_format::DictLimits;
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId};
use kronika_reader::{Reader, SegmentKind, SegmentRef};
use kronika_registry::os_topology::OsTopology;
use kronika_registry::{StrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};

use super::{path_of, read, resource};

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn address() -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(SEGMENT_ID).expect("segment id")).expect("address")
}

fn row(ts: i64, model_name: StrId, mhz: f64) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id: 7,
        model_name,
        mhz_max: Some(mhz),
        core_id: 3,
        socket_id: 1,
        numa_node: 0,
        scope: 0,
    }
}

fn append_fixture(journal: &mut Journal) {
    let mut interner = Interner::new(DictLimits::default());
    let model_name = StrId(
        interner
            .intern(b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL")
            .expect("intern label")
            .get(),
    );
    let dictionary = dict::encode(interner.window()).expect("dictionary");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(row(100, model_name, 2_000.0))
        .expect("first row");
    buffers
        .push(row(200, model_name, 2_500.0))
        .expect("second row");
    let part = buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("nonempty part");
    journal.append(address().id, &part).expect("append fixture");
}

fn only_segment(reader: &Reader, kind: SegmentKind) -> SegmentRef {
    let listing = reader.catalog_segments(..).expect("list fixture");
    let segments: Vec<_> = listing
        .segments
        .into_iter()
        .filter(|segment| segment.kind() == kind)
        .collect();
    assert_eq!(segments.len(), 1, "one segment of requested kind");
    segments.into_iter().next().expect("one segment")
}

#[test]
fn an_index_lives_beside_its_finished_segment() {
    assert_eq!(
        path_of(Path::new("/data/2026/08/08/17.zms")),
        Some(Path::new("/data/2026/08/08/17.idx").to_path_buf())
    );
}

#[test]
fn active_data_never_gets_an_index_path() {
    assert_eq!(path_of(Path::new("/data/active.wal")), None);
}

#[test]
fn real_active_and_finished_resources_are_bounded_and_atomically_cached() {
    let directory = tempfile::tempdir().expect("tempdir");
    let data_root = DataRoot::open(directory.path()).expect("data root");
    let writer = data_root
        .acquire_writer(LayoutLimits::default())
        .expect("writer");
    let mut journal = Journal::open(&writer, JournalConfig::default()).expect("journal");
    append_fixture(&mut journal);

    let reader = Reader::open(directory.path()).expect("reader");
    let active_ref = only_segment(&reader, SegmentKind::Active);
    let active = resource(directory.path(), &reader, &active_ref, 0b101, "os_topology")
        .expect("active resource");
    assert!(!active.persisted);
    assert_eq!(active.index.checksum, None);
    assert_eq!(active.index.sections.len(), 1);

    write_segment(&journal, &writer, address()).expect("finish segment");
    let reader = Reader::open(directory.path()).expect("finished reader");
    let finished_ref = only_segment(&reader, SegmentKind::Finished);
    let index_path = path_of(reader.open_segment(&finished_ref).expect("segment").path())
        .expect("finished index path");

    let contended_owner = data_root
        .acquire_index(LayoutLimits::default())
        .expect("hold index owner");
    let computed = resource(
        directory.path(),
        &reader,
        &finished_ref,
        0b101,
        "os_topology",
    )
    .expect("serve while publication is contended");
    assert!(!computed.persisted);
    assert!(computed.index.checksum.is_some());
    assert!(!index_path.exists(), "contended request must not publish");
    drop(contended_owner);

    let published = resource(
        directory.path(),
        &reader,
        &finished_ref,
        0b101,
        "os_topology",
    )
    .expect("publish finished index");
    assert!(published.persisted);
    assert_eq!(published.index.checksum, computed.index.checksum);
    assert!(index_path.is_file());
    let bytes = std::fs::read(&index_path).expect("read index bytes");
    assert!(
        !bytes
            .windows(b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL".len())
            .any(|window| window == b"IDX-MUST-NOT-COPY-THIS-DISPLAY-LABEL"),
        "non-identity display labels do not belong in IDX"
    );
    assert_eq!(read(&index_path).expect("read index").sources, 0b101);

    let rebuilt = resource(
        directory.path(),
        &reader,
        &finished_ref,
        0b111,
        "os_topology",
    )
    .expect("rebuild for changed source set");
    assert!(rebuilt.persisted);
    assert_ne!(rebuilt.index.checksum, published.index.checksum);
    assert_eq!(
        read(&index_path).expect("read rebuilt index").sources,
        0b111
    );
}
