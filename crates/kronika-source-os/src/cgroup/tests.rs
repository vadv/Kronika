use super::*;

fn fixture_roots() -> (tempfile::TempDir, ProcFs, SysFs) {
    let dir = tempfile::tempdir().expect("tempdir");
    let proc_root = dir.path().join("proc");
    let sys_root = dir.path().join("sys");
    std::fs::create_dir_all(proc_root.join("self")).expect("mkdir proc self");
    std::fs::create_dir_all(sys_root.join("fs/cgroup")).expect("mkdir cgroup root");
    (dir, ProcFs::new(proc_root), SysFs::new(sys_root))
}

#[test]
fn context_v2_uses_the_unified_self_path_and_effective_cpuset() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "0::/kubepods/pod-a/container-a\n",
    )
    .expect("write self cgroup");
    let cgroup = dir.path().join("sys/fs/cgroup/kubepods/pod-a/container-a");
    std::fs::create_dir_all(&cgroup).expect("mkdir unified cgroup");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io cpuset\n",
    )
    .expect("write controllers");
    std::fs::write(cgroup.join("cpuset.cpus.effective"), "0-2,5,8-9\n")
        .expect("write effective cpuset");
    std::fs::write(
        cgroup.join("cpu.stat"),
        "usage_usec 10\nuser_usec 6\nsystem_usec 4\n",
    )
    .expect("write cpu stat");
    std::fs::write(cgroup.join("memory.current"), "4096\n").expect("write memory current");
    std::fs::write(
        cgroup.join("memory.stat"),
        "anon 100\nfile 200\nkernel 50\nslab 20\n",
    )
    .expect("write memory stat");
    std::fs::write(
        cgroup.join("io.stat"),
        "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n",
    )
    .expect("write io stat");

    let context = collect_context(&procfs, &sys, 99).expect("collect context");

    assert_eq!(context.ts, 99);
    assert_eq!(context.cgroup_version, 2);
    assert_eq!(
        context.cpu_path.as_deref(),
        Some("/kubepods/pod-a/container-a")
    );
    assert_eq!(context.memory_path, context.cpu_path);
    assert_eq!(context.io_path, context.cpu_path);
    assert_eq!(context.cpuset_cpus, Some(6));
}

#[test]
fn context_v1_keeps_controller_specific_paths() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "2:cpu,cpuacct:/service/cpu\n3:memory:/service/memory\n\
         4:blkio:/service/io\n5:cpuset:/service/set\n",
    )
    .expect("write self cgroup");
    let cpuset = dir.path().join("sys/fs/cgroup/cpuset/service/set");
    std::fs::create_dir_all(&cpuset).expect("mkdir cpuset cgroup");
    std::fs::write(cpuset.join("cpuset.effective_cpus"), "1,3-4\n")
        .expect("write effective cpuset");
    let cpu = dir.path().join("sys/fs/cgroup/cpu,cpuacct/service/cpu");
    let memory = dir.path().join("sys/fs/cgroup/memory/service/memory");
    let io = dir.path().join("sys/fs/cgroup/blkio/service/io");
    for path in [&cpu, &memory, &io] {
        std::fs::create_dir_all(path).expect("mkdir controller cgroup");
    }
    std::fs::write(cpu.join("cpuacct.usage"), "1000\n").expect("write cpu usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 6\nsystem 4\n").expect("write cpu account stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n").expect("write memory current");
    std::fs::write(
        memory.join("memory.stat"),
        "total_rss 100\ntotal_cache 200\ntotal_slab 20\ntotal_kernel_stack 5\n",
    )
    .expect("write memory stat");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write io bytes");
    std::fs::write(
        io.join("blkio.throttle.io_serviced"),
        "8:0 Read 3\n8:0 Write 4\n",
    )
    .expect("write io operations");

    let context = collect_context(&procfs, &sys, 123).expect("collect context");

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path.as_deref(), Some("/service/cpu"));
    assert_eq!(context.memory_path.as_deref(), Some("/service/memory"));
    assert_eq!(context.io_path.as_deref(), Some("/service/io"));
    assert_eq!(context.cpuset_cpus, Some(3));
}

