use super::{before_start, overlaps};

use std::collections::HashSet;

use kronika_format::{DEFAULT_BLOB_THRESHOLD, DEFAULT_TRUNCATE_LIMIT, DictLimits, Resolved, StrId};
use kronika_layout::{DataRoot, LayoutLimits, SegmentAddress, SegmentId, WriterOwner};
use kronika_registry::os_topology::OsTopology;
use kronika_registry::pg_stat_archiver::PgStatArchiver;
use kronika_registry::pg_stat_bgwriter::{PgStatBgwriterV1, PgStatBgwriterV2};
use kronika_registry::{Cell, Section as _, StrId as RegistryStrId, Ts};
use kronika_writer::{Interner, Journal, JournalConfig, SectionBuffers, dict, write_segment};
use sha2::{Digest as _, Sha256};

use super::{Dictionary, Reader, ReaderError, Segment};

/// A segment covering ten to twenty microseconds.
const MIN: i64 = 10;
const MAX: i64 = 20;

#[test]
fn only_replaced_io_sources_are_reopenable() {
    assert!(
        ReaderError::Io(std::io::Error::from(std::io::ErrorKind::Interrupted))
            .source_changed_during_read()
    );
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::Interrupted,
        std::io::ErrorKind::UnexpectedEof,
    ] {
        assert!(
            ReaderError::Store(kronika_store::StoreError::Io(std::io::Error::from(kind)))
                .source_changed_during_read(),
            "segment I/O kind {kind:?}"
        );
    }
    for kind in [
        std::io::ErrorKind::NotFound,
        std::io::ErrorKind::UnexpectedEof,
        std::io::ErrorKind::InvalidData,
    ] {
        assert!(
            !ReaderError::Io(std::io::Error::from(kind)).source_changed_during_read(),
            "directory I/O kind {kind:?}"
        );
    }
    assert!(!ReaderError::Store(kronika_store::StoreError::BadMagic).source_changed_during_read());
}

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
fn predecessor_bounds_follow_range_inclusivity() {
    assert!(before_start(&(21..), 20));
    assert!(!before_start(&(21..), 21));
    assert!(before_start(
        &(std::ops::Bound::Excluded(20), std::ops::Bound::Unbounded),
        20
    ));
    assert!(!before_start(&(..), 20));
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

fn one_segment(reader: &Reader) -> Segment {
    let listing = reader.segments(..).expect("list segments");
    assert!(listing.warnings.is_empty(), "unexpected warnings");
    assert_eq!(listing.segments.len(), 1, "one logical segment");
    reader
        .open_segment(&listing.segments[0])
        .expect("open logical segment")
}

fn assert_model_names_resolve(segment: &Segment, dictionary: &Dictionary) {
    for row in segment
        .rows(OsTopology::CONTRACT.type_id.get())
        .expect("decode rows")
    {
        let Some(Cell::StrId(id)) = row.get("model_name") else {
            panic!("model_name must be a StrId")
        };
        assert!(dictionary.resolve(*id).is_some());
    }
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
        ReaderError::Io(ref source) if source.kind() == std::io::ErrorKind::InvalidInput
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
    let selected = second
        .dictionary_for(&HashSet::from([first_id.get()]))
        .expect("select an id from the older dictionary delta");
    assert_eq!(
        selected.resolve(first_id.get()),
        Some(Resolved::Str(b"first"))
    );
    assert_eq!(selected.resolve(second_id.get()), None);
    assert!(
        reader
            .segments(201..)
            .expect("after active")
            .segments
            .is_empty()
    );
}

#[test]
fn an_active_reference_can_be_pinned_to_an_earlier_committed_position() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"first");

    let reader = Reader::open(directory.path()).expect("open reader");
    let first = reader.segments(..).expect("capture first prefix");
    let position = first.segments[0]
        .active_position()
        .expect("active position");

    append_text_window(&mut journal, address.id, 200, b"second");
    let latest = reader.segments(..).expect("capture latest prefix");
    let pinned = latest.segments[0]
        .at_active_position(position)
        .expect("pin earlier frame boundary");
    assert_eq!(pinned.active_position(), Some(position));
    let segment = reader.open_segment(&pinned).expect("open pinned prefix");
    assert_eq!(
        segment
            .rows(OsTopology::CONTRACT.type_id.get())
            .expect("pinned rows")
            .len(),
        1
    );
}

