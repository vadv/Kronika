//! Parse `/proc/stat` CPU lines (`1_102`) and the misc counters (`1_103`).

use std::num::ParseIntError;

use kronika_registry::Ts;
use kronika_registry::os_cpu::OsCpu;
use kronika_registry::os_stat::OsStat;

/// One CPU's ticks; `cpu_id = -1` is the aggregate line.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CpuRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// `-1` for the aggregate `cpu` line, else the CPU index.
    pub cpu_id: i32,
    /// Ticks in user mode.
    pub user: i64,
    /// Ticks in user mode with low priority (nice).
    pub nice: i64,
    /// Ticks in system (kernel) mode.
    pub system: i64,
    /// Ticks idle.
    pub idle: i64,
    /// Ticks waiting for I/O to complete.
    pub iowait: i64,
    /// Ticks serving hardware interrupts.
    pub irq: i64,
    /// Ticks serving software interrupts.
    pub softirq: i64,
    /// Ticks stolen by a hypervisor.
    pub steal: i64,
    /// Ticks spent running a virtual CPU for a guest OS.
    pub guest: i64,
    /// Ticks spent running a niced guest.
    pub guest_nice: i64,
}

/// Parse error for procfs lines.
#[derive(Debug)]
pub struct ParseError(pub String);

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.0)
    }
}
impl std::error::Error for ParseError {}
impl From<ParseIntError> for ParseError {
    fn from(e: ParseIntError) -> Self {
        Self(format!("integer: {e}"))
    }
}

/// Parse every `cpu`/`cpuN` line. Ten time fields; missing trailing fields
/// (older kernels) default to `0` per the `/proc/stat` contract.
///
/// # Errors
///
/// Returns [`ParseError`] when an integer field cannot be parsed, or when no
/// `cpu` lines are present.
pub fn parse_cpu(content: &str, ts: i64) -> Result<Vec<CpuRow>, ParseError> {
    let mut rows = Vec::new();
    for line in content.lines() {
        let Some(rest) = line.strip_prefix("cpu") else {
            continue;
        };
        let Some((id_part, values)) = rest.split_once(char::is_whitespace) else {
            continue;
        };
        let cpu_id = if id_part.is_empty() {
            -1
        } else {
            // Skip lines like `cpufreq` that start with "cpu" but are not cpu lines.
            if !id_part.starts_with(|c: char| c.is_ascii_digit()) {
                continue;
            }
            id_part
                .parse::<i32>()
                .map_err(|e| ParseError(format!("cpu id {id_part:?}: {e}")))?
        };
        let mut f = values.split_whitespace();
        let mut next = || -> Result<i64, ParseError> {
            f.next()
                .map_or(Ok(0), |s| s.parse::<i64>().map_err(Into::into))
        };
        rows.push(CpuRow {
            ts,
            cpu_id,
            user: next()?,
            nice: next()?,
            system: next()?,
            idle: next()?,
            iowait: next()?,
            irq: next()?,
            softirq: next()?,
            steal: next()?,
            guest: next()?,
            guest_nice: next()?,
        });
    }
    if rows.is_empty() {
        return Err(ParseError("/proc/stat: no cpu lines".to_owned()));
    }
    Ok(rows)
}

impl CpuRow {
    /// Registry row for `1_102_001` with the given scope.
    #[must_use]
    pub const fn to_section(self, scope: u8) -> OsCpu {
        OsCpu {
            ts: Ts(self.ts),
            cpu_id: self.cpu_id,
            user: self.user,
            nice: self.nice,
            system: self.system,
            idle: self.idle,
            iowait: self.iowait,
            irq: self.irq,
            softirq: self.softirq,
            steal: self.steal,
            guest: self.guest,
            guest_nice: self.guest_nice,
            scope,
        }
    }
}

/// Misc `/proc/stat` singleton counters for `1_103_001`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatMiscRow {
    /// Collection timestamp, unix microseconds.
    pub ts: i64,
    /// Context switches since boot.
    pub ctxt: i64,
    /// Processes forked since boot.
    pub processes: i64,
    /// Processes in runnable state at collection time.
    pub procs_running: i64,
    /// Processes blocked waiting for I/O at collection time.
    pub procs_blocked: i64,
    /// Kernel boot time, unix microseconds (`btime_secs * 1_000_000`).
    pub btime: i64,
    /// Hardware interrupts since boot, summed over every line.
    pub intr_total: Option<i64>,
    /// Software interrupts since boot, summed over every vector.
    pub softirq_total: Option<i64>,
    /// Seconds since boot from `/proc/uptime`, microseconds.
    pub uptime_us: Option<i64>,
    /// Cumulative core idle time from `/proc/uptime`, microseconds.
    pub idle_us: Option<i64>,
}

