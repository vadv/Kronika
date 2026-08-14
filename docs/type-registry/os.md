# Class 1: operating system and cgroup

[Русская версия](os.ru.md)

OS sources occupy `1_100_001`–`1_299_999`. The schemas and semantics are
declared in
[`crates/kronika-registry/src/codec`](../../crates/kronika-registry/src/codec).
This reference lists each section's purpose, source, and collection limits.

Every row carries `scope`:

| Code | Scope |
| ---: | --- |
| `0` | node (`host`) |
| `1` | pod |
| `2` | pod network namespace (`pod_net`) |
| `3` | container |
| `4` | undetermined |

CPU, memory, disks, mount points, and topology describe the node even when the
collector runs inside a container. Network sections get `pod_net` once cgroup
data identifies the container environment. Processes and cgroups get
`container` inside a container, `host` otherwise.

The filesystem roots are overridable with `KRONIKA_PROC_ROOT` (default
`/proc`) and `KRONIKA_SYS_ROOT` (default `/sys`).

## Registered types

| `type_id` | Source | Semantics | Sort key |
|-----------|--------|-----------|----------|
| `1_100_001` | `/proc/PID/{stat,status,io,cmdline}`, hot set | `snapshot_full` | `(pid, starttime, ts)` |
| `1_101_001` | `/proc/PID/status`, extended set | `snapshot_full` | `(pid, starttime, ts)` |
| `1_102_001` | `/proc/stat`: CPU lines | `snapshot_full` | `(cpu_id, ts)` |
| `1_103_001` | `/proc/stat` singletons and `/proc/uptime` | `snapshot_full` | `(ts)` |
| `1_104_001` | `/proc/meminfo` | `snapshot_full` | `(ts)` |
| `1_105_001` | `/proc/loadavg` | `snapshot_full` | `(ts)` |
| `1_106_001` | `/proc/vmstat` | `snapshot_full` | `(ts)` |
| `1_107_001` | `/proc/pressure/*` | `snapshot_full` | `(resource, ts)` |
| `1_108_001` | `/proc/diskstats` | `snapshot_full` | `(major, minor, ts)` |
| `1_109_001` | `/proc/net/dev` plus sysfs link facts | `snapshot_full` | `(iface, ts)` |
| `1_110_001` | `/proc/net/snmp` | `snapshot_full` | `(ts)` |
| `1_111_001` | `/proc/net/netstat` | `snapshot_full` | `(ts)` |
| `1_112_001` | `mountinfo` plus bounded local-filesystem `statvfs` | `on_change` | `(major, minor, mount_point, ts)` |
| `1_113_001` | `/proc/cpuinfo` plus sysfs topology | `on_change` | `(cpu_id, ts)` |
| `1_114_001` | `/proc/interrupts` | `snapshot_full` | `(irq, ts)` |
| `1_115_001` | `/proc/softirqs` | `snapshot_full` | `(vector, ts)` |
| `1_116_001` | `/proc/sys/fs/{file-nr,inode-nr,dentry-state}` | `snapshot_full` | `(ts)` |
| `1_117_001` | `/sys/devices/system/node/node*/meminfo` | `snapshot_full` | `(node_id, ts)` |
| `1_118_001` | `/proc/net/snmp6` | `snapshot_full` | `(ts)` |
| `1_119_001` | `/proc/net/rpc/nfs` | `snapshot_full` | `(ts)` |
| `1_120_001` | `/proc/net/rpc/nfsd` | `snapshot_full` | `(ts)` |
| `1_200_001` | cgroup: process mapping | `snapshot_full` | `(pid, starttime, ts)` |
| `1_201_001` | cgroup: cpu | `snapshot_full` | `(cgroup_path, ts)` |
| `1_201_002` | cgroup: cpu with effective cpuset, retained reader layout | `snapshot_full` | `(cgroup_path, ts)` |
| `1_202_001` | cgroup: memory | `snapshot_full` | `(cgroup_path, ts)` |
| `1_202_002` | cgroup: memory with shared memory, retained reader layout | `snapshot_full` | `(cgroup_path, ts)` |
| `1_203_001` | cgroup: io | `snapshot_full` | `(cgroup_path, major, minor, ts)` |
| `1_204_001` | cgroup: pids | `snapshot_full` | `(cgroup_path, ts)` |
| `1_205_001` | collector cgroup context | `snapshot_full` | `(ts)` |