#[test]
fn projected_visit_keeps_stable_active_ordinals_and_stops_at_its_limit() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"first");
    append_text_window(&mut journal, address.id, 200, b"second");

    let reader = Reader::open(directory.path()).expect("open reader");
    let segment = one_segment(&reader);
    let mut rows = Vec::new();
    let visited = segment
        .visit_rows(
            OsTopology::CONTRACT.type_id.get(),
            &["cpu_id"],
            1,
            1,
            |ordinal, row| {
                rows.push((ordinal, row));
                true
            },
        )
        .expect("visit projected row");
    assert_eq!(visited, 1);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].0, 1);
    assert_eq!(rows[0].1.get("cpu_id"), Some(&Cell::I32(200)));
    assert_eq!(rows[0].1.get("ts"), Some(&Cell::Null));
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
fn damaged_finished_segment_does_not_hide_the_same_valid_active_generation() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    append_text_window(&mut journal, address.id, 100, b"one row");
    write_segment(&journal, &owner, address).expect("publish finished segment");

    let path = directory
        .path()
        .join(address.day.year_component())
        .join(address.day.month_component())
        .join(address.day.day_component())
        .join(address.zms_name());
    let mut bytes = std::fs::read(&path).expect("read finished segment");
    bytes[kronika_format::MAGIC.len()] ^= 0xff;
    std::fs::write(&path, bytes).expect("damage finished section body");

    let reader = Reader::open(directory.path()).expect("open reader");
    let listing = reader.segments(..).expect("list with body validation");
    assert_eq!(listing.segments.len(), 1);
    assert_eq!(listing.warnings.len(), 1);
    let segment = reader
        .open_segment(&listing.segments[0])
        .expect("open active fallback");
    assert_eq!(
        segment.path().file_name().and_then(std::ffi::OsStr::to_str),
        Some("active.wal")
    );
}

#[test]
fn current_dictionary_preserves_boundary_and_truncated_blob_metadata() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let active_address = address(SEGMENT_ID);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");
    let small = vec![0xff; DEFAULT_BLOB_THRESHOLD - 1];
    let boundary = vec![b'b'; DEFAULT_BLOB_THRESHOLD];
    let oversized = vec![b't'; DEFAULT_TRUNCATE_LIMIT + 1];
    let ignored = b"not selected".to_vec();
    let mut interner = Interner::new(DictLimits::default());
    let small_id = interner.intern(&small).expect("4095-byte string");
    let boundary_id = interner.intern(&boundary).expect("4096-byte blob");
    let oversized_id = interner.intern(&oversized).expect("truncated blob");
    let ignored_id = interner.intern(&ignored).expect("unrequested string");
    let dictionary = dict::encode(interner.window()).expect("collector dictionary output");
    let mut buffers = SectionBuffers::new();
    for (cpu_id, id) in [small_id, boundary_id, oversized_id, ignored_id]
        .into_iter()
        .enumerate()
    {
        let cpu_id = i32::try_from(cpu_id).expect("four fixture rows fit i32");
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
    assert_eq!(dictionary.entries().count(), 4);
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
    let selected = segment
        .dictionary_for(&HashSet::from([
            small_id.get(),
            boundary_id.get(),
            oversized_id.get(),
        ]))
        .expect("decode selected dictionary values");
    assert_eq!(
        selected.resolve(small_id.get()),
        Some(Resolved::Str(&small))
    );
    assert_eq!(
        selected.resolve(boundary_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: boundary_id,
            stored_bytes: &boundary,
            full_len: DEFAULT_BLOB_THRESHOLD as u64,
            truncated: false,
            full_sha256: None,
        }))
    );
    assert_eq!(
        selected.resolve(oversized_id.get()),
        Some(Resolved::Blob(kronika_format::BlobEntry {
            str_id: oversized_id,
            stored_bytes: &oversized[..DEFAULT_TRUNCATE_LIMIT],
            full_len: oversized.len() as u64,
            truncated: true,
            full_sha256: Some(expected_hash),
        }))
    );
    assert_eq!(selected.resolve(ignored_id.get()), None);
    assert_model_names_resolve(&segment, &dictionary);
}

