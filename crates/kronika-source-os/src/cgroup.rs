//! Parse and collect cgroup v2 and selected cgroup v1 metrics.

use std::collections::{BTreeSet, VecDeque};

use crate::SysFs;

mod model;
mod parse;
mod sections;

pub use model::{CgroupCollection, CgroupCpuRow, CgroupIoRow, CgroupMemoryRow, CgroupPidsRow};
pub use parse::{parse_blkio_service_stats, parse_cpu_max, parse_cpu_stat, parse_io_stat};

use crate::proc::pressure::{PsiRow, parse_pressure};
pub use sections::{to_cpu_section, to_io_section, to_memory_section, to_pids_section};

use parse::{
    parse_cpuacct_stat, parse_i64, parse_memory_events, parse_memory_stat_v1, parse_memory_stat_v2,
    parse_optional_max, parse_v1_cpu_stat, parse_v1_memory_limit,
};

const CGROUP_ROOT: &str = "fs/cgroup";
const DEFAULT_CPU_PERIOD_USEC: i64 = 100_000;

const CPU_V1_DIRS: &[&str] = &["cpu,cpuacct", "cpuacct,cpu", "cpu", "cpuacct", ""];
const MEMORY_V1_DIRS: &[&str] = &["memory", ""];
const PIDS_V1_DIRS: &[&str] = &["pids", ""];
const CPUSET_V1_DIRS: &[&str] = &["cpuset", ""];
const BLKIO_V1_DIRS: &[&str] = &["blkio", ""];

/// Collect cgroup v2 or v1 rows from `KRONIKA_SYS_ROOT/fs/cgroup`.
#[must_use]
pub fn collect(sys: &SysFs, ts: i64, clock_ticks_per_sec: i64) -> CgroupCollection {
    if is_v2(sys) {
        collect_v2(sys, ts)
    } else {
        collect_v1(sys, ts, clock_ticks_per_sec)
    }
}

fn is_v2(sys: &SysFs) -> bool {
    sys.read(&rel("/", "cgroup.controllers")).is_ok()
        || sys.read(&rel("/", "cpu.stat")).is_ok()
        || sys.read(&rel("/", "memory.current")).is_ok()
        || sys.read(&rel("/", "io.stat")).is_ok()
}

fn collect_v2(sys: &SysFs, ts: i64) -> CgroupCollection {
    let paths = discover_v2_paths(sys);
    let mut out = CgroupCollection::default();
    for path in paths {
        if let Some(cpu) = read_cpu_v2(sys, ts, &path) {
            out.cpu.push(cpu);
        }
        if let Some(memory) = read_memory_v2(sys, ts, &path) {
            out.memory.push(memory);
        }
        if let Some(pids) = read_pids_v2(sys, ts, &path) {
            out.pids.push(pids);
        }
        if let Ok(content) = sys.read(&rel(&path, "io.stat")) {
            out.io.extend(parse_io_stat(&content, ts, &path));
        }
        // "/" is the current cgroup namespace; host pressure comes from procfs.
        if path == "/" {
            out.psi = read_pressure_v2(sys, ts, &path);
        }
    }
    out
}

fn read_pressure_v2(sys: &SysFs, ts: i64, path: &str) -> Vec<PsiRow> {
    let cpu = sys.read(&rel(path, "cpu.pressure")).ok();
    let memory = sys.read(&rel(path, "memory.pressure")).ok();
    let io = sys.read(&rel(path, "io.pressure")).ok();
    if cpu.is_none() && memory.is_none() && io.is_none() {
        return Vec::new();
    }
    parse_pressure(cpu.as_deref(), memory.as_deref(), io.as_deref(), ts).unwrap_or_default()
}

