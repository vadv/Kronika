//! Type `1_201_002`: cgroup CPU counters and limits.

use crate::{Section, StrId, Ts};

/// CPU usage and throttling for one cgroup.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_201_002,
    name = "os_cgroup_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupCpu {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: i64,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: i64,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: i64,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: i64,
    /// Number of CPU throttling events.
    #[column(c, unit = count)]
    pub nr_throttled: i64,
    /// CPU quota per period, microseconds (`-1` means unlimited).
    #[column(g, unit = microseconds)]
    pub quota_usec: i64,
    /// CPU quota period.
    #[column(g, unit = microseconds)]
    pub period_usec: i64,
    /// CPUs in `cpuset.cpus.effective`; `None` where the file is absent. A pod
    /// pinned by cpuset alone has no quota, and without this its ceiling would
    /// read as every core of the node.
    #[column(g, unit = count)]
    pub cpuset_cpus: Option<i64>,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

/// Previous type `1_201_001`, retained so existing ZMS remains readable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Section)]
#[section(
    id = 1_201_001,
    name = "os_cgroup_cpu",
    semantics = snapshot_full,
    sort_key("cgroup_path", "ts"),
    identity("cgroup_path")
)]
pub struct OsCgroupCpuV1 {
    /// Collection timestamp, unix microseconds.
    #[column(t)]
    pub ts: Ts,
    /// Cgroup path as a string dictionary reference.
    #[column(l)]
    pub cgroup_path: StrId,
    /// Total CPU usage.
    #[column(c, unit = microseconds)]
    pub usage_usec: i64,
    /// User CPU usage.
    #[column(c, unit = microseconds)]
    pub user_usec: i64,
    /// System CPU usage.
    #[column(c, unit = microseconds)]
    pub system_usec: i64,
    /// CPU throttled time.
    #[column(c, unit = microseconds)]
    pub throttled_usec: i64,
    /// Number of CPU throttling events.
    #[column(c, unit = count)]
    pub nr_throttled: i64,
    /// CPU quota per period, microseconds (`-1` means unlimited).
    #[column(g, unit = microseconds)]
    pub quota_usec: i64,
    /// CPU quota period.
    #[column(g, unit = microseconds)]
    pub period_usec: i64,
    /// Source scope. See `kronika_source_os::OsScope`.
    #[column(l)]
    pub scope: u8,
}

#[cfg(test)]
mod tests {
    use super::OsCgroupCpu;
    use crate::{Section, StrId, Ts, contract::lint};

    #[test]
    fn contract_passes_the_linter() {
        assert_eq!(lint(&[OsCgroupCpu::CONTRACT]), Ok(()));
    }

    #[test]
    fn contract_shape() {
        let c = OsCgroupCpu::CONTRACT;
        assert_eq!(c.type_id.get(), 1_201_002);
        assert_eq!(c.sort_key, ["cgroup_path", "ts"]);
        assert_eq!(c.identity, ["cgroup_path"]);
    }

    #[test]
    fn roundtrip() {
        crate::assert_roundtrips(&[OsCgroupCpu {
            ts: Ts(1),
            cgroup_path: StrId(10),
            usage_usec: 100,
            user_usec: 60,
            system_usec: 40,
            throttled_usec: 7,
            nr_throttled: 2,
            quota_usec: -1,
            period_usec: 100_000,
            cpuset_cpus: Some(4),
            scope: 1,
        }]);
    }
}
