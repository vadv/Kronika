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
| `1_100_001` | `/proc/PID/{stat,status,io,cmdline}`, hot set | `snapshot_full` | `(pid, ts)` |
| `1_101_001` | `/proc/PID/status`, extended set | `snapshot_full` | `(pid, ts)` |
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
| `1_112_002` | `mountinfo` plus bounded local-filesystem byte/inode `statvfs` | `on_change` | `(major, minor, mount_point, ts)` |
| `1_113_001` | `/proc/cpuinfo` plus sysfs topology | `on_change` | `(cpu_id, ts)` |
| `1_114_001` | `/proc/interrupts` | `snapshot_full` | `(irq, ts)` |
| `1_115_001` | `/proc/softirqs` | `snapshot_full` | `(vector, ts)` |
| `1_116_001` | `/proc/sys/fs/{file-nr,inode-nr,dentry-state}` | `snapshot_full` | `(ts)` |
| `1_117_001` | `/sys/devices/system/node/node*/meminfo` | `snapshot_full` | `(node_id, ts)` |
| `1_118_001` | `/proc/net/snmp6` | `snapshot_full` | `(ts)` |
| `1_119_001` | `/proc/net/rpc/nfs` | `snapshot_full` | `(ts)` |
| `1_120_001` | `/proc/net/rpc/nfsd` | `snapshot_full` | `(ts)` |
| `1_121_001` | CPUFreq policy membership, driver, source, and hardware range from sysfs | `on_change` | `(policy_id, ts)` |
| `1_122_001` | CPUFreq policy frequencies, allowed range, and online CPU count from sysfs | `snapshot_full` | `(policy_id, ts)` |
| `1_123_001` | exact sysfs partition-to-parent block-device edges | `on_change` | `(major, minor, ts)` |
| `1_124_001` | observed process UID-to-username references from `/etc/passwd` | `on_change` | `(scope, uid, ts)` |
| `1_200_001` | cgroup: process mapping | `snapshot_full` | `(pid, ts)` |
| `1_201_001` | cgroup: cpu | `snapshot_full` | `(cgroup_path, ts)` |
| `1_201_002` | cgroup: cpu with effective cpuset, retained reader layout | `snapshot_full` | `(cgroup_path, ts)` |
| `1_202_001` | cgroup: memory | `snapshot_full` | `(cgroup_path, ts)` |
| `1_202_002` | cgroup: memory with shared memory, retained reader layout | `snapshot_full` | `(cgroup_path, ts)` |
| `1_203_001` | cgroup: io | `snapshot_full` | `(cgroup_path, major, minor, ts)` |
| `1_204_001` | cgroup: pids | `snapshot_full` | `(cgroup_path, ts)` |
| `1_205_001` | collector cgroup context | `snapshot_full` | `(ts)` |

The collector currently writes the `1_201_001` and `1_202_001` layouts. The
`002` layouts remain registered because existing WAL and ZMS files carry them.

Workload cgroup sections contain only cgroups named by direct memberships of
live numeric `/proc/<pid>` entries; collection never recursively attributes an
ancestor's descendants. V2 uses its unified membership path. V1 keeps CPU,
memory, block-I/O, and PIDs controller paths separate, and emits a CPU row only
when `cpu` and `cpuacct` name the same path. A tick accepts at most 512 distinct
controller/path candidates and 512 KiB of candidate path bytes. Exceeding
either ceiling omits all workload cgroup sections for that tick. More than
1,024 cgroup/device rows omits the complete I/O section while retaining the
independently complete CPU, memory, and PIDs sections. Collection runs on the
same 30-second cadence as process-to-cgroup mapping.
The collector reuses the process pass's membership reads on that shared tick.

`os_cgroup_context` records the collector process's exact controller paths from
`/proc/self/cgroup`. On cgroup v2 the CPU, memory, and I/O paths are the unified
path. On cgroup v1 they remain controller-specific. `cpuset_cpus` is the count
from `cpuset.cpus.effective` on v2 or `cpuset.effective_cpus` on v1; it is null
when the exact file is missing or cannot be parsed. Cpuset collection does not
walk to an ancestor or substitute the host CPU count.

`effective_cpu_quota_usec` and `effective_cpu_period_usec` store the exact pair
with the smallest quota/period ratio from the configured cgroup root through
the membership leaf. A quota of `-1` with a positive period means every
required controller-bearing level was validated as unlimited. Both fields are
null when any required level is missing or malformed. Cgroup v2 reads
`cpu.max` from the mount root through the leaf. For non-root membership,
`NotFound` at the mount
root alone means an unbounded true root; every descendant is required. Other
root errors are not ignored, and root membership requires a readable root file.
Cgroup v1 binds one unambiguous CPU-controller root at the membership leaf,
then reads `cpu.cfs_quota_us` and `cpu.cfs_period_us` from that same root at
every ancestor. Ratios are compared as ratios, not as raw quota values.