The collector currently writes the `1_201_001` and `1_202_001` layouts. The
`002` layouts remain registered because existing WAL and ZMS files carry them.

`os_cgroup_context` records the collector process's exact controller paths from
`/proc/self/cgroup`. On cgroup v2 the CPU, memory, and I/O paths are the unified
path. On cgroup v1 they remain controller-specific. `cpuset_cpus` is the count
from `cpuset.cpus.effective` on v2 or `cpuset.effective_cpus` on v1; it is null
when the exact file is missing or cannot be parsed. The collector does not walk
to an ancestor or substitute the host CPU count. `cgroup_version` is `0` when
membership cannot be read or matched to the cgroup tree used for collection.
On v1 `cpu_path` is null when the `cpu` and `cpuacct` controllers place the
process at different paths, because no single stored cgroup CPU row then
contains both usage and quota. A controller path is also null when the current
stored layout lacks any counter or composition operand presented by web;
partial legacy files are not represented as zero.

The collection period is not part of a `type_id`. The collector's scheduler
sets it per source; the intervals and their defaults are listed in the
[collector README](../../bins/kronika-collector/README.md).

## Bounds

A single procfs read is capped at 4 MiB by a format constant. There is no row
cap on a source: a host with thirty thousand processes produces thirty thousand
rows. The `segment_write_finish` log record reports peak RSS as `rss_kib`.

## Units

Every counter and gauge column declares its unit in the contract, and the
registry linter rejects one that does not. The set in use:

| Unit | Where it shows up |
| --- | --- |
| `none` | a bare number with no dimension, such as a load average |
| `count` | entities: processes, threads, handles, interrupts, packets |
| `bytes` | byte counters |
| `kib` | the kibibyte figures the kernel prints in `/proc/meminfo` |
| `pages` | memory pages, of `instance_metadata.page_size_bytes` each |
| `sectors` | the 512-byte sectors of `/proc/diskstats` |
| `seconds`, `milliseconds`, `microseconds`, `nanoseconds` | time |
| `jiffies` | scheduler ticks, of `instance_metadata.clock_ticks_per_sec` per second |
| `hertz` | clock frequency |
| `megabits_per_second` | negotiated link speed |
| `megabytes_per_second` | the I/O rates `PostgreSQL` prints in an autovacuum report |
| `percent`, `celsius` | declared and unused by OS sections |

`TypeContract` is compile-time data and never reaches a segment, so a unit
costs nothing on disk and changes no `type_id`.

## Coverage

The target for this registry is the union of what `atop` records, what the
predecessor project recorded, and what an internal reference tool
recorded. `✓` means the data is in a section above.

### CPU

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| Per-CPU and total user/nice/system/idle/iowait/irq/softirq/steal/guest | ✓ | ✓ | ✓ | ✓ `1_102` |
| Context switches, forks, run and blocked queue | ✓ | ✓ | ✓ | ✓ `1_103` |
| Hardware interrupt total, softirq total | ✓ | — | ✓ | ✓ `1_103` |
| Uptime and cumulative idle | ✓ | — | ✓ | ✓ `1_103` |
| Load average and process counts | ✓ | ✓ | ✓ | ✓ `1_105` |
| Per-IRQ counts | ✓ | — | ✓ | ✓ `1_114` |
| Per-softirq-vector counts | ✓ | — | ✓ | ✓ `1_115` |
| Model, core, socket, max frequency | ✓ | ✓ | — | ✓ `1_113` |
| NUMA node per CPU | ✓ | — | — | ✓ `1_113` |
| Instructions and cycles (`perf`) | ✓ | — | — | — (not collected) |

