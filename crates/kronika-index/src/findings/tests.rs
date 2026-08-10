use super::{
    Finding, FindingBlock, FindingKind, PriorValue, is_upward_spike, select_baseline,
    upper_tukey_fence,
};

const SECOND: i64 = 1_000_000;
const MINUTE: i64 = 60 * SECOND;

fn point(timestamp: i64, value: f64) -> PriorValue {
    PriorValue { timestamp, value }
}

#[test]
fn five_nearby_values_have_the_expected_tukey_fence() {
    assert_eq!(
        upper_tukey_fence(&[98.0, 99.0, 100.0, 101.0, 102.0]),
        Some(104.0)
    );
    assert!(!is_upward_spike(104.0, &[98.0, 99.0, 100.0, 101.0, 102.0]));
    assert!(is_upward_spike(104.01, &[98.0, 99.0, 100.0, 101.0, 102.0]));
}

#[test]
fn a_zero_baseline_keeps_zero_as_data() {
    assert_eq!(upper_tukey_fence(&[0.0; 5]), Some(0.0));
    assert!(!is_upward_spike(0.0, &[0.0; 5]));
    assert!(is_upward_spike(1.0, &[0.0; 5]));
}

#[test]
fn one_old_outlier_is_excluded_when_five_recent_values_exist() {
    let current = 20 * MINUTE;
    let prior = [
        point(1, 10_000.0),
        point(current - 5 * MINUTE, 98.0),
        point(current - 4 * MINUTE, 99.0),
        point(current - 3 * MINUTE, 100.0),
        point(current - 2 * MINUTE, 101.0),
        point(current - MINUTE, 102.0),
    ];
    let selected = select_baseline(&prior, current).expect("five recent values");
    assert_eq!(selected.len(), 5);
    assert_eq!(
        upper_tukey_fence(&selected.iter().map(|point| point.value).collect::<Vec<_>>()),
        Some(104.0)
    );
}

#[test]
fn selection_uses_every_value_in_the_preceding_fifteen_minutes() {
    let current = 30 * MINUTE;
    let prior: Vec<_> = (0_i32..8)
        .map(|index| point(current - i64::from(8 - index) * MINUTE, f64::from(index)))
        .collect();
    assert_eq!(select_baseline(&prior, current), Some(prior.as_slice()));
}

#[test]
fn a_sparse_series_extends_only_to_the_nearest_five_values() {
    let current = 40 * MINUTE;
    let prior = [
        point(MINUTE, 1.0),
        point(5 * MINUTE, 2.0),
        point(10 * MINUTE, 3.0),
        point(20 * MINUTE, 4.0),
        point(30 * MINUTE, 5.0),
        point(39 * MINUTE, 6.0),
    ];
    assert_eq!(select_baseline(&prior, current), Some(&prior[1..]));
    assert_eq!(select_baseline(&prior[..4], current), None);
}

#[test]
fn event_and_known_bad_roundtrip_in_locator_order() {
    let block = FindingBlock {
        type_id: 2_004_001,
        total_hits: 2,
        truncated: false,
        findings: vec![
            Finding {
                kind: FindingKind::Event,
                field_ordinal: 0,
                row_ordinal: 7,
                timestamp: 10,
            },
            Finding {
                kind: FindingKind::KnownBad,
                field_ordinal: 6,
                row_ordinal: 7,
                timestamp: 10,
            },
        ],
    };
    let bytes = block.encode().expect("encode");
    assert_eq!(FindingBlock::decode(block.type_id, &bytes), Ok(block));
}

#[test]
fn finding_block_rejects_out_of_order_and_truncated_payloads() {
    let block = FindingBlock {
        type_id: 1_100_001,
        total_hits: 2,
        truncated: false,
        findings: vec![
            Finding {
                kind: FindingKind::Spike,
                field_ordinal: 34,
                row_ordinal: 8,
                timestamp: 20,
            },
            Finding {
                kind: FindingKind::KnownBad,
                field_ordinal: 3,
                row_ordinal: 7,
                timestamp: 10,
            },
        ],
    };
    assert!(block.encode().is_err());

    let valid = FindingBlock {
        type_id: 1_100_001,
        total_hits: 0,
        truncated: false,
        findings: Vec::new(),
    }
    .encode()
    .expect("encode");
    assert!(FindingBlock::decode(1_100_001, &valid[..valid.len() - 1]).is_err());
}