#[test]
fn hybrid_membership_uses_the_v1_tree_selected_by_collection() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "0::/unified/workload\n2:cpu,cpuacct:/service/cpu\n\
         3:memory:/service/memory\n4:blkio:/service/io\n5:cpuset:/service/set\n",
    )
    .expect("write hybrid self cgroup");
    let unified = dir.path().join("sys/fs/cgroup/unified");
    std::fs::create_dir_all(&unified).expect("mkdir nested unified mount");
    std::fs::write(unified.join("cgroup.controllers"), "cpu memory io cpuset\n")
        .expect("write nested unified controllers");

    let cpu = dir.path().join("sys/fs/cgroup/cpu,cpuacct/service/cpu");
    let memory = dir.path().join("sys/fs/cgroup/memory/service/memory");
    let io = dir.path().join("sys/fs/cgroup/blkio/service/io");
    let cpuset = dir.path().join("sys/fs/cgroup/cpuset/service/set");
    for path in [&cpu, &memory, &io, &cpuset] {
        std::fs::create_dir_all(path).expect("mkdir hybrid controller cgroup");
    }
    std::fs::write(cpu.join("cpuacct.usage"), "1000\n").expect("write cpu usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 6\nsystem 4\n").expect("write cpu account stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n").expect("write memory current");
    std::fs::write(
        memory.join("memory.stat"),
        "total_rss 100\ntotal_cache 200\ntotal_slab 20\ntotal_kernel_stack 5\n",
    )
    .expect("write memory stat");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write io bytes");
    std::fs::write(
        io.join("blkio.throttle.io_serviced"),
        "8:0 Read 3\n8:0 Write 4\n",
    )
    .expect("write io operations");
    std::fs::write(cpuset.join("cpuset.effective_cpus"), "0-1\n").expect("write effective cpuset");

    let context = collect_context(&procfs, &sys, 123).expect("collect hybrid context");
    let rows = collect(&sys, 123, 100);

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path.as_deref(), Some("/service/cpu"));
    assert_eq!(context.memory_path.as_deref(), Some("/service/memory"));
    assert_eq!(context.io_path.as_deref(), Some("/service/io"));
    assert_eq!(context.cpuset_cpus, Some(2));
    assert!(rows.cpu.iter().any(|row| row.cgroup_path == "/service/cpu"));
    assert!(
        rows.memory
            .iter()
            .any(|row| row.cgroup_path == "/service/memory")
    );
    assert!(rows.io.iter().any(|row| row.cgroup_path == "/service/io"));
}

#[test]
fn context_does_not_combine_different_v1_cpu_controller_paths() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "2:cpu:/service/quota\n3:cpuacct:/service/usage\n",
    )
    .expect("write self cgroup");

    let context = collect_context(&procfs, &sys, 1).expect("collect context");

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.cpuset_cpus, None);
}

#[test]
fn context_keeps_unavailable_or_malformed_cpuset_null() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(dir.path().join("proc/self/cgroup"), "0::/workload\n")
        .expect("write self cgroup");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu cpuset\n",
    )
    .expect("write controllers");
    let workload = dir.path().join("sys/fs/cgroup/workload");
    std::fs::create_dir_all(&workload).expect("mkdir workload");
    std::fs::write(workload.join("cpuset.cpus.effective"), "0-2,2\n")
        .expect("write malformed effective cpuset");

    let context = collect_context(&procfs, &sys, 1).expect("collect context");

    assert_eq!(context.cgroup_version, 2);
    assert_eq!(context.cpuset_cpus, None);
}

#[test]
fn missing_self_membership_is_reported_even_on_a_v2_mount() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write v2 controllers");

    assert_eq!(
        collect_context(&procfs, &sys, 7)
            .expect_err("missing membership must be reported")
            .kind(),
        io::ErrorKind::NotFound
    );
}

