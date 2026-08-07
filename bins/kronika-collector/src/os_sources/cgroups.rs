use super::{
    DueSet, Instant, Interner, OsLimits, OsSources, ProcFs, SourceKind, SysFs, cgroup, intern_str,
    log_cap_degraded, log_collection_finish, log_degraded, process_facts,
};

#[expect(
    clippy::too_many_arguments,
    reason = "the four cgroup sections share one tree walk and its ceilings"
)]
pub(super) fn collect_cgroup_sections(
    sys: &SysFs,
    interner: &mut Interner,
    scope: u8,
    ts: i64,
    fs: &ProcFs,
    due: &DueSet,
    limits: &OsLimits,
    os: &mut OsSources,
) {
    if !due.has(SourceKind::OsCgroup) {
        return;
    }

    let cpu_type_id = 1_201_001_u32;
    let memory_type_id = 1_202_001_u32;
    let io_type_id = 1_203_001_u32;
    let pids_type_id = 1_204_001_u32;
    let started = Instant::now();
    let clock_ticks = process_facts(fs).map_or_else(
        |err| {
            log_degraded(cpu_type_id, "cgroup", &err);
            0
        },
        |facts| facts.clock_ticks_per_sec,
    );
    let max_cgroups = limits.max_cgroups;
    let max_io_rows = limits.max_cgroup_io_rows;
    let max_depth = limits.cgroup_max_depth;
    let rows = cgroup::collect(sys, ts, clock_ticks, max_cgroups, max_io_rows, max_depth);
    if rows.dropped_cgroups > 0 {
        for type_id in [cpu_type_id, memory_type_id, io_type_id, pids_type_id] {
            log_cap_degraded(
                type_id,
                "cgroup",
                "cgroup_cap",
                rows.dropped_cgroups,
                max_cgroups,
            );
        }
    }
    if rows.dropped_io_rows > 0 {
        log_cap_degraded(
            io_type_id,
            "cgroup/io",
            "cgroup_io_cap",
            rows.dropped_io_rows,
            max_io_rows,
        );
    }

    for row in &rows.cpu {
        if let Some(cgroup_path) = intern_str(interner, cpu_type_id, "cgroup/cpu", &row.cgroup_path)
        {
            os.cgroup_cpu
                .push(cgroup::to_cpu_section(row, scope, cgroup_path));
        }
    }
    for row in &rows.memory {
        if let Some(cgroup_path) =
            intern_str(interner, memory_type_id, "cgroup/memory", &row.cgroup_path)
        {
            os.cgroup_memory
                .push(cgroup::to_memory_section(row, scope, cgroup_path));
        }
    }
    for row in &rows.io {
        if let Some(cgroup_path) = intern_str(interner, io_type_id, "cgroup/io", &row.cgroup_path) {
            os.cgroup_io
                .push(cgroup::to_io_section(row, scope, cgroup_path));
        }
    }
    for row in &rows.pids {
        if let Some(cgroup_path) =
            intern_str(interner, pids_type_id, "cgroup/pids", &row.cgroup_path)
        {
            os.cgroup_pids
                .push(cgroup::to_pids_section(row, scope, cgroup_path));
        }
    }
    log_collection_finish(
        cpu_type_id,
        "cgroup",
        os.cgroup_cpu.len(),
        started.elapsed(),
    );
    log_collection_finish(
        memory_type_id,
        "cgroup",
        os.cgroup_memory.len(),
        started.elapsed(),
    );
    log_collection_finish(io_type_id, "cgroup", os.cgroup_io.len(), started.elapsed());
    log_collection_finish(
        pids_type_id,
        "cgroup",
        os.cgroup_pids.len(),
        started.elapsed(),
    );
}