/// Parse the misc singleton lines from `/proc/stat` content.
///
/// Required fields: `ctxt`, `processes`, `procs_running`, `procs_blocked`,
/// `btime`. Missing or unparsable `btime` is an error; the section is skipped.
///
/// # Errors
///
/// Returns [`ParseError`] when a required field is absent or its integer value
/// cannot be parsed, or when `btime` overflows microseconds.
pub fn parse_stat_misc(content: &str, ts: i64) -> Result<StatMiscRow, ParseError> {
    let mut ctxt: Option<i64> = None;
    let mut processes: Option<i64> = None;
    let mut procs_running: Option<i64> = None;
    let mut procs_blocked: Option<i64> = None;
    let mut btime: Option<i64> = None;
    let mut intr_total: Option<i64> = None;
    let mut softirq_total: Option<i64> = None;

    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("intr ") {
            intr_total = rest.split_whitespace().next().and_then(|v| v.parse().ok());
            continue;
        }
        if let Some(rest) = line.strip_prefix("softirq ") {
            softirq_total = rest.split_whitespace().next().and_then(|v| v.parse().ok());
            continue;
        }
        if let Some(rest) = line.strip_prefix("ctxt ") {
            ctxt = Some(rest.trim().parse::<i64>()?);
        } else if let Some(rest) = line.strip_prefix("btime ") {
            let secs = rest.trim().parse::<i64>()?;
            let usecs = secs
                .checked_mul(1_000_000)
                .ok_or_else(|| ParseError(format!("btime {secs} overflows microseconds")))?;
            btime = Some(usecs);
        } else if let Some(rest) = line.strip_prefix("processes ") {
            processes = Some(rest.trim().parse::<i64>()?);
        } else if let Some(rest) = line.strip_prefix("procs_running ") {
            procs_running = Some(rest.trim().parse::<i64>()?);
        } else if let Some(rest) = line.strip_prefix("procs_blocked ") {
            procs_blocked = Some(rest.trim().parse::<i64>()?);
        }
    }

    let require = |opt: Option<i64>, name: &'static str| {
        opt.ok_or_else(|| ParseError(format!("/proc/stat: missing field {name:?}")))
    };

    Ok(StatMiscRow {
        ts,
        ctxt: require(ctxt, "ctxt")?,
        processes: require(processes, "processes")?,
        procs_running: require(procs_running, "procs_running")?,
        procs_blocked: require(procs_blocked, "procs_blocked")?,
        btime: require(btime, "btime")?,
        intr_total,
        softirq_total,
        uptime_us: None,
        idle_us: None,
    })
}

/// Parse `/proc/uptime` into `(uptime, idle)` microseconds.
///
/// Both fields are non-negative seconds with two decimals. Returns `None` when
/// the file is empty or either token is unparsable; the caller leaves the
/// columns null rather than guessing. Parsed as fixed point rather than
/// through `f64` so a long-running host keeps microsecond precision.
#[must_use]
pub fn parse_uptime(content: &str) -> Option<(i64, i64)> {
    fn seconds_to_us(token: &str) -> Option<i64> {
        let (whole, frac) = token.split_once('.').unwrap_or((token, "0"));
        let seconds = whole.parse::<i64>().ok()?;
        if seconds < 0 {
            return None;
        }
        // The kernel prints hundredths; accept anything up to microseconds.
        let mut digits = frac.as_bytes().to_vec();
        digits.resize(6, b'0');
        let micros: i64 = std::str::from_utf8(&digits[..6]).ok()?.parse().ok()?;
        seconds.checked_mul(1_000_000)?.checked_add(micros)
    }
    let mut fields = content.split_whitespace();
    let uptime = seconds_to_us(fields.next()?)?;
    let idle = seconds_to_us(fields.next()?)?;
    Some((uptime, idle))
}

impl StatMiscRow {
    /// Registry row for `1_103_001` with the given scope.
    #[must_use]
    pub const fn to_section(self, scope: u8) -> OsStat {
        OsStat {
            ts: Ts(self.ts),
            ctxt: self.ctxt,
            processes: self.processes,
            procs_running: self.procs_running,
            procs_blocked: self.procs_blocked,
            btime: Ts(self.btime),
            intr_total: self.intr_total,
            softirq_total: self.softirq_total,
            uptime_us: self.uptime_us,
            idle_us: self.idle_us,
            scope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::parse_cpu;

    const SAMPLE: &str = "cpu  100 20 30 400 5 6 7 8 9 10\n\
                          cpu0 50 10 15 200 2 3 3 4 4 5\n\
                          cpu1 50 10 15 200 3 3 4 4 5 5\n\
                          intr 999\nctxt 12345\n";

    #[test]
    fn parses_aggregate_and_per_cpu() {
        let rows = parse_cpu(SAMPLE, 1_700).expect("parse");
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].cpu_id, -1);
        assert_eq!(rows[0].user, 100);
        assert_eq!(rows[1].cpu_id, 0);
        assert_eq!(rows[2].cpu_id, 1);
        assert_eq!(rows[0].guest_nice, 10);
    }

    #[test]
    fn old_kernel_missing_trailing_fields_default_to_zero() {
        let rows = parse_cpu("cpu 100 20 30 400 5 6 7\n", 1).expect("parse");
        assert_eq!(rows[0].steal, 0);
        assert_eq!(rows[0].guest, 0);
    }

