use super::overlaps;

use kronika_format::{DEFAULT_BLOB_THRESHOLD, DEFAULT_TRUNCATE_LIMIT, DictLimits, Resolved, StrId};
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_topology::OsTopology;
use kronika_registry::{Cell, Section as _, StrId as RegistryStrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use sha2::{Digest as _, Sha256};

use super::Reader;

/// A segment covering ten to twenty microseconds.
const MIN: i64 = 10;
const MAX: i64 = 20;

#[test]
fn an_unbounded_range_takes_every_segment() {
    assert!(overlaps(&(..), MIN, MAX));
    assert!(overlaps(&(..), i64::MIN, i64::MAX));
}

#[test]
fn a_segment_entirely_before_the_range_is_out() {
    assert!(!overlaps(&(21..), MIN, MAX));
    assert!(!overlaps(&(21..30), MIN, MAX));
}

#[test]
fn a_segment_entirely_after_the_range_is_out() {
    assert!(!overlaps(&(..10), MIN, MAX));
    assert!(!overlaps(&(0..10), MIN, MAX));
}

#[test]
fn a_segment_overlapping_at_one_end_is_in() {
    assert!(overlaps(&(20..), MIN, MAX));
    assert!(overlaps(&(..11), MIN, MAX));
    assert!(overlaps(&(0..15), MIN, MAX));
    assert!(overlaps(&(15..30), MIN, MAX));
}

#[test]
fn a_range_inside_the_segment_is_in() {
    assert!(overlaps(&(12..15), MIN, MAX));
}

#[test]
fn an_excluded_bound_drops_the_touching_segment() {
    // `..=10` keeps a segment starting at 10, `..10` does not.
    assert!(overlaps(&(..=10), MIN, MAX));
    assert!(!overlaps(&(..10), MIN, MAX));
}

#[test]
fn an_instant_segment_is_matched_by_the_instant() {
    assert!(overlaps(&(10..=10), 10, 10));
    assert!(!overlaps(&(10..10), 10, 10));
}

#[test]
fn an_empty_range_takes_nothing() {
    assert!(!overlaps(&(15..15), MIN, MAX));
}

#[test]
fn a_bound_at_the_end_of_the_scale_excludes_everything() {
    use std::ops::Bound;

    // Nothing sits past the last microsecond, or before the first.
    assert!(!overlaps(
        &(Bound::Excluded(i64::MAX), Bound::Unbounded),
        i64::MAX,
        i64::MAX
    ));
    assert!(!overlaps(&(..i64::MIN), i64::MIN, i64::MIN));
}

const SEGMENT_ID: i64 = 1_709_164_800_000_000;

fn writer(directory: &tempfile::TempDir) -> WriterOwner {
    DataRoot::open(directory.path())
        .expect("open data root")
        .acquire_writer(LayoutLimits::default())
        .expect("acquire writer")
}

fn address(raw_id: i64) -> SegmentAddress {
    SegmentAddress::new(SegmentId::new(raw_id).expect("positive segment id"))
        .expect("segment address")
}

fn topology(ts: i64, cpu_id: i32, model_name: StrId) -> OsTopology {
    OsTopology {
        ts: Ts(ts),
        cpu_id,
        model_name: RegistryStrId(model_name.get()),
        mhz_max: Some(3_600.0),
        core_id: cpu_id,
        socket_id: 0,
        numa_node: 0,
        scope: 0,
    }
}

fn append_text_window(journal: &mut Journal, segment_id: SegmentId, ts: i64, text: &[u8]) -> StrId {
    let mut interner = Interner::new(DictLimits::default());
    let id = interner.intern(text).expect("intern fixture text");
    let dictionary = dict::encode(interner.window()).expect("encode dictionary delta");
    let mut buffers = SectionBuffers::new();
    buffers
        .push(topology(ts, i32::try_from(ts).unwrap_or(0), id))
        .expect("buffer topology row");
    let part = buffers
        .flush(&dictionary)
        .expect("encode part")
        .expect("part has one row");
    journal
        .append(segment_id, &part)
        .expect("append current window");
    id
}

fn one_segment(reader: &Reader) -> super::Segment {
    let listing = reader.segments(..).expect("list segments");
    assert!(listing.warnings.is_empty(), "unexpected warnings");
    assert_eq!(listing.segments.len(), 1, "one logical segment");
    reader
        .open_segment(&listing.segments[0])
        .expect("open logical segment")
}

#[test]
fn a_segment_reference_cannot_cross_reader_roots() {
    let first_directory = tempfile::tempdir().expect("first tempdir");
    let first_owner = writer(&first_directory);
    let address = address(SEGMENT_ID);
    let mut journal =
        Journal::open(&first_owner, JournalConfig::default()).expect("open first journal");
    append_text_window(&mut journal, address.id, 100, b"first root");
    let first_reader = Reader::open(first_directory.path()).expect("open first reader");
    let listing = first_reader.segments(..).expect("list first root");

    let second_directory = tempfile::tempdir().expect("second tempdir");
    let _second_owner = writer(&second_directory);
    let second_reader = Reader::open(second_directory.path()).expect("open second reader");
    let error = second_reader
        .open_segment(&listing.segments[0])
        .expect_err("a reference is bound to the reader that listed it");
    assert!(matches!(
        error,
        super::ReaderError::Io(ref source) if source.kind() == std::io::ErrorKind::InvalidInput
    ));
}

#[test]
fn active_read_keeps_its_prefix_and_next_read_gets_later_deltas() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let first_id = append_text_window(&mut journal, address.id, 100, b"first");
    let first_prefix_bytes = std::fs::metadata(directory.path().join("active.wal"))
        .expect("active journal metadata")
        .len();

    let reader = Reader::open(directory.path()).expect("open reader");
    let first_listing = reader.segments(..).expect("capture first prefix");
    assert_eq!(first_listing.segments.len(), 1);
    assert!(
        reader
            .segments(..100)
            .expect("range before")
            .segments
            .is_empty()
    );
    assert!(
        reader
            .segments(101..)
            .expect("range after")
            .segments
            .is_empty()
    );

    let second_id = append_text_window(&mut journal, address.id, 200, b"second");
    let first = reader
        .open_segment(&first_listing.segments[0])
        .expect("open captured prefix after append");
    assert_eq!(first.captured_bytes(), first_prefix_bytes);
    assert_eq!(
        first
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("rows")
            .len(),
        1
    );
    let first_dictionary = first.dictionary().expect("first dictionary");
    assert_eq!(
        first_dictionary.resolve(first_id.get()),
        Some(Resolved::Str(b"first"))
    );
    assert_eq!(first_dictionary.resolve(second_id.get()), None);

    let second = one_segment(&reader);
    assert_eq!(
        second
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("rows")
            .len(),
        2
    );
    let second_dictionary = second.dictionary().expect("complete dictionary");
    assert_eq!(
        second_dictionary.resolve(first_id.get()),
        Some(Resolved::Str(b"first"))
    );
    assert_eq!(
        second_dictionary.resolve(second_id.get()),
        Some(Resolved::Str(b"second"))
    );
    assert!(
        reader
            .segments(201..)
            .expect("after active")
            .segments
            .is_empty()
    );
}

