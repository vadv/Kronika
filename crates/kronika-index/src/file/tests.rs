use super::{HEADER_LEN, Index, IndexError, MAGIC, POINT_LEN, Point};

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
    }
}

#[test]
fn a_file_survives_the_round_trip() {
    let index = sample();
    assert_eq!(Index::decode(&index.encode()), Ok(index));
}

#[test]
fn an_empty_index_is_a_header_and_nothing_else() {
    let index = Index {
        sources: 0,
        points: Vec::new(),
    };
    let bytes = index.encode();
    assert_eq!(bytes.len(), HEADER_LEN);
    assert_eq!(Index::decode(&bytes), Ok(index));
}

#[test]
fn a_point_costs_nine_bytes() {
    assert_eq!(sample().encode().len(), HEADER_LEN + 3 * POINT_LEN);
}

#[test]
fn the_header_checksum_is_readable_without_the_points() {
    let bytes = sample().encode();
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
fn a_rebuild_that_changes_nothing_keeps_the_checksum() {
    assert_eq!(
        Index::checksum_of(&sample().encode()),
        Index::checksum_of(&sample().encode())
    );
}

#[test]
fn a_flipped_point_byte_fails_the_checksum() {
    let mut bytes = sample().encode();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x01;
    assert_eq!(Index::decode(&bytes), Err(IndexError::BadChecksum));
}

#[test]
fn a_foreign_file_is_rejected_by_its_magic() {
    let mut bytes = sample().encode();
    bytes[0] = b'X';
    assert_eq!(Index::decode(&bytes), Err(IndexError::BadMagic));
    assert_eq!(Index::checksum_of(&bytes), Err(IndexError::BadMagic));
}

#[test]
fn another_version_is_named_rather_than_guessed_at() {
    let mut bytes = sample().encode();
    bytes[8] = 7;
    assert_eq!(
        Index::decode(&bytes),
        Err(IndexError::UnsupportedVersion(7))
    );
}

#[test]
fn a_cut_file_is_truncated_whether_the_header_or_the_points_went() {
    let bytes = sample().encode();
    assert_eq!(
        Index::decode(&bytes[..HEADER_LEN - 1]),
        Err(IndexError::Truncated)
    );
    assert_eq!(
        Index::decode(&bytes[..bytes.len() - 1]),
        Err(IndexError::Truncated)
    );
}

#[test]
fn a_body_that_is_not_whole_points_is_truncated() {
    let mut bytes = sample().encode();
    bytes.push(0);
    assert_eq!(Index::decode(&bytes), Err(IndexError::Truncated));
}

#[test]
fn the_magic_names_the_file_kind_and_its_version() {
    assert_eq!(&MAGIC, b"KRNIDX1\0");
}
