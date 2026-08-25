use kronika_reader::{Cell, Row};

pub(super) const FIELDS: &str = "active_count active_pct mean_exec_ms buffer_read_pct \
wal_bytes_per_call calls_per_active rollback_pct temp_bytes_per_transaction seq_scan_pct \
hot_update_pct dead_tuple_pct vacuumed_pct toast_pct xid_boundary_pct scanned_pct no_scan_pct usable_pct";

pub(super) const WANTED: &str = "calls total_exec_time total_time shared_blks_read \
shared_blks_hit local_blks_read local_blks_hit wal_bytes datid xact_commit xact_rollback \
temp_bytes blks_read blks_hit seq_scan idx_scan n_tup_upd n_tup_hot_upd n_live_tup \
n_dead_tup vacuum_count autovacuum_count main_fork_bytes toast_bytes heap_blks_read \
heap_blks_hit idx_blks_read idx_blks_hit toast_blks_read toast_blks_hit tidx_blks_read \
tidx_blks_hit xid_age indisvalid indisready";

pub(super) type Values = [Option<f64>; 17];

#[derive(Clone, Default)]
pub(super) struct Totals {
    values: [f64; 12],
}

impl Totals {
    fn add(&mut self, at: usize, value: Option<f64>) {
        self.values[at] += value.filter(|value| value.is_finite()).unwrap_or(f64::NAN);
    }

    fn get(&self, at: usize) -> Option<f64> {
        self.values[at].is_finite().then_some(self.values[at])
    }

    fn merge(&mut self, other: &Self) {
        for (left, right) in self.values.iter_mut().zip(other.values) {
            *left += right;
        }
    }
}

#[derive(Clone)]
pub(super) enum Summary {
    Dense(f64, Totals),
    Database(Totals),
    Table(f64, f64, Totals),
    Index(f64, Totals),
}

impl Summary {
    pub(super) fn new(surface: u8) -> Self {
        match surface {
            1 | 2 => Self::Dense(0.0, Totals::default()),
            3 => Self::Database(Totals::default()),
            4 => Self::Table(0.0, 0.0, Totals::default()),
            5 => Self::Index(0.0, Totals::default()),
            _ => unreachable!("fixed PostgreSQL summary surface"),
        }
    }

    pub(super) fn add(&mut self, row: &Row, before: Option<&Row>) {
        match self {
            Self::Dense(total, sums) => {
                *total += 1.0;
                let calls = delta(row, before, "calls");
                sums.add(0, calls.map(nonzero));
                sums.add(1, calls);
                let execution = row
                    .get("total_exec_time")
                    .map_or("total_time", |_| "total_exec_time");
                sums.add(2, delta(row, before, execution));
                sums.add(3, deltas(row, before, "shared_blks_read local_blks_read"));
                sums.add(
                    4,
                    deltas(
                        row,
                        before,
                        "shared_blks_read shared_blks_hit local_blks_read local_blks_hit",
                    ),
                );
                sums.add(5, delta(row, before, "wal_bytes"));
            }
            Self::Database(sums) => {
                sums.add(0, delta(row, before, "xact_commit"));
                sums.add(1, delta(row, before, "xact_rollback"));
                sums.add(2, delta(row, before, "temp_bytes"));
                sums.add(3, delta(row, before, "blks_read"));
                sums.add(4, deltas(row, before, "blks_read blks_hit"));
            }
            Self::Table(total, xid_total, sums) => {
                *total += 1.0;
                sums.add(0, delta(row, before, "seq_scan"));
                sums.add(1, optional_delta(row, before, "idx_scan"));
                sums.add(2, delta(row, before, "n_tup_upd"));
                sums.add(3, delta(row, before, "n_tup_hot_upd"));
                sums.add(4, number(row, "n_live_tup"));
                sums.add(5, number(row, "n_dead_tup"));
                sums.add(
                    6,
                    deltas(row, before, "vacuum_count autovacuum_count").map(nonzero),
                );
                sums.add(7, number(row, "main_fork_bytes"));
                sums.add(8, number(row, "toast_bytes").or(Some(0.0)));
                let (reads, blocks) = table_buffers(row, before);
                sums.add(9, reads);
                sums.add(10, blocks);
                if let Some(age) = number(row, "xid_age") {
                    *xid_total += 1.0;
                    sums.add(11, Some(flag(age >= 1_600_000_000.0)));
                }
            }
            Self::Index(total, sums) => {
                *total += 1.0;
                let scans = delta(row, before, "idx_scan");
                sums.add(0, scans.map(nonzero));
                sums.add(1, scans.map(|value| flag(value == 0.0)));
                sums.add(2, delta(row, before, "idx_blks_read"));
                sums.add(3, deltas(row, before, "idx_blks_read idx_blks_hit"));
                let usable = matches!(row.get("indisvalid"), Some(Cell::Bool(true)))
                    && matches!(row.get("indisready"), Some(Cell::Bool(true)));
                sums.add(4, Some(flag(usable)));
            }
        }
    }

