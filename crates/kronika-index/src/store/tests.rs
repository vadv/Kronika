use std::path::Path;

use super::{LoadError, is_dictionary, path_of, read, write};
use crate::file::{Index, Point};
use crate::objects::{Object, SectionObjects, Value};

fn sample() -> Index {
    Index {
        sources: 0b101,
        points: vec![Point {
            ts: 1_700_000_000_000_000,
            health: Some(97),
        }],
        objects: vec![SectionObjects {
            type_id: 1_108_001,
            label_count: 1,
            value_count: 1,
            objects: vec![Object {
                labels: vec!["sda".to_owned()],
                values: vec![Value::Int(4_000)],
            }],
        }],
    }
}

#[test]
fn an_index_lives_beside_its_segment_under_the_same_name() {
    assert_eq!(
        path_of(Path::new("/data/2026/08/08/17.zms")),
        Path::new("/data/2026/08/08/17.idx")
    );
}

#[test]
fn a_written_index_reads_back_as_it_was() {
    let dir = tempfile::tempdir().expect("a directory to write in");
    let path = dir.path().join("17.idx");
    write(&path, &sample()).expect("write the index");
    assert_eq!(read(&path).expect("read it back"), sample());
}

#[test]
fn writing_again_replaces_the_file_and_leaves_no_temporary() {
    let dir = tempfile::tempdir().expect("a directory to write in");
    let path = dir.path().join("17.idx");
    write(&path, &sample()).expect("first write");
    let mut second = sample();
    second.points[0].health = Some(12);
    write(&path, &second).expect("second write");
    assert_eq!(read(&path).expect("read"), second);
    let left: Vec<_> = std::fs::read_dir(dir.path())
        .expect("list the directory")
        .filter_map(Result::ok)
        .map(|entry| entry.file_name())
        .collect();
    assert_eq!(left.len(), 1, "a temporary was left behind: {left:?}");
}

#[test]
fn a_file_that_is_not_there_is_an_io_error() {
    let dir = tempfile::tempdir().expect("a directory");
    assert!(matches!(
        read(&dir.path().join("absent.idx")),
        Err(LoadError::Io(_))
    ));
}

#[test]
fn a_file_that_is_not_an_index_says_so_rather_than_being_read() {
    let dir = tempfile::tempdir().expect("a directory");
    let path = dir.path().join("17.idx");
    std::fs::write(&path, b"not an index at all").expect("write the impostor");
    assert!(matches!(read(&path), Err(LoadError::Bad(_))));
}

#[test]
fn the_dictionaries_are_not_sections_of_rows() {
    assert!(is_dictionary(3_001_001));
    assert!(is_dictionary(3_002_001));
    assert!(!is_dictionary(1_108_001));
}
