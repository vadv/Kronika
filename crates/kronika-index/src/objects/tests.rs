use super::{Object, SectionObjects, Value, decode, encode};
use crate::file::IndexError;

fn disks() -> SectionObjects {
    SectionObjects {
        type_id: 1_108_001,
        label_count: 3,
        value_count: 2,
        objects: vec![
            Object {
                labels: vec!["8".to_owned(), "0".to_owned(), "sda".to_owned()],
                values: vec![Value::Int(4_000), Value::Float(12.5)],
            },
            Object {
                labels: vec!["8".to_owned(), "16".to_owned(), "sdb".to_owned()],
                values: vec![Value::Int(0), Value::Null],
            },
        ],
    }
}

fn round_trip(sections: &[SectionObjects]) -> Vec<SectionObjects> {
    let mut bytes = Vec::new();
    encode(sections, &mut bytes);
    decode(&bytes).expect("the block just written decodes")
}

#[test]
fn a_block_comes_back_as_it_went_in() {
    assert_eq!(round_trip(&[disks()]), vec![disks()]);
}

#[test]
fn several_sections_keep_their_order() {
    let mut processes = disks();
    processes.type_id = 1_100_001;
    let decoded = round_trip(&[disks(), processes]);
    assert_eq!(
        decoded.iter().map(|s| s.type_id).collect::<Vec<_>>(),
        vec![1_108_001, 1_100_001]
    );
}

#[test]
fn a_section_without_objects_is_still_a_section() {
    let empty = SectionObjects {
        objects: Vec::new(),
        ..disks()
    };
    assert_eq!(round_trip(std::slice::from_ref(&empty)), vec![empty]);
}

#[test]
fn nothing_at_all_encodes_and_decodes() {
    assert_eq!(round_trip(&[]), Vec::new());
}

#[test]
fn a_label_keeps_the_bytes_it_had() {
    let long = "/sys/fs/cgroup/system.slice/postgresql@15-main.service".to_owned();
    let section = SectionObjects {
        label_count: 1,
        value_count: 0,
        objects: vec![Object {
            labels: vec![long.clone()],
            values: Vec::new(),
        }],
        ..disks()
    };
    assert_eq!(round_trip(&[section])[0].objects[0].labels[0], long);
}

#[test]
fn a_float_survives_the_trip_bit_for_bit() {
    let section = SectionObjects {
        label_count: 0,
        value_count: 1,
        objects: vec![Object {
            labels: Vec::new(),
            values: vec![Value::Float(0.1 + 0.2)],
        }],
        ..disks()
    };
    assert_eq!(
        round_trip(&[section])[0].objects[0].values[0],
        Value::Float(0.1 + 0.2)
    );
}

#[test]
fn a_block_cut_short_is_an_error_rather_than_a_panic() {
    let mut bytes = Vec::new();
    encode(&[disks()], &mut bytes);
    for cut in 1..bytes.len() {
        assert_eq!(
            decode(&bytes[..cut]),
            Err(IndexError::Truncated),
            "{cut} bytes decoded as something"
        );
    }
}

#[test]
fn trailing_bytes_are_an_error() {
    let mut bytes = Vec::new();
    encode(&[disks()], &mut bytes);
    bytes.push(0);
    assert_eq!(decode(&bytes), Err(IndexError::Truncated));
}

#[test]
fn an_unknown_value_tag_is_an_error() {
    let mut bytes = Vec::new();
    encode(&[disks()], &mut bytes);
    let tag = bytes
        .windows(1)
        .rposition(|byte| byte == [super::TAG_FLOAT])
        .expect("the sample carries a float");
    bytes[tag] = 9;
    assert_eq!(decode(&bytes), Err(IndexError::Truncated));
}
