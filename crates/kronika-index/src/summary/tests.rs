use super::{
    IdentityValue, Number, ObjectSummary, Observation, Sample, SectionSummary, decode_section,
    encode_section,
};

#[test]
fn blob_identity_metadata_roundtrips_without_lossy_text() {
    let observation = Observation {
        count: 1,
        first: Some(Sample {
            ts: -5,
            value: Number::I64(i64::MIN),
        }),
        last: Some(Sample {
            ts: -5,
            value: Number::I64(i64::MIN),
        }),
        nonnegative_delta: None,
        observed_us: 0,
    };
    let section = SectionSummary {
        type_id: 1_201_001,
        objects: vec![ObjectSummary {
            identity: vec![IdentityValue::Blob {
                stored_bytes: vec![0xff, 0x00, b'a'],
                full_len: 99,
                truncated: true,
                full_sha256: Some([3; 32]),
            }],
            observations: vec![observation; 7],
        }],
    };
    let bytes = encode_section(&section).expect("encode");
    assert_eq!(decode_section(&bytes, 1_201_001).expect("decode"), section);
}
