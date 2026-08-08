use super::{
    CHECKSUM_AT, ENTRY_LEN, HEADER_LEN, Index, IndexError, MAGIC, POINT_LEN, Point, checksum,
};
use crate::objects::{Object, SectionObjects, Value};

fn sample() -> Index {
    Index {
        sources: 0b11,
        points: vec![
            Point {
                ts: 1_700_000_000_000_000,
                health: None,
            },
            Point {
                ts: 1_700_000_010_000_000,
                health: Some(100),
            },
            Point {
                ts: 1_700_000_020_000_000,
                health: Some(0),
            },
        ],
        objects: Vec::new(),
    }
}

fn with_objects() -> Index {
    Index {
        objects: vec![SectionObjects {
            type_id: 1_108_001,
            label_count: 3,
            value_count: 1,
            objects: vec![Object {
                labels: vec!["8".to_owned(), "0".to_owned(), "sda".to_owned()],
                values: vec![Value::Int(4_000)],
            }],
        }],
        ..sample()
    }
}

/// Tamper with a file and leave it internally consistent, so a test reaches
/// the checks that come after the checksum.
fn restamped(mut bytes: Vec<u8>, edit: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    edit(&mut bytes);
    let stamped = checksum(&bytes[..CHECKSUM_AT], &bytes[HEADER_LEN..]);
    bytes[CHECKSUM_AT..HEADER_LEN].copy_from_slice(&stamped.to_le_bytes());
    bytes
}

#[test]
fn a_file_survives_the_round_trip() {
    let index = sample();
    assert_eq!(Index::decode(&index.encode()), Ok(index));
}

#[test]
fn health_and_objects_travel_together() {
    let index = with_objects();
    assert_eq!(Index::decode(&index.encode()), Ok(index));
}

#[test]
fn an_empty_index_is_a_header_and_nothing_else() {
    let index = Index {
        sources: 0,
        points: Vec::new(),
        objects: Vec::new(),
    };
    let bytes = index.encode();
    assert_eq!(bytes.len(), HEADER_LEN);
    assert_eq!(Index::decode(&bytes), Ok(index));
}

#[test]
fn a_block_with_nothing_in_it_is_left_out() {
    assert_eq!(
        sample().encode().len(),
        HEADER_LEN + ENTRY_LEN + 3 * POINT_LEN,
        "an index without objects still wrote an objects entry"
    );
}

#[test]
fn the_header_checksum_is_readable_without_the_blocks() {
    let bytes = with_objects().encode();
    let from_header = Index::checksum_of(&bytes).expect("read the checksum");
    let from_header_only = Index::checksum_of(&bytes[..HEADER_LEN]).expect("header alone");
    assert_eq!(from_header, from_header_only);
}

#[test]
fn a_rebuild_that_changes_a_value_changes_the_checksum() {
    let before = Index::checksum_of(&sample().encode()).expect("before");
    let mut changed = sample();
    changed.points[1].health = Some(99);
    let after = Index::checksum_of(&changed.encode()).expect("after");
    assert_ne!(before, after);
}

#[test]
fn a_rebuild_that_changes_an_object_changes_the_checksum() {
    let before = Index::checksum_of(&with_objects().encode()).expect("before");
    let mut changed = with_objects();
    changed.objects[0].objects[0].values[0] = Value::Int(4_001);
    let after = Index::checksum_of(&changed.encode()).expect("after");
    assert_ne!(before, after);
}

#[test]
fn a_rebuild_that_changes_sources_changes_the_checksum() {
    let before = Index::checksum_of(&sample().encode()).expect("before");
    let mut changed = sample();
    changed.sources ^= 0b100;
    let after = Index::checksum_of(&changed.encode()).expect("after");
    assert_ne!(before, after);
}

#[test]
fn a_rebuild_that_changes_nothing_keeps_the_checksum() {
    assert_eq!(
        Index::checksum_of(&sample().encode()),
        Index::checksum_of(&sample().encode())
    );
}

#[test]
fn a_flipped_byte_fails_the_checksum() {
    let mut bytes = sample().encode();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    assert_eq!(Index::decode(&bytes), Err(IndexError::BadChecksum));
}

#[test]
fn a_file_that_lost_a_byte_fails_the_checksum() {
    let bytes = sample().encode();
    assert_eq!(
        Index::decode(&bytes[..bytes.len() - 1]),
        Err(IndexError::BadChecksum)
    );
}

#[test]
fn a_foreign_file_is_rejected_by_its_magic() {
    let mut bytes = sample().encode();
    bytes[0] = b'X';
    assert_eq!(Index::decode(&bytes), Err(IndexError::BadMagic));
    assert_eq!(Index::checksum_of(&bytes), Err(IndexError::BadMagic));
}

#[test]
fn a_header_cut_short_is_truncated() {
    let bytes = sample().encode();
    assert_eq!(
        Index::decode(&bytes[..HEADER_LEN - 1]),
        Err(IndexError::Truncated)
    );
}

#[test]
fn a_block_that_runs_past_the_file_is_truncated() {
    let bytes = restamped(sample().encode(), |bytes| {
        let length_at = HEADER_LEN + 8;
        bytes[length_at..length_at + 4].copy_from_slice(&u32::MAX.to_le_bytes());
    });
    assert_eq!(Index::decode(&bytes), Err(IndexError::Truncated));
}

#[test]
fn a_health_block_that_is_not_whole_points_is_truncated() {
    let bytes = restamped(sample().encode(), |bytes| {
        let length_at = HEADER_LEN + 8;
        bytes[length_at..length_at + 4].copy_from_slice(&8_u32.to_le_bytes());
    });
    assert_eq!(Index::decode(&bytes), Err(IndexError::Truncated));
}

#[test]
fn a_block_kind_this_version_does_not_know_is_truncated() {
    let bytes = restamped(sample().encode(), |bytes| {
        bytes[HEADER_LEN..HEADER_LEN + 4].copy_from_slice(&99_u32.to_le_bytes());
    });
    assert_eq!(Index::decode(&bytes), Err(IndexError::Truncated));
}

#[test]
fn the_magic_names_the_file_kind_and_its_version() {
    assert_eq!(&MAGIC, b"KRNIDX2\0");
}
