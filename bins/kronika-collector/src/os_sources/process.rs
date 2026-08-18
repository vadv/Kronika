use super::{
    DueSet, Instant, Interner, OsCgroupMapping, OsSources, ProcFs, ProcessError, SourceKind, Ts,
    UserReferences, intern_str, log_collection_finish, log_count_degraded, log_degraded,
    process_facts, read_process_with_cgroup,
};

#[allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    reason = "process sections share procfs enumeration, segment user state, and degradation counters"
)]
pub(super) fn collect_process_sections(
    fs: &ProcFs,
    interner: &mut Interner,
    users: &mut UserReferences,
    scope: u8,
    ts: i64,
    due: &DueSet,
    collect_cgroups: bool,
    os: &mut OsSources,
) -> Vec<String> {
    let hot_due = due.has(SourceKind::OsProcesses);
    let status_due = due.has(SourceKind::OsProcessStatus);
    let mapping_due = due.has(SourceKind::OsCgroupMapping);
    let cgroup_due = collect_cgroups;
    if !hot_due && !status_due && !mapping_due && !cgroup_due {
        return Vec::new();
    }

    let hot_type_id = 1_100_001_u32;
    let status_type_id = 1_101_001_u32;
    let mapping_type_id = 1_200_001_u32;
    let started = Instant::now();
    let pids = match fs.pid_dirs() {
        Ok(pids) => pids,
        Err(err) => {
            for type_id in [hot_type_id, status_type_id, mapping_type_id] {
                if (type_id == hot_type_id && hot_due)
                    || (type_id == status_type_id && status_due)
                    || (type_id == mapping_type_id && mapping_due)
                {
                    log_degraded(type_id, "process", &err);
                }
            }
            return Vec::new();
        }
    };
    let facts = match process_facts(fs) {
        Ok(facts) => facts,
        Err(err) => {
            for type_id in [hot_type_id, status_type_id, mapping_type_id] {
                if (type_id == hot_type_id && hot_due)
                    || (type_id == status_type_id && status_due)
                    || (type_id == mapping_type_id && mapping_due)
                {
                    log_degraded(type_id, "process", &err);
                }
            }
            return if cgroup_due {
                pids.into_iter()
                    .filter_map(|pid| fs.read_raw(&format!("{pid}/cgroup")).ok())
                    .collect()
            } else {
                Vec::new()
            };
        }
    };
    let mut skipped = 0_usize;
    let mut io_nulls = 0_usize;
    let mut mapping_nulls = 0_usize;
    let mut cgroup_memberships = Vec::new();
    for pid in pids {
        let membership = if mapping_due || cgroup_due {
            fs.read_raw(&format!("{pid}/cgroup")).ok()
        } else {
            None
        };
        if cgroup_due && let Some(membership) = &membership {
            cgroup_memberships.push(membership.clone());
        }
        let read = match read_process_with_cgroup(fs, pid, facts, ts, membership) {
            Ok(read) => read,
            Err(ProcessError::Gone(_)) => continue,
            Err(_) => {
                skipped = skipped.saturating_add(1);
                continue;
            }
        };
        if hot_due {
            if read.hot.io.is_none() {
                io_nulls = io_nulls.saturating_add(1);
            }
            let Some(comm) = intern_str(interner, hot_type_id, "process", &read.hot.comm) else {
                skipped = skipped.saturating_add(1);
                continue;
            };
            let cmdline = read
                .hot
                .cmdline
                .as_deref()
                .and_then(|value| intern_str(interner, hot_type_id, "process", value));
            let row =
                kronika_source_os::proc::process::to_hot_section(&read.hot, scope, comm, cmdline);
            users.observe(scope, row.uid);
            users.observe(scope, row.euid);
            os.processes.push(row);
        }
        if status_due {
            os.process_status
                .push(kronika_source_os::proc::process::to_status_section(
                    &read.status,
                    scope,
                ));
        }
        if mapping_due {
            if let Some(mapping) = read.cgroup {
                if let Some(cgroup_path) = intern_str(
                    interner,
                    mapping_type_id,
                    "process/cgroup",
                    &mapping.cgroup_path,
                ) {
                    os.cgroup_mapping.push(OsCgroupMapping {
                        ts: Ts(mapping.ts),
                        pid: mapping.pid,
                        starttime: Ts(mapping.starttime),
                        cgroup_path,
                        scope,
                    });
                }
            } else {
                mapping_nulls = mapping_nulls.saturating_add(1);
            }
        }
    }

    if skipped > 0 {
        for type_id in [hot_type_id, status_type_id, mapping_type_id] {
            if (type_id == hot_type_id && hot_due)
                || (type_id == status_type_id && status_due)
                || (type_id == mapping_type_id && mapping_due)
            {
                log_count_degraded(type_id, "process", "process_skipped", skipped);
            }
        }
    }
    if hot_due && io_nulls > 0 {
        log_count_degraded(
            hot_type_id,
            "process/io",
            "process_io_unavailable",
            io_nulls,
        );
    }
    if mapping_due && mapping_nulls > 0 {
        log_count_degraded(
            mapping_type_id,
            "process/cgroup",
            "process_cgroup_unavailable",
            mapping_nulls,
        );
    }
    if hot_due {
        (os.users, os.pending_users) = users.prepare_rows(interner, scope, ts, std::iter::empty());
        log_collection_finish(hot_type_id, "procfs", os.processes.len(), started.elapsed());
    }
    if status_due {
        log_collection_finish(
            status_type_id,
            "procfs",
            os.process_status.len(),
            started.elapsed(),
        );
    }
    if mapping_due {
        log_collection_finish(
            mapping_type_id,
            "procfs",
            os.cgroup_mapping.len(),
            started.elapsed(),
        );
    }
    cgroup_memberships
}
