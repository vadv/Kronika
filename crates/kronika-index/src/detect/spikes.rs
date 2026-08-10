//! The two explicit upward-spike series.

use kronika_reader::{Cell, Segment};

use crate::build::{BuildError, integer_as_f64};
use crate::findings::{Finding, FindingKind, PriorValue, is_upward_spike, select_baseline};

use super::{
    FindingBuilder, OS_PROCESS, PROCESS_READ_BYTES_FIELD, ProcessId, ProcessRaw, StatementId,
    StatementRaw, optional_i64, statement_layouts,
};

impl FindingBuilder {
    pub(super) fn discover_processes(&mut self, segment: &Segment) -> Result<(), BuildError> {
        if !self.requested.contains(&OS_PROCESS) || segment.rows_of(OS_PROCESS).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_PROCESS,
            &["pid", "starttime"],
            0,
            usize::MAX,
            |_ordinal, row| {
                if let (Some(Cell::I32(pid)), Some(Cell::Ts(starttime))) =
                    (row.get("pid"), row.get("starttime"))
                {
                    self.process
                        .entry(ProcessId {
                            pid: *pid,
                            starttime: *starttime,
                        })
                        .or_default();
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn discover_statements(&mut self, segment: &Segment) -> Result<(), BuildError> {
        for type_id in statement_layouts() {
            if !self.requested.contains(&type_id) || segment.rows_of(type_id).is_none() {
                continue;
            }
            let columns = statement_columns(type_id);
            let identities = self.statements.entry(type_id).or_default();
            segment.visit_rows(type_id, columns, 0, usize::MAX, |_ordinal, row| {
                if let Some(identity) = statement_identity(type_id, &row) {
                    identities.entry(identity).or_default();
                }
                true
            })?;
        }
        Ok(())
    }

    pub(super) fn observe_processes(
        &mut self,
        segment: &Segment,
        current: bool,
        hits: &mut Vec<Finding>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(OS_PROCESS).is_none() {
            return Ok(());
        }
        segment.visit_rows(
            OS_PROCESS,
            &["ts", "pid", "starttime", "read_bytes"],
            0,
            usize::MAX,
            |ordinal, row| {
                let (Some(Cell::Ts(timestamp)), Some(Cell::I32(pid)), Some(Cell::Ts(starttime))) =
                    (row.get("ts"), row.get("pid"), row.get("starttime"))
                else {
                    return true;
                };
                let identity = ProcessId {
                    pid: *pid,
                    starttime: *starttime,
                };
                let Some(history) = self.process.get_mut(&identity) else {
                    return true;
                };
                let read_bytes = optional_i64(row.get("read_bytes"));
                let raw = ProcessRaw {
                    timestamp: *timestamp,
                    read_bytes,
                };
                let value = history.before.and_then(|before| process_rate(before, raw));
                history.before = Some(raw);
                if let Some(value) = value {
                    if current {
                        if baseline_is_spike(&history.values, *timestamp, value)
                            && let Some(row_ordinal) = u32::try_from(ordinal).ok()
                        {
                            hits.push(Finding {
                                kind: FindingKind::Spike,
                                category: None,
                                field_ordinal: PROCESS_READ_BYTES_FIELD,
                                row_ordinal,
                                timestamp: *timestamp,
                            });
                        }
                        history.values.push(PriorValue {
                            timestamp: *timestamp,
                            value,
                        });
                    } else {
                        push_prior(&mut history.values, self.cutoff, *timestamp, value);
                    }
                }
                true
            },
        )?;
        Ok(())
    }

    pub(super) fn observe_statements(
        &mut self,
        segment: &Segment,
        type_id: u32,
        current: bool,
        hits: &mut Vec<Finding>,
    ) -> Result<(), BuildError> {
        if segment.rows_of(type_id).is_none() {
            return Ok(());
        }
        let columns = statement_value_columns(type_id);
        let Some(identities) = self.statements.get_mut(&type_id) else {
            return Ok(());
        };
        segment.visit_rows(type_id, columns, 0, usize::MAX, |ordinal, row| {
            let Some(identity) = statement_identity(type_id, &row) else {
                return true;
            };
            let Some(history) = identities.get_mut(&identity) else {
                return true;
            };
            let (
                Some(Cell::Ts(timestamp)),
                Some(Cell::I64(calls)),
                Some(Cell::F64(total_exec_time)),
            ) = (row.get("ts"), row.get("calls"), row.get("total_exec_time"))
            else {
                return true;
            };
            let raw = StatementRaw {
                timestamp: *timestamp,
                calls: *calls,
                total_exec_time: *total_exec_time,
            };
            let value = history
                .before
                .and_then(|before| statement_average(before, raw));
            history.before = Some(raw);
            if let Some(value) = value {
                if current {
                    if baseline_is_spike(&history.values, *timestamp, value)
                        && let Some(row_ordinal) = u32::try_from(ordinal).ok()
                    {
                        hits.push(Finding {
                            kind: FindingKind::Spike,
                            category: None,
                            field_ordinal: statement_total_time_field(type_id),
                            row_ordinal,
                            timestamp: *timestamp,
                        });
                    }
                    history.values.push(PriorValue {
                        timestamp: *timestamp,
                        value,
                    });
                } else {
                    push_prior(&mut history.values, self.cutoff, *timestamp, value);
                }
            }
            true
        })?;
        Ok(())
    }
}

pub(super) fn process_rate(before: ProcessRaw, current: ProcessRaw) -> Option<f64> {
    let elapsed = current.timestamp.checked_sub(before.timestamp)?;
    let delta = current.read_bytes?.checked_sub(before.read_bytes?)?;
    if elapsed <= 0 || delta < 0 {
        return None;
    }
    let value =
        integer_as_f64(i128::from(delta))? * 1_000_000.0 / integer_as_f64(i128::from(elapsed))?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

pub(super) fn statement_average(before: StatementRaw, current: StatementRaw) -> Option<f64> {
    if current.timestamp <= before.timestamp
        || !before.total_exec_time.is_finite()
        || !current.total_exec_time.is_finite()
    {
        return None;
    }
    let calls = current.calls.checked_sub(before.calls)?;
    let total = current.total_exec_time - before.total_exec_time;
    if calls <= 0 || !total.is_finite() || total < 0.0 {
        return None;
    }
    let value = total / integer_as_f64(i128::from(calls))?;
    (value.is_finite() && value >= 0.0).then_some(value)
}

fn baseline_is_spike(history: &[PriorValue], timestamp: i64, current: f64) -> bool {
    let Some(selected) = select_baseline(history, timestamp) else {
        return false;
    };
    let values: Vec<f64> = selected.iter().map(|point| point.value).collect();
    is_upward_spike(current, &values)
}

fn push_prior(history: &mut Vec<PriorValue>, cutoff: i64, timestamp: i64, value: f64) {
    if timestamp < cutoff
        && history.len() == 5
        && history.iter().all(|point| point.timestamp < cutoff)
    {
        history.remove(0);
    }
    history.push(PriorValue { timestamp, value });
}

fn statement_identity(type_id: u32, row: &kronika_reader::Row) -> Option<StatementId> {
    let queryid = match row.get("queryid")? {
        Cell::I64(value) => Some(*value),
        Cell::Null => None,
        _ => return None,
    };
    let (Some(Cell::U32(userid)), Some(Cell::U32(dbid))) = (row.get("userid"), row.get("dbid"))
    else {
        return None;
    };
    let toplevel = if type_id == 1_002_002 {
        None
    } else {
        match row.get("toplevel") {
            Some(Cell::Bool(value)) => Some(*value),
            _ => return None,
        }
    };
    Some(StatementId {
        queryid,
        userid: *userid,
        dbid: *dbid,
        toplevel,
    })
}

const fn statement_columns(type_id: u32) -> &'static [&'static str] {
    if type_id == 1_002_002 {
        &["queryid", "userid", "dbid"]
    } else {
        &["queryid", "userid", "dbid", "toplevel"]
    }
}

const fn statement_value_columns(type_id: u32) -> &'static [&'static str] {
    if type_id == 1_002_002 {
        &[
            "ts",
            "queryid",
            "userid",
            "dbid",
            "calls",
            "total_exec_time",
        ]
    } else {
        &[
            "ts",
            "queryid",
            "userid",
            "dbid",
            "toplevel",
            "calls",
            "total_exec_time",
        ]
    }
}

const fn statement_total_time_field(type_id: u32) -> u16 {
    if type_id == 1_002_002 { 10 } else { 11 }
}