#[test]
fn partial_v1_controller_files_do_not_select_zero_filled_rows() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(
        dir.path().join("proc/self/cgroup"),
        "2:cpu:/partial\n3:memory:/partial\n4:blkio:/partial\n",
    )
    .expect("write self cgroup");
    let cpu = dir.path().join("sys/fs/cgroup/cpu/partial");
    let memory = dir.path().join("sys/fs/cgroup/memory/partial");
    let io = dir.path().join("sys/fs/cgroup/blkio/partial");
    for path in [&cpu, &memory, &io] {
        std::fs::create_dir_all(path).expect("mkdir partial controller cgroup");
    }
    std::fs::write(cpu.join("cpu.stat"), "nr_throttled 1\n").expect("write partial cpu stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "4096\n")
        .expect("write partial memory current");
    std::fs::write(
        io.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 1\n8:0 Write 2\n",
    )
    .expect("write partial io bytes");

    let context = collect_context(&procfs, &sys, 7).expect("collect partial context");

    assert_eq!(context.cgroup_version, 1);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.memory_path, None);
    assert_eq!(context.io_path, None);
}

#[test]
fn partial_v2_controller_files_do_not_select_zero_filled_rows() {
    let (dir, procfs, sys) = fixture_roots();
    std::fs::write(dir.path().join("proc/self/cgroup"), "0::/partial\n")
        .expect("write self cgroup");
    std::fs::write(
        dir.path().join("sys/fs/cgroup/cgroup.controllers"),
        "cpu memory io\n",
    )
    .expect("write controllers");
    let cgroup = dir.path().join("sys/fs/cgroup/partial");
    std::fs::create_dir_all(&cgroup).expect("mkdir partial cgroup");
    std::fs::write(cgroup.join("cpu.stat"), "usage_usec 10\n").expect("write partial cpu stat");
    std::fs::write(cgroup.join("memory.current"), "4096\n").expect("write partial memory current");
    std::fs::write(cgroup.join("io.stat"), "8:0 rbytes=1 wbytes=2\n")
        .expect("write partial io stat");

    let context = collect_context(&procfs, &sys, 7).expect("collect partial context");

    assert_eq!(context.cgroup_version, 2);
    assert_eq!(context.cpu_path, None);
    assert_eq!(context.memory_path, None);
    assert_eq!(context.io_path, None);
}

#[test]
fn collect_v2_reads_every_controller_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("fs/cgroup");
    let workload = root.join("workload");
    std::fs::create_dir_all(&workload).expect("mkdir cgroup");
    std::fs::write(root.join("cgroup.controllers"), "cpu memory io pids\n")
        .expect("write controllers");
    std::fs::write(
        workload.join("cpu.stat"),
        "usage_usec 100\nuser_usec 60\nsystem_usec 40\nnr_throttled 2\nthrottled_usec 500\n",
    )
    .expect("write cpu.stat");
    std::fs::write(workload.join("cpu.max"), "200000 100000\n").expect("write cpu.max");
    std::fs::write(workload.join("memory.current"), "4096\n").expect("write memory.current");
    std::fs::write(workload.join("memory.max"), "max\n").expect("write memory.max");
    std::fs::write(
        workload.join("memory.stat"),
        "anon 100\nfile 200\nkernel 50\nslab 20\n",
    )
    .expect("write memory.stat");
    std::fs::write(
        workload.join("memory.events"),
        "low 1\nhigh 2\nmax 3\noom 4\noom_kill 5\n",
    )
    .expect("write memory.events");
    std::fs::write(workload.join("pids.current"), "7\n").expect("write pids.current");
    std::fs::write(workload.join("pids.max"), "max\n").expect("write pids.max");
    std::fs::write(
        workload.join("io.stat"),
        "8:0 rbytes=1 wbytes=2 rios=3 wios=4\n\
             259:0 rbytes=5 wbytes=6 rios=7 wios=8\n",
    )
    .expect("write io.stat");

    let sys = SysFs::new(dir.path().to_path_buf());
    let rows = collect(&sys, 99, 100);

    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.memory.len(), 1);
    assert_eq!(rows.io.len(), 2);
    assert_eq!(rows.pids.len(), 1);

    let cpu = &rows.cpu[0];
    assert_eq!(cpu.cgroup_path, "/workload");
    assert_eq!(cpu.ts, 99);
    assert_eq!(cpu.usage_usec, 100);
    assert_eq!(cpu.user_usec, 60);
    assert_eq!(cpu.system_usec, 40);
    assert_eq!(cpu.nr_throttled, 2);
    assert_eq!(cpu.throttled_usec, 500);
    assert_eq!(cpu.quota_usec, 200_000);
    assert_eq!(cpu.period_usec, 100_000);

    let memory = &rows.memory[0];
    assert_eq!(memory.current, 4096);
    assert_eq!(memory.max, None);
    assert_eq!(memory.anon, 100);
    assert_eq!(memory.file, 200);
    assert_eq!(memory.kernel, 50);
    assert_eq!(memory.slab, 20);
    assert_eq!(memory.low_events, 1);
    assert_eq!(memory.high_events, 2);
    assert_eq!(memory.max_events, 3);
    assert_eq!(memory.oom_events, 4);
    assert_eq!(memory.oom_kill, 5);

    assert_eq!(rows.pids[0].current, 7);
    assert_eq!(rows.pids[0].max, None);
    assert_eq!((rows.io[0].major, rows.io[0].minor), (8, 0));
    assert_eq!(rows.io[0].rbytes, 1);
    assert_eq!(rows.io[0].wios, 4);
}