fn collect_v1(sys: &SysFs, ts: i64, clock_ticks_per_sec: i64) -> CgroupCollection {
    let paths = discover_v1_paths(sys);
    let mut out = CgroupCollection::default();
    for path in paths {
        if let Some(cpu) = read_cpu_v1(sys, ts, &path, clock_ticks_per_sec) {
            out.cpu.push(cpu);
        }
        if let Some(memory) = read_memory_v1(sys, ts, &path) {
            out.memory.push(memory);
        }
        if let Some(pids) = read_pids_v1(sys, ts, &path) {
            out.pids.push(pids);
        }
        let bytes = read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.throttle.io_service_bytes")
            .or_else(|| read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.io_service_bytes"));
        let ops = read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.throttle.io_serviced")
            .or_else(|| read_first_v1(sys, BLKIO_V1_DIRS, &path, "blkio.io_serviced"));
        if bytes.is_some() || ops.is_some() {
            out.io.extend(parse_blkio_service_stats(
                bytes.as_deref().unwrap_or_default(),
                ops.as_deref().unwrap_or_default(),
                ts,
                &path,
            ));
        }
    }
    out
}

fn read_cpu_v2(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupCpuRow> {
    let stat = sys.read(&rel(path, "cpu.stat")).ok()?;
    let mut row = parse_cpu_stat(&stat, ts, path);
    if let Ok(max) = sys.read(&rel(path, "cpu.max")) {
        let (quota, period) = parse_cpu_max(&max);
        row.quota_usec = quota;
        row.period_usec = period;
    }
    row.cpuset_cpus = sys
        .read(&rel(path, "cpuset.cpus.effective"))
        .ok()
        .and_then(|content| count_cpus(&content));
    Some(row)
}

/// Counts cpuset ranges; an empty set is inherited and returns `None`.
fn count_cpus(content: &str) -> Option<i64> {
    let mut total = 0_i64;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }
    for part in trimmed.split(',') {
        let (first, last) = part.split_once('-').unwrap_or((part, part));
        let first: i64 = first.trim().parse().ok()?;
        let last: i64 = last.trim().parse().ok()?;
        if last < first {
            return None;
        }
        total += last - first + 1;
    }
    Some(total)
}

fn read_cpu_v1(sys: &SysFs, ts: i64, path: &str, clock_ticks_per_sec: i64) -> Option<CgroupCpuRow> {
    let mut row = CgroupCpuRow {
        ts,
        cgroup_path: path.to_owned(),
        usage_usec: 0,
        user_usec: 0,
        system_usec: 0,
        throttled_usec: 0,
        nr_throttled: 0,
        quota_usec: -1,
        period_usec: DEFAULT_CPU_PERIOD_USEC,
        cpuset_cpus: None,
    };
    let usage_found =
        read_first_v1(sys, CPU_V1_DIRS, path, "cpuacct.usage").is_some_and(|content| {
            row.usage_usec = parse_i64(&content).unwrap_or(0) / 1000;
            true
        });
    let acct_found = read_first_v1(sys, CPU_V1_DIRS, path, "cpuacct.stat").is_some_and(|content| {
        parse_cpuacct_stat(&content, clock_ticks_per_sec, &mut row);
        true
    });
    if let Some(content) = read_first_v1(sys, CPU_V1_DIRS, path, "cpu.cfs_quota_us") {
        row.quota_usec = parse_i64(&content).unwrap_or(-1);
    }
    if let Some(content) = read_first_v1(sys, CPU_V1_DIRS, path, "cpu.cfs_period_us") {
        row.period_usec = parse_i64(&content).unwrap_or(DEFAULT_CPU_PERIOD_USEC);
    }
    row.cpuset_cpus = read_first_v1(sys, CPUSET_V1_DIRS, path, "cpuset.effective_cpus")
        .or_else(|| read_first_v1(sys, CPUSET_V1_DIRS, path, "cpuset.cpus"))
        .and_then(|content| count_cpus(&content));
    let throttle_found = read_first_v1(sys, CPU_V1_DIRS, path, "cpu.stat").is_some_and(|content| {
        parse_v1_cpu_stat(&content, &mut row);
        true
    });
    (usage_found || acct_found || throttle_found).then_some(row)
}

fn read_memory_v2(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupMemoryRow> {
    let current = parse_i64(&sys.read(&rel(path, "memory.current")).ok()?)?;
    let mut row = CgroupMemoryRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max: sys
            .read(&rel(path, "memory.max"))
            .ok()
            .and_then(|content| parse_optional_max(&content)),
        anon: 0,
        file: 0,
        kernel: 0,
        slab: 0,
        shmem: 0,
        low_events: 0,
        high_events: 0,
        max_events: 0,
        oom_events: 0,
        oom_kill: 0,
    };
    if let Ok(content) = sys.read(&rel(path, "memory.stat")) {
        parse_memory_stat_v2(&content, &mut row);
    }
    if let Ok(content) = sys.read(&rel(path, "memory.events")) {
        parse_memory_events(&content, &mut row);
    }
    Some(row)
}

fn read_memory_v1(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupMemoryRow> {
    let current = parse_i64(&read_first_v1(
        sys,
        MEMORY_V1_DIRS,
        path,
        "memory.usage_in_bytes",
    )?)?;
    let mut row = CgroupMemoryRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max: read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.limit_in_bytes")
            .and_then(|content| parse_v1_memory_limit(&content)),
        anon: 0,
        file: 0,
        kernel: 0,
        slab: 0,
        shmem: 0,
        low_events: 0,
        high_events: 0,
        max_events: 0,
        oom_events: 0,
        oom_kill: 0,
    };
    if let Some(content) = read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.stat") {
        parse_memory_stat_v1(&content, &mut row);
    }
    if let Some(content) = read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.failcnt") {
        row.max_events = parse_i64(&content).unwrap_or(0);
    }
    Some(row)
}

