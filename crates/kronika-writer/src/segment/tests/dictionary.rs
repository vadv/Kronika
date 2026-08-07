//! Dictionary.

use super::*;

#[test]
fn a_finished_segment_carries_the_window_dictionary() {
    let dir = tempfile::tempdir().expect("tempdir");
    let owner = writer(&dir);
    let segment_path = owner
        .root()
        .diagnostic_file_path(address(), kronika_layout::FileKind::Zms);
    let mut journal = Journal::open(&owner, JournalConfig::default()).expect("open journal");

    // Intern two short strings and encode the window dictionary.
    let mut interner = Interner::new(DictLimits::new(4096, 1 << 20).expect("limits"));
    interner.intern(b"db-host-01").expect("intern");
    interner.intern(b"node-7").expect("intern");
    let dict_sections = dict::encode(interner.window()).expect("encode dictionary");

    // One data section plus the dictionary in a single part.
    let mut buffers = SectionBuffers::new();
    buffers.push(topology(1_000)).expect("buffer not full");
    let part = buffers
        .flush(&dict_sections)
        .expect("flush")
        .expect("a part");
    journal.append(address().id, &part).expect("append");

    let summary = write_segment(&journal, &owner, address()).expect("write the segment");
    assert_eq!(summary.sections, 2, "os_topology + dict.strings");

    let segment = std::fs::read(&segment_path).expect("read segment");
    let catalog = validate_part(&segment).expect("segment validates");
    let dict_entry = catalog
        .entries
        .iter()
        .find(|entry| entry.type_id == DICT_STRINGS_TYPE_ID)
        .expect("the dictionary section reached the segment");
    assert_eq!(dict_entry.rows, 2, "both interned strings");
    let start = usize::try_from(dict_entry.offset).unwrap();
    let end = start + usize::try_from(dict_entry.len).unwrap();
    assert_eq!(&segment[start..start + 4], b"PAR1", "a Parquet dict body");
    assert_eq!(&segment[end - 4..end], b"PAR1", "intact to its last byte");
}