### Memory and swap

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| Total, free, available, buffers, cache, slab | ✓ | ✓ | ✓ | ✓ `1_104` |
| Dirty, writeback, anon, mapped, shmem, page tables | ✓ | ✓ | — | ✓ `1_104` |
| Commit limit and committed | ✓ | ✓ | — | ✓ `1_104` |
| Huge pages: total, free, size | ✓ | ✓ | — | ✓ `1_104` |
| Huge pages: reserved, surplus, anonymous, shmem | ✓ | — | — | ✓ `1_104` |
| Swap total, free, cached | ✓ | partial | ✓ | ✓ `1_104` |
| Kernel stack, per-CPU, bounce, vmalloc, unevictable, mlocked | ✓ | — | — | ✓ `1_104` |
| Zswap pool and stored size | ✓ | — | — | ✓ `1_104` |
| Page in/out, swap in/out, faults | ✓ | ✓ | ✓ | ✓ `1_106` |
| Scan and steal, kswapd and direct | ✓ | ✓ | ✓ | ✓ `1_106` |
| Scan and steal, khugepaged | ✓ | — | — | ✓ `1_106` |
| Allocation stalls, compaction stalls | ✓ | — | — | ✓ `1_106` |
| NUMA migration, page migration success and failure | ✓ | — | — | ✓ `1_106` |
| Transparent huge page allocations | ✓ | — | — | ✓ `1_106` |
| Working-set refault, restore, node reclaim | — | — | — | ✓ `1_106` |
| Swap read-ahead and hits | — | — | — | ✓ `1_106` |
| OOM kills | ✓ | ✓ | ✓ | ✓ `1_106` |
| Per-NUMA-node memory | ✓ | — | — | ✓ `1_117` |
| KSM sharing, ZFS ARC, balloon | ✓ | — | — | — (not collected) |

### Pressure stall

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| CPU, memory, IO: some and full, avg10/60/300 and total | ✓ | ✓ | ✓ | ✓ `1_107` |

### Storage

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| Reads, writes, sectors, merges, service and queue time | ✓ | ✓ | ✓ | ✓ `1_108` |
| Discards and flushes | ✓ | ✓ | — | ✓ `1_108` |
| In-flight requests | ✓ | ✓ | ✓ | ✓ `1_108` |
| LVM and MD devices | ✓ | ✓ | — | ✓ `1_108` (`/proc/diskstats` lists them) |
| Mount points, filesystem type, source | — | ✓ | ✓ | ✓ `1_112` |
| Filesystem total and free bytes | — | ✓ | ✓ | ✓ `1_112` |
| File handles, inodes, dentries | — | — | ✓ | ✓ `1_116` |

Filesystem capacity is populated only for the explicit local allowlist:
`ext2`, `ext3`, `ext4`, `xfs`, `btrfs`, `f2fs`, `zfs`, `tmpfs`, and `overlay`.
Network, FUSE/userspace, `autofs`, and unknown types remain `null`. The entire
capacity pass has a single one-second deadline; results completed before it are
retained.

### Network

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| Per-interface bytes, packets, errors, drops, fifo, frame, carrier, collisions | ✓ | ✓ | ✓ | ✓ `1_109` |
| Link speed and duplex | ✓ | — | — | ✓ `1_109` |
| TCP opens, resets, segments, retransmits, established | ✓ | ✓ | ✓ | ✓ `1_110` |
| UDP datagrams, errors, no-port | ✓ | ✓ | ✓ | ✓ `1_110` |
| IPv4 receive, deliver, forward, discard, reassembly, fragmentation | ✓ | — | — | ✓ `1_110` |
| ICMP in and out, with errors | ✓ | — | — | ✓ `1_110` |
| Listen overflows and drops, timeouts, retransmit detail | — | ✓ | ✓ | ✓ `1_111` |
| Aborts, memory pressure, backlog drops, pruning, delayed ACKs | — | — | — | ✓ `1_111` |
| Payload octets in and out | ✓ | — | — | ✓ `1_111` |
| IPv6 and ICMPv6 and UDPv6 | ✓ | — | — | ✓ `1_118` |
| NFS client RPC and operations | ✓ | — | — | ✓ `1_119` |
| NFS server RPC, reply cache, I/O | ✓ | — | — | ✓ `1_120` |
| Per-process network I/O | ✓ | — | — | — (not collected) |

