use super::{CHECKSUM_AT, HEADER_LEN, Index, IndexError, checksum};
use crate::{Number, ObjectSummary, Observation, Sample, SectionSummary};

fn observation(value: Number) -> Observation {
    Observation {
        count: 1,
        first: Some(Sample { ts: 42, value }),
        last: Some(Sample { ts: 42, value }),
        nonnegative_delta: None,
        observed_us: 0,
    }
}

fn health_section(value: u32) -> SectionSummary {
    SectionSummary {
        type_id: 0,
        objects: vec![ObjectSummary {
            identity: Vec::new(),
            observations: vec![observation(Number::U32(value))],
        }],
    }
}

fn loadavg_section() -> SectionSummary {
    SectionSummary {
        type_id: 1_105_001,
        objects: vec![ObjectSummary {
            identity: Vec::new(),
            observations: vec![
                observation(Number::F64(1.0)),
                observation(Number::F64(2.0)),
                observation(Number::F64(3.0)),
                observation(Number::I32(4)),
                observation(Number::I32(5)),
            ],
        }],
    }
}

fn sample() -> Index {
    Index {
        sources: 0b101,
        sections: vec![health_section(5), loadavg_section()],
    }
}

#[test]
fn every_block_roundtrips_with_exact_unsigned_values() {
    let index = sample();
    let bytes = index.encode().expect("encode");
    assert_eq!(Index::decode(&bytes).expect("decode"), index);
}

#[test]
fn a_targeted_decode_returns_only_requested_physical_layouts() {
    let bytes = sample().encode().expect("encode");
    let selected = Index::decode_target(&bytes, &[1_105_001]).expect("target");
    assert_eq!(selected.sections, [loadavg_section()]);
}

#[test]
fn an_unrelated_malformed_block_is_not_decoded() {
    let mut bytes = sample().encode().expect("encode");
    let second_entry = HEADER_LEN + super::ENTRY_LEN;
    let second_offset = u32::from_le_bytes(
        bytes[second_entry + 8..second_entry + 12]
            .try_into()
            .expect("offset"),
    ) as usize;
    let body_at = HEADER_LEN + 2 * super::ENTRY_LEN;
    bytes[body_at + second_offset] = 0;
    let value = checksum(&bytes[..CHECKSUM_AT], &bytes[HEADER_LEN..]);
    bytes[CHECKSUM_AT..HEADER_LEN].copy_from_slice(&value.to_le_bytes());

    let selected = Index::decode_target(&bytes, &[0]).expect("first block");
    assert_eq!(selected.sections.len(), 1);
    assert_eq!(Index::decode(&bytes), Err(IndexError::BadLayout));
}

#[test]
fn changing_the_configured_source_set_changes_the_checksum() {
    let first = sample().encode().expect("first");
    let mut changed = sample();
    changed.sources ^= 0b010;
    let second = changed.encode().expect("second");
    assert_ne!(
        &first[CHECKSUM_AT..HEADER_LEN],
        &second[CHECKSUM_AT..HEADER_LEN]
    );
}