#[test]
fn finished_dictionary_preserves_boundary_blob_metadata() {
    let directory = tempfile::tempdir().expect("finished tempdir");
    let owner = writer(&directory);
    let finished_address = address(SEGMENT_ID + 1);
    let mut journal =
        Journal::open(&owner, JournalConfig::default()).expect("open finished journal");
    let boundary = vec![b'b'; DEFAULT_BLOB_THRESHOLD];
    let boundary_id = append_text_window(&mut journal, finished_address.id, 200, &boundary);
    write_segment(&journal, &owner, finished_address).expect("publish finished blob output");
    let reader = Reader::open(directory.path()).expect("open finished reader");
    let finished = one_segment(&reader);
    assert_eq!(
        finished
            .path()
            .extension()
            .and_then(std::ffi::OsStr::to_str),
        Some("zms")
    );
    let dictionary = finished.dictionary().expect("finished dictionary");
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

    let catalog = reader
        .catalog_segments(200..=200)
        .expect("catalog-only discovery");
    assert_eq!(catalog.segments.len(), 1, "catalog remains discoverable");
    assert!(catalog.warnings.is_empty());
    let selected = reader
        .open_segment(&catalog.segments[0])
        .expect("open selected catalog");
    assert!(
        selected.rows(OsTopology::CONTRACT.type_id.get()).is_err(),
        "the production row path must reject the damaged selected body"
    );

    let exact = reader
        .catalog_segment(second_address.id.get())
        .expect("exact catalog discovery");
    assert_eq!(
        exact
            .segments
            .iter()
            .map(super::SegmentRef::id)
            .collect::<Vec<_>>(),
        [second_address.id.get()]
    );
    assert!(
        reader
            .catalog_segment(second_address.id.get() + 1)
            .expect("missing exact catalog")
            .segments
            .is_empty()
    );

    let discovery = reader.catalog_discovery().expect("compact discovery");
    assert_eq!(
        discovery.ranges().collect::<Vec<_>>(),
        vec![(100, 100), (200, 200)]
    );
    let selected = discovery
        .segments(200..=200)
        .expect("materialize selected catalog");
    assert_eq!(
        selected
            .segments
            .iter()
            .map(super::SegmentRef::id)
            .collect::<Vec<_>>(),
        [second_address.id.get()]
    );

    let with_predecessor = reader
        .catalog_segments_with_predecessor(200..=200)
        .expect("bounded catalogs with predecessor");
    assert_eq!(
        with_predecessor
            .segments
            .iter()
            .map(super::SegmentRef::id)
            .collect::<Vec<_>>(),
        vec![first_address.id.get(), second_address.id.get()]
    );
}

