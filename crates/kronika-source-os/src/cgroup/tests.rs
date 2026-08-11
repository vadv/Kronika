use super::*;

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
        cpuset_cpus: Some(4),
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
        shmem: 8,
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

#[test]
fn pressure_of_our_own_group_is_collected_and_other_groups_are_not() {
    let root = tempfile::tempdir().expect("tempdir");
    let cgroup = root.path().join("fs/cgroup");
    std::fs::create_dir_all(cgroup.join("workload")).expect("tree");
    std::fs::write(cgroup.join("cgroup.controllers"), "cpu memory io\n").expect("controllers");
    std::fs::write(
        cgroup.join("cpu.pressure"),
        "some avg10=1.00 avg60=2.00 avg300=3.00 total=400\n",
    )
    .expect("cpu pressure");
    std::fs::write(
        cgroup.join("io.pressure"),
        "some avg10=0.10 avg60=0.20 avg300=0.30 total=40\nfull avg10=0.05 avg60=0.06 avg300=0.07 total=8\n",
    )
    .expect("io pressure");
    std::fs::write(
        cgroup.join("workload/cpu.pressure"),
        "some avg10=9.00 avg60=9.00 avg300=9.00 total=900\n",
    )
    .expect("other group pressure");

    let sys = SysFs::new(root.path().to_path_buf());
    let rows = collect(&sys, 100, 100);

    let resources: Vec<u8> = rows.psi.iter().map(|row| row.resource).collect();
    assert_eq!(
        resources,
        vec![0, 2],
        "cpu and io of our own group, memory absent"
    );
    assert_eq!(rows.psi[0].some_total, 400);
    assert_eq!(rows.psi[1].full_total, Some(8));
}

#[test]
fn a_group_without_pressure_files_reports_none() {
    let root = tempfile::tempdir().expect("tempdir");
    let cgroup = root.path().join("fs/cgroup");
    std::fs::create_dir_all(&cgroup).expect("tree");
    std::fs::write(cgroup.join("cgroup.controllers"), "cpu\n").expect("controllers");

    let sys = SysFs::new(root.path().to_path_buf());
    assert!(collect(&sys, 100, 100).psi.is_empty());
}

#[test]
fn a_cpuset_list_counts_the_cpus_it_names() {
    assert_eq!(count_cpus("0-3\n"), Some(4));
    assert_eq!(count_cpus("0-3,8\n"), Some(5));
    assert_eq!(count_cpus("2\n"), Some(1));
    assert_eq!(count_cpus("0-1,4-5,10\n"), Some(5));
    assert_eq!(
        count_cpus("\n"),
        None,
        "an empty list is no ceiling of its own"
    );
    assert_eq!(count_cpus("3-1\n"), None);
    assert_eq!(count_cpus("what\n"), None);
}