    #[test]
    fn garbled_cpu_line_errors() {
        assert!(parse_cpu("cpu notanumber 2 3\n", 1).is_err());
    }

    #[test]
    fn non_cpu_line_starting_with_cpu_is_skipped_not_an_error() {
        // "cpufreq" must not cause a parse error; only cpu/cpuN lines are parsed.
        let input = "cpufreq 100\ncpu 10 20 30 40 5 6 7 8 9 10\n";
        let rows = parse_cpu(input, 1).expect("parse");
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].cpu_id, -1);
    }

    use super::{parse_stat_misc, parse_uptime};

    const STAT_SAMPLE: &str = "cpu  100 20 30 400 5 6 7 8 9 10\n\
                               cpu0 50 10 15 200 2 3 3 4 4 5\n\
                               intr 999\n\
                               ctxt 12345\n\
                               btime 1700000000\n\
                               processes 42\n\
                               procs_running 3\n\
                               procs_blocked 1\n\
                               softirq 100 0 50 0 10 0 0 0 10 0 30\n";

    #[test]
    fn parses_all_five_misc_fields() {
        let row = parse_stat_misc(STAT_SAMPLE, 9_999).expect("parse");
        assert_eq!(row.ts, 9_999);
        assert_eq!(row.ctxt, 12_345);
        assert_eq!(row.btime, 1_700_000_000_000_000);
        assert_eq!(row.processes, 42);
        assert_eq!(row.procs_running, 3);
        assert_eq!(row.procs_blocked, 1);
    }

    #[test]
    fn missing_btime_is_an_error() {
        let no_btime = "ctxt 1\nprocesses 2\nprocs_running 1\nprocs_blocked 0\n";
        assert!(parse_stat_misc(no_btime, 1).is_err());
    }

    #[test]
    fn missing_any_required_field_is_an_error() {
        let no_ctxt = "btime 1700000000\nprocesses 2\nprocs_running 1\nprocs_blocked 0\n";
        assert!(parse_stat_misc(no_ctxt, 1).is_err());
    }

    #[test]
    fn btime_overflow_is_an_error() {
        // i64::MAX / 1_000_000 + 1 overflows microseconds.
        let overflow =
            "ctxt 1\nbtime 9223372036854776\nprocesses 2\nprocs_running 1\nprocs_blocked 0\n";
        assert!(parse_stat_misc(overflow, 1).is_err());
    }

    #[test]
    fn to_section_carries_every_floor_field_and_scope() {
        let section = parse_stat_misc(STAT_SAMPLE, 9_999)
            .expect("parse")
            .to_section(0);
        assert_eq!(section.ts.0, 9_999);
        assert_eq!(section.ctxt, 12_345);
        assert_eq!(section.btime.0, 1_700_000_000_000_000);
        assert_eq!(section.processes, 42);
        assert_eq!(section.procs_running, 3);
        assert_eq!(section.procs_blocked, 1);
        assert_eq!(section.scope, 0);
    }

    #[test]
    fn interrupt_totals_come_from_the_first_token_of_their_line() {
        let content = "ctxt 1\nbtime 2\nprocesses 3\nprocs_running 0\nprocs_blocked 0\n\
intr 900 1 2 3 4\nsoftirq 800 5 6 7\n";
        let row = parse_stat_misc(content, 1).expect("parse");
        assert_eq!(row.intr_total, Some(900));
        assert_eq!(row.softirq_total, Some(800));
    }

    #[test]
    fn a_kernel_without_interrupt_lines_leaves_the_totals_null() {
        let bare = "ctxt 1\nbtime 2\nprocesses 3\nprocs_running 0\nprocs_blocked 0\n";
        let row = parse_stat_misc(bare, 1).expect("parse");
        assert_eq!(row.intr_total, None);
        assert_eq!(row.softirq_total, None);
    }

    #[test]
    fn the_sample_totals_match_its_intr_and_softirq_lines() {
        let row = parse_stat_misc(STAT_SAMPLE, 1).expect("parse");
        assert_eq!(row.intr_total, Some(999));
        assert_eq!(row.softirq_total, Some(100));
    }

    #[test]
    fn uptime_is_exact_at_hundredth_second_resolution() {
        assert_eq!(
            parse_uptime("3600.25 28000.50\n"),
            Some((3_600_250_000, 28_000_500_000))
        );
        assert_eq!(parse_uptime("0.00 0.00\n"), Some((0, 0)));
        assert_eq!(parse_uptime("12 34\n"), Some((12_000_000, 34_000_000)));
    }

    #[test]
    fn uptime_rejects_a_short_or_garbled_file() {
        assert_eq!(parse_uptime(""), None);
        assert_eq!(parse_uptime("3600.25\n"), None);
        assert_eq!(parse_uptime("nope 1.0\n"), None);
        assert_eq!(parse_uptime("-1.0 2.0\n"), None);
    }
}