#[test]
fn finished_segment_wins_over_the_same_active_generation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"one row");
    write_segment(&journal, &owner, address).expect("publish finished segment");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    assert_eq!(
        segment.path().extension().and_then(std::ffi::OsStr::to_str),
        Some("zms")
    );
    assert_eq!(
        segment.captured_bytes(),
        std::fs::metadata(segment.path())
            .expect("finished segment metadata")
            .len()
    );
    assert_eq!(
        segment
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("rows")
            .len(),
        1,
        "the active generation must not duplicate the finished segment"
    );
}

#[test]
fn complete_dictionary_preserves_boundary_and_truncated_blob_metadata() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let active_address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let small = vec![b's'; DEFAULT_BLOB_THRESHOLD - 1];
    let boundary = vec![b'b'; DEFAULT_BLOB_THRESHOLD];
    let oversized = vec![b't'; DEFAULT_TRUNCATE_LIMIT + 1];
    let mut interner = Interner::new(DictLimits::default());
    let small_id = interner.intern(&small).expect("4095-byte string");
    let boundary_id = interner.intern(&boundary).expect("4096-byte blob");
    let oversized_id = interner.intern(&oversized).expect("truncated blob");
    let dictionary = dict::encode(interner.window()).expect("collector dictionary output");
    let mut buffers = SectionBuffers::new();
    for (cpu_id, id) in [small_id, boundary_id, oversized_id]
        .into_iter()
        .enumerate()
    {
        let cpu_id = i32::try_from(cpu_id).expect("three fixture rows fit i32");
        buffers
            .push(topology(100 + i64::from(cpu_id), cpu_id, id))
            .expect("buffer topology row");
    }
    let part = buffers
        .flush(&dictionary)
        .expect("encode current collector part")
        .expect("part has rows");
    journal
        .append(active_address.id, &part)
        .expect("append current collector output");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    let dictionary = segment.dictionary().expect("decode complete dictionary");
    assert_eq!(dictionary.entries().count(), 3);
    assert_eq!(
        dictionary.resolve(small_id.get()),
        Some(Resolved::Str(&small))
    );
    assert_eq!(
        dictionary.resolve(boundary_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: boundary_id,
            stored_bytes: &boundary,
            full_len: DEFAULT_BLOB_THRESHOLD as u64,
            truncated: false,
            full_sha256: None,
        }))
    );
    let expected_hash: [u8; 32] = Sha256::digest(&oversized).into();
    assert_eq!(
        dictionary.resolve(oversized_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: oversized_id,
            stored_bytes: &oversized[..DEFAULT_TRUNCATE_LIMIT],
            full_len: oversized.len() as u64,
            truncated: true,
            full_sha256: Some(expected_hash),
        }))
    );
    for row in segment
        .rows(OsTopology::CONTRACT.type_id.get())
        .expect("decode rows")
    {
        let Some(Cell::StrId(id)) = row.get("model_name") else {
            panic!("model_name must be a StrId")
        };
        assert!(dictionary.resolve(*id).is_some());
    }

    let finished_directory = tempfile::tempdir().expect("finished tempdir");
    let finished_owner = writer(&finished_directory);
    let finished_address = address(SEGMENT_ID + 1);
    let mut finished_journal =
        Journal::open(&finished_owner, JournalConfig::default()).expect("open finished journal");
    let finished_boundary_id =
        append_text_window(&mut finished_journal, finished_address.id, 200, &boundary);
    write_segment(&finished_journal, &finished_owner, finished_address)
        .expect("publish finished blob output");
    let finished_reader = Reader::open(finished_directory.path()).expect("open finished reader");
    let finished = one_segment(&finished_reader);
    assert_eq!(
        finished
            .path()
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("zms")
    );
    let dictionary = finished.dictionary().expect("finished dictionary");
    assert_eq!(
        dictionary.resolve(finished_boundary_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: finished_boundary_id,
            stored_bytes: &boundary,
            full_len: DEFAULT_BLOB_THRESHOLD as u64,
            truncated: false,
            full_sha256: None,
        }))
    );
}

