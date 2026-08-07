use kronika_format::DictLimits;
use kronika_registry::{Bytes, DICT_BLOBS_TYPE_ID, DICT_STRINGS_TYPE_ID};
use parquet::basic::Encoding;
use parquet::file::reader::{FileReader, SerializedFileReader};

use super::encode;
use crate::Interner;

#[test]
fn write_compatible_limits_are_validated_at_configuration() {
    use super::validate_dict_limits_for_write;

    validate_dict_limits_for_write(DictLimits::new(4096, 64 * 1024).expect("collector limits"))
        .expect("collector limits fit the final page budget");
    let oversized = DictLimits::new(4096, 1024 * 1024).expect("valid but unwritable limits");
    assert!(validate_dict_limits_for_write(oversized).is_err());
}

#[test]
fn an_empty_window_encodes_no_sections() {
    let interner = Interner::new(DictLimits::new(8, 1024).expect("limits"));
    assert!(encode(interner.window()).expect("encode").is_empty());
}

#[test]
fn strings_and_blobs_split_by_placement() {
    // blob_threshold 8: short values are strings, longer ones blobs.
    let mut interner = Interner::new(DictLimits::new(8, 1024).expect("limits"));
    interner.intern(b"short").expect("string");
    interner.intern(b"also").expect("string");
    interner
        .intern(b"a value longer than eight bytes")
        .expect("blob by size");

    let sections = encode(interner.window()).expect("encode");
    assert_eq!(sections.len(), 2, "one strings section, one blobs section");

    let strings = sections
        .iter()
        .find(|s| s.type_id == DICT_STRINGS_TYPE_ID)
        .expect("strings section");
    assert_eq!(strings.rows, 2);
    let blobs = sections
        .iter()
        .find(|s| s.type_id == DICT_BLOBS_TYPE_ID)
        .expect("blobs section");
    assert_eq!(blobs.rows, 1);
    assert_eq!(&blobs.body[..4], b"PAR1", "a Parquet body");
}

#[test]
fn journal_dictionary_columns_have_no_dictionary_pages() {
    let mut interner = Interner::new(DictLimits::new(8, 4096).expect("limits"));
    for value in [
        b"short-a".as_slice(),
        b"short-b".as_slice(),
        b"a value longer than eight bytes".as_slice(),
        b"another value longer than eight bytes".as_slice(),
    ] {
        interner.intern(value).expect("dictionary value");
    }

    let sections = encode(interner.window()).expect("encode journal dictionary");
    assert_eq!(sections.len(), 2, "strings and blobs are both exercised");
    for section in sections {
        let reader = SerializedFileReader::new(Bytes::from(section.body))
            .expect("open journal dictionary metadata");
        for row_group in reader.metadata().row_groups() {
            for column in row_group.columns() {
                assert!(
                    column.dictionary_page_offset().is_none(),
                    "journal dictionary type {} column {} has a dictionary page",
                    section.type_id,
                    column.column_path().string()
                );
                assert!(
                    column.encodings().iter().all(|encoding| !matches!(
                        encoding,
                        Encoding::PLAIN_DICTIONARY | Encoding::RLE_DICTIONARY
                    )),
                    "journal dictionary type {} column {} advertises dictionary encoding",
                    section.type_id,
                    column.column_path().string()
                );
            }
        }
    }
}