`effective_memory_max` stores the smallest finite v2 `memory.max` across the
same exact hierarchy and applies the same v2 mount-root rule. On cgroup v1, the
field uses the exact leaf's validated `hierarchical_memory_limit`, which already
applies the kernel's hierarchy semantics. The value and the leaf's
`memory.limit_in_bytes` must come from the same bound controller root. A finite
hierarchical value cannot exceed a finite local value, and an unlimited
hierarchical value cannot accompany a finite local value. The field is null for
a validated unlimited value and when these inputs cannot be read coherently. V1
`-1` and values at or above half of `i64::MAX` are unlimited sentinels, never
byte limits.

`cgroup_version` is `0` when membership cannot be read or matched to the cgroup
tree used for collection. On v1 `cpu_path` is null when the `cpu` and `cpuacct`
controllers place the process at different paths, because no single stored
cgroup CPU row then contains both usage and quota. A controller path is also
null when the current stored layout lacks any counter or composition operand
presented by web; partial legacy files are not represented as zero. Local CPU
quota and memory-limit columns continue to report the exact leaf files; they
are not renamed as effective hierarchy values.

The collection period is not part of a `type_id`. The collector's scheduler
sets it per source; the intervals and their defaults are listed in the
[collector README](../../bins/kronika-collector/README.md).

CPUFreq is policy-scoped. `1_121_001` stores exact `related_cpus`, the scaling
driver, the selected actual-frequency attribute, and hardware min/max in
integer hertz. `1_122_001` stores one temporal row per policy. The collector
prefers `cpuinfo_avg_freq`; when that attribute is absent on the policy's first
observation it selects `cpuinfo_cur_freq` instead. The choice stays fixed while
the policy exists. A failed later read remains null and never falls through to
another source. `scaling_cur_freq` is retained separately because it is a
reported or requested policy value, not necessarily a hardware measurement.
One policy sample is never copied into per-logical-CPU rows.

## Bounds

A single procfs read is capped at 4 MiB by a format constant. Process snapshots
have no row cap: a host with thirty thousand processes produces thirty thousand
rows. User-reference capture is separately capped as described below. The
`segment_write_finish` log record reports peak RSS as `rss_kib`.

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
| CPUFreq policy membership and actual/scaling frequency history | ✓ | — | — | ✓ `1_121`, `1_122` |
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
| Filesystem root and total/available inodes | — | — | — | ✓ `1_112` |
| Exact partition-to-parent device edges | — | — | — | ✓ `1_123` |
| File handles, inodes, dentries | — | — | ✓ | ✓ `1_116` |

Filesystem capacity is populated only for the explicit local allowlist:
`ext2`, `ext3`, `ext4`, `xfs`, `btrfs`, `f2fs`, `zfs`, `tmpfs`, and `overlay`.
Network, FUSE/userspace, `autofs`, and unknown types remain `null`. The entire
capacity pass has a single one-second deadline; results completed before it are
retained.

`1_112_002` directly replaces the unreleased mount layout. Its identity is
`(major, minor, mount_point)`, so two mount points exposing the same filesystem
remain distinct. `root` is mountinfo field 4. Byte availability is `f_bavail`
and inode availability is `f_favail`; neither is renamed to free or used space.
`1_123_001` emits only an exact sysfs partition marker and its immediate parent
device identity. Whole devices, dm/LVM/MD layers, unresolved sysfs links, and
bind-mount ancestry emit no inferred edge.

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
| Real and effective user names, with numeric UID retained | ✓ | ✓ | ✓ | ✓ `1_100`, `1_124` |
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

`os_user` records at most one mapping for each observed `(scope, uid)` in an
open segment. Only real and effective UIDs from successfully decoded
`os_process` rows are candidates. A new UID observed later in the same segment
is appended through the normal WAL path; a failed append leaves it eligible for
the next window. The row stores the collection timestamp, UID, interned user
name, provenance `source=0` (`/etc/passwd`), and scope.

The collector reads exactly `/etc/passwd` once per open segment. The read is
bounded to 256 KiB, each line to 4 KiB, the parsed snapshot to 4,096 entries,
and each user name to 256 bytes. Malformed or overlong entries are skipped;
an oversized file or entry count disables only user-name enrichment for that
segment. There is no NSS, LDAP, SSSD, or web-service lookup. Missing and
dynamic users therefore remain numeric in the API and interface.

Readers join `os_process` only to `os_user` and `dict.strings` from the same
segment, keyed by `(scope, uid)`. Human-facing `user` and `effective_user`
search selectors are resolved by the server before ordering and pagination;
`user_id` and `effective_user_id` remain exact numeric selectors. Ordinary
text search covers command plus both resolved names. No query path consults
the live host identity database.

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
