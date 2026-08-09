use super::{ActiveBackendPoint, HealthPoint, SeriesBlock, TransactionPoint};

#[test]
fn each_allowlisted_block_roundtrips() {
    let blocks = [
        SeriesBlock::OsHealth(vec![
            HealthPoint {
                timestamp: 10,
                value: None,
            },
            HealthPoint {
                timestamp: 20,
                value: Some(73),
            },
        ]),
        SeriesBlock::OverallHealth(vec![HealthPoint {
            timestamp: 20,
            value: Some(51),
        }]),
        SeriesBlock::PostgresHealth(vec![HealthPoint {
            timestamp: 20,
            value: Some(80),
        }]),
        SeriesBlock::PgbouncerHealth(vec![HealthPoint {
            timestamp: 20,
            value: None,
        }]),
        SeriesBlock::PgTransactions {
            type_id: 1_005_004,
            points: vec![
                TransactionPoint {
                    timestamp: 10,
                    datid: 42,
                    value: None,
                },
                TransactionPoint {
                    timestamp: 20,
                    datid: 42,
                    value: Some(12.5),
                },
            ],
        },
        SeriesBlock::PgActiveBackends {
            type_id: 1_001_003,
            points: vec![ActiveBackendPoint {
                timestamp: 10,
                count: 4,
            }],
        },
    ];
    for block in blocks {
        let bytes = block.encode().expect("encode");
        assert_eq!(
            SeriesBlock::decode(block.key(), &bytes).expect("decode"),
            block
        );
    }
}

#[test]
fn a_real_zero_is_distinct_from_an_unknown_rate() {
    let block = SeriesBlock::PgTransactions {
        type_id: 1_005_003,
        points: vec![
            TransactionPoint {
                timestamp: 10,
                datid: 1,
                value: None,
            },
            TransactionPoint {
                timestamp: 20,
                datid: 1,
                value: Some(0.0),
            },
        ],
    };
    let bytes = block.encode().expect("encode");
    assert_eq!(
        SeriesBlock::decode(block.key(), &bytes).expect("decode"),
        block
    );
}