### Processes

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| Identity: pid, ppid, uid, gid, name, command line, start time | ✓ | ✓ | ✓ | ✓ `1_100` |
| State, threads, priority, nice, policy, real-time priority, current CPU | ✓ | ✓ | ✓ | ✓ `1_100` |
| User and system CPU time | ✓ | ✓ | ✓ | ✓ `1_100` |
| Run-queue delay and block-I/O delay | ✓ | ✓ | ✓ | ✓ `1_100` |
| Voluntary and involuntary context switches | ✓ | ✓ | ✓ | ✓ `1_100`, `1_101` |
| Minor and major faults | ✓ | ✓ | ✓ | ✓ `1_100` |
| Virtual, resident, and swap footprint | ✓ | ✓ | ✓ | ✓ `1_100` |
| Read and write syscalls, characters, and storage bytes | ✓ | ✓ | ✓ | ✓ `1_100` |
| Data, stack, library, locked, page-table, peak, high-water footprint | ✓ | ✓ | ✓ | ✓ `1_101` |
| File descriptor table size | — | — | — | ✓ `1_101` |
| cgroup of a process | ✓ | ✓ | ✓ | ✓ `1_200` |
| Per-thread rows, `wchan`, proportional set size | ✓ | — | — | — (not collected) |

### cgroup and container

| Metric | atop | predecessor | internal | Kronika |
| --- | :-: | :-: | :-: | :-: |
| CPU usage, user, system, throttling, quota, period | ✓ | ✓ | ✓ | ✓ `1_201` |
| Memory current, max, anon, file, kernel, slab, events, OOM | ✓ | ✓ | ✓ | ✓ `1_202` |
| Block I/O bytes and operations per device | ✓ | ✓ | ✓ | ✓ `1_203` |
| PIDs current and max | ✓ | ✓ | ✓ | ✓ `1_204` |
| cgroup v1 and v2 layouts | ✓ | ✓ | ✓ | ✓ |

## Not collected

The collector omits sources that require extra privileges, kernel modules,
vendor hardware, or more CPU and memory than its host budget allows.

| Missing | Source | Why not |
| --- | --- | --- |
| Instructions, cycles, and other PMU counters | `perf_event_open` | Needs `CAP_PERFMON` or a permissive `perf_event_paranoid`, and opens one event per CPU. |
| GPU utilization and memory | NVML through a helper daemon | Needs a vendor library and a second process; `atop` ships a separate daemon for it. |
| Per-process network I/O | `netatop` kernel module | Needs an out-of-tree module loaded on the monitored host. |
| Infiniband port counters | `/sys/class/infiniband` | Plain sysfs reads; not implemented. |
| Last-level cache occupancy and memory bandwidth | `/sys/fs/resctrl` | Needs `resctrl` mounted and Intel RDT; not implemented. |
| Per-thread rows and `wchan` | `/proc/PID/task/*` | One directory walk per process per snapshot; the cost does not fit the memory and CPU budget. |
| Proportional set size | `/proc/PID/smaps_rollup` | Walks the whole address space per process; measurably expensive on a large shared-buffers backend. |
| KSM, ZFS ARC, hypervisor balloon | `/sys/kernel/mm/ksm`, ZFS module state | Each needs a subsystem that is absent on a plain database host. |
