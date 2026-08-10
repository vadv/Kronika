use super::direct::{CpuRaw, cpu_busy_at_least_80};
use super::spikes::{process_rate, statement_average};
use super::{ProcessRaw, StatementRaw, block, finding_layout};
use crate::{Finding, FindingKind, MAX_FINDINGS_PER_BLOCK};

#[test]
fn process_rate_requires_an_adjacent_nonnegative_counter_delta() {
    let before = ProcessRaw {
        timestamp: 1_000_000,
        read_bytes: Some(100),
    };
    assert_eq!(
        process_rate(
            before,
            ProcessRaw {
                timestamp: 2_000_000,
                read_bytes: Some(300),
            }
        ),
        Some(200.0)
    );
    assert_eq!(
        process_rate(
            before,
            ProcessRaw {
                timestamp: 2_000_000,
                read_bytes: Some(50),
            }
        ),
        None
    );
    assert_eq!(
        process_rate(
            before,
            ProcessRaw {
                timestamp: 2_000_000,
                read_bytes: None,
            }
        ),
        None
    );
}

#[test]
fn statement_average_requires_new_calls_and_nonnegative_time() {
    let before = StatementRaw {
        timestamp: 1,
        calls: 10,
        total_exec_time: 100.0,
    };
    assert_eq!(
        statement_average(
            before,
            StatementRaw {
                timestamp: 2,
                calls: 12,
                total_exec_time: 110.0,
            }
        ),
        Some(5.0)
    );
    assert_eq!(
        statement_average(
            before,
            StatementRaw {
                timestamp: 2,
                calls: 10,
                total_exec_time: 110.0,
            }
        ),
        None
    );
    assert_eq!(
        statement_average(
            before,
            StatementRaw {
                timestamp: 2,
                calls: 12,
                total_exec_time: 90.0,
            }
        ),
        None
    );
}

#[test]
fn aggregate_cpu_busy_uses_the_exact_adjacent_counter_share() {
    let before = CpuRaw {
        timestamp: 1,
        counters: [0; 8],
    };
    assert!(cpu_busy_at_least_80(
        before,
        CpuRaw {
            timestamp: 2,
            counters: [40, 0, 40, 10, 10, 0, 0, 0],
        }
    ));
    assert!(!cpu_busy_at_least_80(
        before,
        CpuRaw {
            timestamp: 2,
            counters: [39, 0, 40, 11, 10, 0, 0, 0],
        }
    ));
}

#[test]
fn the_only_statement_spike_layouts_have_total_exec_time() {
    assert!(!finding_layout(1_002_001));
    for type_id in 1_002_002..=1_002_006 {
        assert!(finding_layout(type_id));
    }
    assert!(!finding_layout(1_004_001));
}

#[test]
fn the_fixed_cap_keeps_timestamp_locator_order_and_reports_omissions() {
    let findings: Vec<_> = (0..=MAX_FINDINGS_PER_BLOCK)
        .rev()
        .map(|ordinal| Finding {
            kind: FindingKind::KnownBad,
            field_ordinal: 1,
            row_ordinal: u32::try_from(ordinal).expect("small test ordinal"),
            timestamp: i64::try_from(ordinal).expect("small test timestamp"),
        })
        .collect();
    let block = block(1_104_001, findings);
    assert_eq!(block.findings.len(), MAX_FINDINGS_PER_BLOCK);
    assert_eq!(block.total_hits, 4_097);
    assert!(block.truncated);
    assert_eq!(block.findings[0].timestamp, 0);
    assert_eq!(
        block.findings.last().map(|finding| finding.timestamp),
        Some(4_095)
    );
}
