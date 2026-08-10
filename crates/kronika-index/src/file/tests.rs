use super::{CHECKSUM_AT, HEADER_LEN, Index, IndexError, checksum};
use crate::{
    ActiveBackendPoint, Finding, FindingBlock, FindingKind, HealthPoint, SeriesBlock, SeriesKey,
    SeriesKind, TransactionPoint,
};

fn sample() -> Index {
    Index {
        blocks: vec![
            SeriesBlock::OsHealth(vec![HealthPoint {
                timestamp: 42,
                value: Some(95),
            }]),
            SeriesBlock::PgTransactions {
                type_id: 1_005_004,
                points: vec![TransactionPoint {
                    timestamp: 42,
                    datid: 7,
                    value: Some(1.5),
                }],
            },
            SeriesBlock::PgActiveBackends {
                type_id: 1_001_003,
                points: vec![ActiveBackendPoint {
                    timestamp: 42,
                    count: 4,
                }],
            },
            SeriesBlock::Findings(FindingBlock {
                type_id: 1_100_001,
                total_hits: 1,
                truncated: false,
                findings: vec![Finding {
                    kind: FindingKind::Spike,
                    field_ordinal: 34,
                    row_ordinal: 7,
                    start_ts: 30,
                    end_ts: 42,
                }],
            }),
        ],
    }
}

#[test]
fn current_format_roundtrips() {
    let index = sample();
    let bytes = index.encode().expect("encode");
    assert_eq!(Index::decode(&bytes).expect("decode"), index);
}

#[test]
fn targeted_decode_never_allocates_unrequested_blocks() {
    let bytes = sample().encode().expect("encode");
    let selected = Index::decode_target(
        &bytes,
        &[SeriesKey {
            kind: SeriesKind::PgTransactionsPerSecond,
            type_id: 1_005_004,
        }],
    )
    .expect("target");
    assert!(matches!(
        selected.blocks.as_slice(),
        [SeriesBlock::PgTransactions { .. }]
    ));
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
    let body_at = HEADER_LEN + 4 * super::ENTRY_LEN;
    bytes[body_at + second_offset] = 0xff;
    let value = checksum(&bytes[..CHECKSUM_AT], &bytes[HEADER_LEN..]);
    bytes[CHECKSUM_AT..HEADER_LEN].copy_from_slice(&value.to_le_bytes());

    let selected = Index::decode_target(&bytes, &[SeriesKey::OS_HEALTH]).expect("first block");
    assert_eq!(selected.blocks.len(), 1);
    assert_eq!(Index::decode(&bytes), Err(IndexError::BadLayout));
}

#[test]
fn the_allowlist_stays_small_at_far_more_than_one_normal_segment() {
    let index = Index {
        blocks: vec![
            SeriesBlock::OsHealth(
                (0..1_000)
                    .map(|point| HealthPoint {
                        timestamp: point,
                        value: Some(100),
                    })
                    .collect(),
            ),
            SeriesBlock::PgTransactions {
                type_id: 1_005_004,
                points: (0_u32..100)
                    .flat_map(|datid| {
                        (0_i64..100).map(move |timestamp| TransactionPoint {
                            timestamp,
                            datid,
                            value: Some(1.0),
                        })
                    })
                    .collect(),
            },
            SeriesBlock::PgActiveBackends {
                type_id: 1_001_003,
                points: (0..1_000)
                    .map(|timestamp| ActiveBackendPoint {
                        timestamp,
                        count: 4,
                    })
                    .collect(),
            },
        ],
    };
    assert!(index.encode().expect("encode").len() < 256 * 1024);
}