    pub(super) fn merge(&mut self, other: &Self) {
        match (self, other) {
            (Self::Table(total, xid_total, sums), Self::Table(b, x, s)) => {
                *total += b;
                *xid_total += x;
                sums.merge(s);
            }
            (Self::Index(total, sums), Self::Index(b, s)) => {
                *total += b;
                sums.merge(s);
            }
            _ => {}
        }
    }

    pub(super) fn values(&self, surface: u8) -> Values {
        let mut out = [None; 17];
        match self {
            Self::Dense(total, sums) => {
                out[0] = sums.get(0);
                out[1] = pct(sums.get(0), Some(*total));
                out[2] = ratio(sums.get(2), sums.get(1));
                out[3] = pct(sums.get(3), sums.get(4));
                if surface == 1 {
                    out[4] = ratio(sums.get(5), sums.get(1));
                } else {
                    out[5] = ratio(sums.get(1), sums.get(0));
                }
            }
            Self::Database(sums) => {
                let tx = plus(sums, 0, 1);
                out[6] = pct(sums.get(1), tx);
                out[7] = ratio(sums.get(2), tx);
                out[3] = pct(sums.get(3), sums.get(4));
            }
            Self::Table(total, xid_total, sums) => {
                out[8] = pct(sums.get(0), plus(sums, 0, 1));
                out[9] = pct(sums.get(3), sums.get(2));
                out[10] = pct(sums.get(5), plus(sums, 4, 5));
                out[11] = pct(sums.get(6), Some(*total));
                out[12] = pct(sums.get(8), plus(sums, 7, 8));
                out[3] = pct(sums.get(9), sums.get(10));
                out[13] = pct(sums.get(11), Some(*xid_total));
            }
            Self::Index(total, sums) => {
                out[14] = pct(sums.get(0), Some(*total));
                out[15] = pct(sums.get(1), Some(*total));
                out[3] = pct(sums.get(2), sums.get(3));
                out[16] = pct(sums.get(4), Some(*total));
            }
        }
        out
    }
}

fn nonzero(value: f64) -> f64 {
    flag(value > 0.0)
}

fn flag(value: bool) -> f64 {
    f64::from(u8::from(value))
}

fn plus(sums: &Totals, left: usize, right: usize) -> Option<f64> {
    sums.get(left).zip(sums.get(right)).map(|(a, b)| a + b)
}

fn ratio(part: Option<f64>, total: Option<f64>) -> Option<f64> {
    let (Some(part), Some(total)) = (part, total) else {
        return None;
    };
    (total > 0.0).then_some(part / total)
}

fn pct(part: Option<f64>, total: Option<f64>) -> Option<f64> {
    ratio(part, total).map(|value| 100.0 * value)
}

#[expect(
    clippy::cast_precision_loss,
    reason = "counters become floating point only for summary division"
)]
fn number(row: &Row, name: &str) -> Option<f64> {
    match row.get(name) {
        Some(Cell::F64(value)) => Some(*value),
        cell => integer(cell).map(|value| value as f64),
    }
}

pub(super) fn integer(cell: Option<&Cell>) -> Option<i128> {
    match cell {
        Some(Cell::I16(v)) => Some(i128::from(*v)),
        Some(Cell::I32(v)) => Some(i128::from(*v)),
        Some(Cell::I64(v) | Cell::Ts(v)) => Some(i128::from(*v)),
        Some(Cell::U32(v)) => Some(i128::from(*v)),
        Some(Cell::U64(v) | Cell::StrId(v)) => Some(i128::from(*v)),
        Some(Cell::Bool(v)) => Some(i128::from(*v)),
        _ => None,
    }
}

fn delta(row: &Row, before: Option<&Row>, name: &str) -> Option<f64> {
    let value = number(row, name)? - number(before?, name)?;
    (value >= 0.0 && value.is_finite()).then_some(value)
}

fn deltas(row: &Row, before: Option<&Row>, names: &str) -> Option<f64> {
    names
        .split_ascii_whitespace()
        .try_fold(0.0, |sum, name| Some(sum + delta(row, before, name)?))
}

fn optional_delta(row: &Row, before: Option<&Row>, name: &str) -> Option<f64> {
    let before = before?;
    if matches!(row.get(name), Some(Cell::Null)) && matches!(before.get(name), Some(Cell::Null)) {
        Some(0.0)
    } else {
        delta(row, Some(before), name)
    }
}

fn table_buffers(row: &Row, before: Option<&Row>) -> (Option<f64>, Option<f64>) {
    let reads = optional_deltas(
        row,
        before,
        "heap_blks_read idx_blks_read toast_blks_read tidx_blks_read",
    );
    let hits = optional_deltas(
        row,
        before,
        "heap_blks_hit idx_blks_hit toast_blks_hit tidx_blks_hit",
    );
    (reads, reads.zip(hits).map(|(reads, hits)| reads + hits))
}

fn optional_deltas(row: &Row, before: Option<&Row>, names: &str) -> Option<f64> {
    names.split_ascii_whitespace().try_fold(0.0, |sum, name| {
        Some(sum + optional_delta(row, before, name)?)
    })
}
