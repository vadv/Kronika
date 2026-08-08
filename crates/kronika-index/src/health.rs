//! Health: one number per interval, from the pressure stall counters.

/// What one snapshot reported in `os_psi`: cumulative stall time per resource,
/// microseconds since the counters started.
///
/// The `some` line of each resource, never `full`: the question is whether
/// anyone lost time, not whether everything stopped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stall {
    /// `/proc/pressure/cpu`, `some`.
    pub cpu: i64,
    /// `/proc/pressure/memory`, `some`.
    pub memory: i64,
    /// `/proc/pressure/io`, `some`.
    pub io: i64,
}

/// Health over the interval between two snapshots, `0` to `100`.
///
/// The value is the share of the interval in which nothing was waiting for the
/// most contended resource. `None` where it cannot be computed: the interval is
/// not positive, or a counter went backwards.
///
/// The counters are already scaled by how much of the machine the waiting
/// covers, so nothing here divides by CPU count or by a cgroup quota.
#[must_use]
pub fn health(before: Stall, before_ts: i64, after: Stall, after_ts: i64) -> Option<u8> {
    let elapsed = after_ts.checked_sub(before_ts).filter(|span| *span > 0)?;
    let worst = [
        stalled(before.cpu, after.cpu)?,
        stalled(before.memory, after.memory)?,
        stalled(before.io, after.io)?,
    ]
    .into_iter()
    .max()?;
    Some(percent_left(worst, elapsed))
}

/// Stall microseconds between two readings of one counter, or `None` when it
/// went backwards.
fn stalled(before: i64, after: i64) -> Option<i64> {
    after.checked_sub(before).filter(|delta| *delta >= 0)
}

/// `100` minus the share `stalled` takes of `elapsed`, rounded to the nearest
/// whole percent.
///
/// A resource can stall for longer than the interval when several tasks wait at
/// once, so the share is capped at one rather than allowed to go negative.
fn percent_left(stalled: i64, elapsed: i64) -> u8 {
    let capped = stalled.min(elapsed);
    let half = elapsed / 2;
    // Integer arithmetic throughout: the inputs are microseconds, and a float
    // would only add a rounding story to explain.
    let percent = (capped.saturating_mul(100).saturating_add(half)) / elapsed;
    100_u8.saturating_sub(u8::try_from(percent).unwrap_or(100))
}

#[cfg(test)]
mod tests;