#[test]
fn collect_v1_reads_every_controller_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join("fs/cgroup");
    let cpu = root.join("cpu,cpuacct/workload");
    let memory = root.join("memory/workload");
    let pids = root.join("pids/workload");
    let blkio = root.join("blkio/workload");
    for path in [&cpu, &memory, &pids, &blkio] {
        std::fs::create_dir_all(path).expect("mkdir cgroup controller");
    }

    std::fs::write(cpu.join("cpuacct.usage"), "200000000\n").expect("write cpuacct.usage");
    std::fs::write(cpu.join("cpuacct.stat"), "user 30\nsystem 20\n").expect("write cpuacct.stat");
    std::fs::write(cpu.join("cpu.cfs_quota_us"), "50000\n").expect("write cpu.cfs_quota_us");
    std::fs::write(cpu.join("cpu.cfs_period_us"), "100000\n").expect("write cpu.cfs_period_us");
    std::fs::write(
        cpu.join("cpu.stat"),
        "nr_periods 9\nnr_throttled 3\nthrottled_time 700000000\n",
    )
    .expect("write cpu.stat");
    std::fs::write(memory.join("memory.usage_in_bytes"), "8192\n")
        .expect("write memory.usage_in_bytes");
    std::fs::write(memory.join("memory.limit_in_bytes"), "16384\n")
        .expect("write memory.limit_in_bytes");
    std::fs::write(
        memory.join("memory.stat"),
        "total_rss 1000\ntotal_cache 2000\ntotal_slab 300\ntotal_kernel_stack 40\n",
    )
    .expect("write memory.stat");
    std::fs::write(memory.join("memory.failcnt"), "6\n").expect("write memory.failcnt");
    std::fs::write(pids.join("pids.current"), "9\n").expect("write pids.current");
    std::fs::write(pids.join("pids.max"), "128\n").expect("write pids.max");
    std::fs::write(
        blkio.join("blkio.throttle.io_service_bytes"),
        "8:0 Read 10\n8:0 Write 20\n259:0 Read 30\n259:0 Write 40\n",
    )
    .expect("write blkio bytes");
    std::fs::write(
        blkio.join("blkio.throttle.io_serviced"),
        "8:0 Read 1\n8:0 Write 2\n259:0 Read 3\n259:0 Write 4\n",
    )
    .expect("write blkio ops");

    let sys = SysFs::new(dir.path().to_path_buf());
    let rows = collect(&sys, 123, 100);

    assert_eq!(rows.cpu.len(), 1);
    assert_eq!(rows.memory.len(), 1);
    assert_eq!(rows.pids.len(), 1);
    assert_eq!(rows.io.len(), 2);

    let cpu = &rows.cpu[0];
    assert_eq!(cpu.cgroup_path, "/workload");
    assert_eq!(cpu.ts, 123);
    assert_eq!(cpu.usage_usec, 200_000);
    assert_eq!(cpu.user_usec, 300_000);
    assert_eq!(cpu.system_usec, 200_000);
    assert_eq!(cpu.nr_throttled, 3);
    assert_eq!(cpu.throttled_usec, 700_000);
    assert_eq!(cpu.quota_usec, 50_000);
    assert_eq!(cpu.period_usec, 100_000);

    let memory = &rows.memory[0];
    assert_eq!(memory.current, 8192);
    assert_eq!(memory.max, Some(16_384));
    assert_eq!(memory.anon, 1000);
    assert_eq!(memory.file, 2000);
    assert_eq!(memory.slab, 300);
    assert_eq!(memory.kernel, 340);
    assert_eq!(memory.max_events, 6);

    assert_eq!(rows.pids[0].current, 9);
    assert_eq!(rows.pids[0].max, Some(128));
    assert_eq!((rows.io[0].major, rows.io[0].minor), (8, 0));
    assert_eq!(rows.io[0].rbytes, 10);
    assert_eq!(rows.io[0].wbytes, 20);
    assert_eq!(rows.io[0].rios, 1);
    assert_eq!(rows.io[0].wios, 2);
}