#[test]
fn range_discovery_checks_bodies_only_after_selection() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let first_address = address(SEGMENT_ID);
    let second_address = address(SEGMENT_ID + 1);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, first_address.id, 100, b"first");
    write_segment(&journal, &owner, first_address).expect("publish first");
    journal.reset().expect("reset after first");
    append_text_window(&mut journal, second_address.id, 200, b"second");
    write_segment(&journal, &owner, second_address).expect("publish second");
    journal.reset().expect("leave no active segment");

    let second_path = directory
        .path()
        .join(second_address.day.year_component())
        .join(second_address.day.month_component())
        .join(second_address.day.day_component())
        .join(second_address.zms_name());
    let mut bytes = std::fs::read(&second_path).expect("read second segment");
    bytes[kronika_format::MAGIC.len()] ^= 0xff;
    std::fs::write(&second_path, bytes).expect("damage one selected body");

    let reader = Reader::open(directory.path()).expect("open reader");
    let first = reader.segments(100..=100).expect("select first only");
    assert_eq!(first.segments.len(), 1);
    assert!(
        first.warnings.is_empty(),
        "unselected body must stay unread"
    );

    let second = reader.segments(200..=200).expect("select damaged second");
    assert!(second.segments.is_empty());
    assert_eq!(second.warnings.len(), 1);
    assert!(matches!(
        second.warnings[0].reason,
        kronika_store::StoreWarningReason::InvalidZms(
            kronika_store::InvalidZmsReason::SectionChecksum
        )
    ));
}