/// Read one already validated cgroup memory path without scanning cgroupfs.
#[must_use]
pub fn read_memory_path(
    sys: &SysFs,
    ts: i64,
    path: &str,
    unified_v2: bool,
) -> Option<(CgroupMemoryRow, bool)> {
    if unified_v2 {
        let limit = sys.read(&rel(path, "memory.max")).ok()?;
        if limit != "max" && parse_i64(&limit).is_none() {
            return None;
        }
        Some((read_memory_v2(sys, ts, path)?, limit == "max"))
    } else {
        let limit = read_first_v1(sys, MEMORY_V1_DIRS, path, "memory.limit_in_bytes")?;
        parse_i64(&limit)?;
        let unlimited = parse_v1_memory_limit(&limit).is_none();
        Some((read_memory_v1(sys, ts, path)?, unlimited))
    }
}

fn read_pids_v2(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupPidsRow> {
    let current = parse_i64(&sys.read(&rel(path, "pids.current")).ok()?).unwrap_or(0);
    Some(CgroupPidsRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max: sys
            .read(&rel(path, "pids.max"))
            .ok()
            .and_then(|content| parse_optional_max(&content)),
    })
}

fn read_pids_v1(sys: &SysFs, ts: i64, path: &str) -> Option<CgroupPidsRow> {
    let current = parse_i64(&read_first_v1(sys, PIDS_V1_DIRS, path, "pids.current")?).unwrap_or(0);
    Some(CgroupPidsRow {
        ts,
        cgroup_path: path.to_owned(),
        current,
        max: read_first_v1(sys, PIDS_V1_DIRS, path, "pids.max")
            .and_then(|content| parse_optional_max(&content)),
    })
}

fn discover_v2_paths(sys: &SysFs) -> Vec<String> {
    discover_tree(sys, CGROUP_ROOT, "/")
}

fn discover_v1_paths(sys: &SysFs) -> Vec<String> {
    let mut paths = BTreeSet::new();
    for controller in [
        "cpu,cpuacct",
        "cpuacct,cpu",
        "cpu",
        "cpuacct",
        "memory",
        "pids",
        "blkio",
    ] {
        // One cgroup appears under every controller mounted for it, and must
        // still produce one row.
        paths.extend(discover_tree(
            sys,
            &format!("{CGROUP_ROOT}/{controller}"),
            "/",
        ));
    }
    if paths.is_empty()
        && ["cpuacct.usage", "memory.usage_in_bytes", "pids.current"]
            .iter()
            .any(|file| sys.read(&format!("{CGROUP_ROOT}/{file}")).is_ok())
    {
        paths.insert("/".to_owned());
    }
    paths.into_iter().collect()
}

fn discover_tree(sys: &SysFs, base_rel: &str, root_path: &str) -> Vec<String> {
    let Ok(root_children) = sys.read_dir(base_rel) else {
        return Vec::new();
    };

    let mut out = vec![normalize_path(root_path, "")];
    let mut queue: VecDeque<String> = root_children
        .into_iter()
        .filter(|entry| entry.is_dir)
        .map(|entry| entry.name)
        .collect();
    while let Some(relative) = queue.pop_front() {
        out.push(normalize_path(root_path, &relative));
        let rel = format!("{base_rel}/{relative}");
        let Ok(children) = sys.read_dir(&rel) else {
            continue;
        };
        for child in children.into_iter().filter(|entry| entry.is_dir) {
            queue.push_back(format!("{relative}/{}", child.name));
        }
    }
    out
}

fn normalize_path(root_path: &str, relative: &str) -> String {
    if relative.is_empty() {
        root_path.to_owned()
    } else if root_path == "/" {
        format!("/{relative}")
    } else {
        format!("{root_path}/{relative}")
    }
}

fn rel(path: &str, file: &str) -> String {
    let path = path.trim_matches('/');
    if path.is_empty() {
        format!("{CGROUP_ROOT}/{file}")
    } else {
        format!("{CGROUP_ROOT}/{path}/{file}")
    }
}

fn read_first_v1(sys: &SysFs, dirs: &[&str], path: &str, file: &str) -> Option<String> {
    let relative = path.trim_matches('/');
    for dir in dirs {
        let candidate = match (dir.is_empty(), relative.is_empty()) {
            (true, true) => format!("{CGROUP_ROOT}/{file}"),
            (true, false) => format!("{CGROUP_ROOT}/{relative}/{file}"),
            (false, true) => format!("{CGROUP_ROOT}/{dir}/{file}"),
            (false, false) => format!("{CGROUP_ROOT}/{dir}/{relative}/{file}"),
        };
        if let Ok(content) = sys.read(&candidate) {
            return Some(content);
        }
    }
    None
}

#[cfg(test)]
mod tests;