#[test]
fn section_conversions_preserve_metric_fields() {
    use kronika_registry::{StrId, Ts};

    let cgroup_path = StrId(55);
    let cpu = CgroupCpuRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        usage_usec: 100,
        user_usec: 60,
        system_usec: 40,
        throttled_usec: 5,
        nr_throttled: 2,
        quota_usec: -1,
        period_usec: 100_000,
    };
    let memory = CgroupMemoryRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        current: 4096,
        max: None,
        anon: 100,
        file: 200,
        kernel: 50,
        slab: 20,
        low_events: 1,
        high_events: 2,
        max_events: 3,
        oom_events: 4,
        oom_kill: 5,
    };
    let io = CgroupIoRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        major: 8,
        minor: 0,
        rbytes: 1,
        wbytes: 2,
        rios: 3,
        wios: 4,
    };
    let pids = CgroupPidsRow {
        ts: 7,
        cgroup_path: "/workload".to_owned(),
        current: 9,
        max: Some(128),
    };
    let context = CgroupContextRow {
        ts: 7,
        cgroup_version: 2,
        cpu_path: Some("/workload".to_owned()),
        memory_path: Some("/workload".to_owned()),
        io_path: Some("/workload".to_owned()),
        cpuset_cpus: Some(4),
    };

    let context_section = to_context_section(
        &context,
        3,
        Some(cgroup_path),
        Some(cgroup_path),
        Some(cgroup_path),
    );
    assert_eq!(context_section.ts, Ts(7));
    assert_eq!(context_section.cgroup_version, 2);
    assert_eq!(context_section.cpu_path, Some(cgroup_path));
    assert_eq!(context_section.cpuset_cpus, Some(4));
    assert_eq!(context_section.scope, 3);

    let cpu_section = to_cpu_section(&cpu, 2, cgroup_path);
    assert_eq!(cpu_section.ts, Ts(7));
    assert_eq!(cpu_section.cgroup_path, cgroup_path);
    assert_eq!(cpu_section.scope, 2);
    assert_eq!(cpu_section.usage_usec, 100);
    assert_eq!(cpu_section.nr_throttled, 2);

    let memory_section = to_memory_section(&memory, 2, cgroup_path);
    assert_eq!(memory_section.ts, Ts(7));
    assert_eq!(memory_section.max, None);
    assert_eq!(memory_section.oom_kill, 5);

    let io_section = to_io_section(&io, 2, cgroup_path);
    assert_eq!((io_section.major, io_section.minor), (8, 0));
    assert_eq!(io_section.rbytes, 1);
    assert_eq!(io_section.wios, 4);

    let pids_section = to_pids_section(&pids, 2, cgroup_path);
    assert_eq!(pids_section.current, 9);
    assert_eq!(pids_section.max, Some(128));
}