#[test]
fn snapshot_selection_uses_real_section_moments_across_layouts() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");

    for offset in 0..65_i64 {
        let old = address(SEGMENT_ID - 65 + offset);
        let mut buffers = SectionBuffers::new();
        buffers.push(archiver(1)).expect("unrelated old row fits");
        let part = buffers.flush(&[]).expect("encode").expect("nonempty");
        journal.append(old.id, &part).expect("append old segment");
        write_segment(&journal, &owner, old).expect("publish old segment");
        journal.reset().expect("reset old segment");
    }

    let previous = address(SEGMENT_ID);
    let mut buffers = SectionBuffers::new();
    buffers
        .push(bgwriter_v1(100, 10))
        .expect("previous row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(previous.id, &part).expect("append previous");
    write_segment(&journal, &owner, previous).expect("publish previous");
    journal.reset().expect("reset previous");

    let current_v1 = address(SEGMENT_ID + 1);
    let mut buffers = SectionBuffers::new();
    buffers.push(bgwriter_v1(200, 20)).expect("v1 row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(current_v1.id, &part).expect("append v1");
    write_segment(&journal, &owner, current_v1).expect("publish v1");
    journal.reset().expect("reset v1");

    let current_v2 = address(SEGMENT_ID + 2);
    let mut buffers = SectionBuffers::new();
    buffers.push(bgwriter_v2(200, 30)).expect("v2 row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(current_v2.id, &part).expect("append v2");
    write_segment(&journal, &owner, current_v2).expect("publish v2");
    journal.reset().expect("reset v2");

    let future = address(SEGMENT_ID + 3);
    let mut buffers = SectionBuffers::new();
    buffers
        .push(archiver(240))
        .expect("unrelated current row fits");
    buffers
        .push(bgwriter_v2(300, 40))
        .expect("future section row fits");
    let part = buffers.flush(&[]).expect("encode").expect("nonempty");
    journal.append(future.id, &part).expect("append future");
    write_segment(&journal, &owner, future).expect("publish future");
    journal.reset().expect("reset future");

    let reader = Reader::open(directory.path()).expect("open reader");
    super::reset_snapshot_finished_catalog_reads();
    let selected = reader
        .snapshot_selection(0, 250, "pg_stat_bgwriter", &|| false)
        .expect("select section snapshot");
    assert_eq!(
        super::snapshot_finished_catalog_reads(),
        5,
        "selection reads only the future, equal current contributors, predecessor, and one older catalog"
    );
    assert!(selected.warnings.is_empty());
    assert_eq!(
        selected.anchor.as_ref().map(super::SegmentRef::id),
        Some(current_v2.id.get())
    );
    assert_eq!(
        selected
            .current_segments
            .iter()
            .map(super::SegmentRef::id)
            .collect::<Vec<_>>(),
        vec![current_v1.id.get()],
        "both physical layouts contributing the current moment survive"
    );
    assert_eq!(
        selected
            .predecessor_segments
            .iter()
            .map(super::SegmentRef::id)
            .collect::<Vec<_>>(),
        vec![previous.id.get()],
        "the immediate real predecessor survives"
    );
}

#[test]
fn snapshot_selection_cancels_between_finished_catalogs() {
    let directory = tempfile::tempdir().expect("tempdir");
    let owner = writer(&directory);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");

    for offset in 0..8_i64 {
        let unrelated = address(SEGMENT_ID + offset);
        let mut buffers = SectionBuffers::new();
        buffers.push(archiver(offset)).expect("unrelated row fits");
        let part = buffers.flush(&[]).expect("encode").expect("nonempty");
        journal
            .append(unrelated.id, &part)
            .expect("append unrelated segment");
        write_segment(&journal, &owner, unrelated).expect("publish unrelated segment");
        journal.reset().expect("reset unrelated segment");
    }

    let reader = Reader::open(directory.path()).expect("open reader");
    super::reset_snapshot_finished_catalog_reads();
    let error = reader
        .snapshot_selection(0, 250, "pg_stat_bgwriter", &|| {
            super::snapshot_finished_catalog_reads() >= 2
        })
        .expect_err("selection must honor cancellation");
    assert!(
        matches!(error, ReaderError::Io(ref source) if source.kind() == std::io::ErrorKind::Interrupted),
        "cancellation must be a retryable interrupted read: {error}"
    );
    assert_eq!(
        super::snapshot_finished_catalog_reads(),
        2,
        "no later finished catalog is read after cancellation"
    );
}

fn bgwriter_v1(ts: i64, buffers_clean: i64) -> PgStatBgwriterV1 {
    PgStatBgwriterV1 {
        ts: Ts(ts),
        checkpoints_timed: 0,
        checkpoints_req: 0,
        checkpoint_write_time: 0.0,
        checkpoint_sync_time: 0.0,
        buffers_checkpoint: 0,
        buffers_clean,
        maxwritten_clean: 0,
        buffers_backend: 0,
        buffers_backend_fsync: 0,
        buffers_alloc: 0,
        stats_reset: None,
    }
}

fn bgwriter_v2(ts: i64, buffers_clean: i64) -> PgStatBgwriterV2 {
    PgStatBgwriterV2 {
        ts: Ts(ts),
        buffers_clean,
        maxwritten_clean: 0,
        buffers_alloc: 0,
        stats_reset: None,
    }
}

fn archiver(ts: i64) -> PgStatArchiver {
    PgStatArchiver {
        ts: Ts(ts),
        archived_count: 0,
        last_archived_wal: None,
        last_archived_time: None,
        failed_count: 0,
        last_failed_wal: None,
        last_failed_time: None,
        stats_reset: None,
    }
}
