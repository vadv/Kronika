# Справочник метрик Linux

[English version](metrics-linux.md) · [Указатель справочника](features.ru.md) · [Время и heatmaps](metrics-time.ru.md)

`R(x) = (x₁ − x₀) / Δt`, где `Δt` — записанное время между samples в секундах. `H` — записанный `clock_ticks_per_sec`; `N(t)` — число записанных online host logical CPUs на timestamp sample; `N_lane` — число уникальных CPU IDs для CPU usage lane. Правила rates и недоступных значений определены [для каждого пути расчёта](metrics-time.ru.md#правила-пар-и-единицы).

## Записанные environment и scope

| Записанный факт | Значение и использование |
|---|---|
| `instance_metadata.environment` | Записанный collector environment machine/container. Управляет наличием container resource rows и cgroup collection. VM относится к machine. |
| OS `scope` | `0`: host; `1`: pod; `2`: pod network namespace; `3`: container; `4`: unknown. |
| Host CPU, memory, pressure, block devices | Значения kernel, видимые через настроенные procfs/sysfs roots. Container recordings сохраняют host context там, где эти файлы его предоставляют. |
| Process table | Живые PID, видимые через настроенный procfs root; identity — числовой PID в выбранном часе. |
| Network в container | Interfaces и traffic, видимые в network namespace, записанные с pod-network scope. |
| Cgroup resource lanes | Точный путь membership collector для соответствующего controller и совпадающий записанный scope. Workload tables могут содержать другие непосредственно населённые cgroups, видимые collector. |
| Filesystems | Data mounts, видимые в mount namespace collector; capacity читается через `statvfs` по видимому mount path. |

На machine/VM collector не записывает workload cgroup sections. В container он опрашивает paths, непосредственно содержащие видимые живые процессы, с пределами 512 controller/path candidates, 512 KiB текста paths и 1,024 cgroup/device I/O rows за tick. Превышение предела candidates исключает workload sections этого tick; превышение предела I/O исключает I/O. Источники: [scope](../crates/kronika-source-os/src/scope.rs), [collection](../bins/kronika-collector/src/os_sources/cgroups.rs), [cgroup reader](../crates/kronika-source-os/src/cgroup.rs), [UI environment](../bins/kronika-web/ui/src/system-view.tsx).

## Processes

### Lenses и поля

| Lens | Колонки |
|---|---|
| General | PID, command, PPID, real/effective user, GID/EGID, threads, TTY, exit signal, state |
| CPU | PID, command, user/system CPU, run delay, block delay, voluntary/involuntary context switches, CPU number, nice, priority, real-time priority, scheduling policy, state |
| Memory | PID, command, RSS, virtual memory, swap, minor/major faults, state |
| Disk | PID, command, read/write bytes, read/write syscalls, logical read/write bytes, cancelled writes, block delay, state |
| Tree | Real user, PID, CPU %, memory %, virtual memory, RSS, TTY, STAT, start time, cumulative CPU TIME, command tree с родителями перед потомками |

Источники: [`LENS_FIELDS`](../bins/kronika-web/ui/src/process-table.tsx), [поля истории inspector](../bins/kronika-web/ui/src/detail.tsx), [tree](../bins/kronika-web/ui/src/process-tree.ts).

| Поле или отображаемая метрика | Записанный источник и расчёт | Единица и значение |
|---|---|---|
| PID, PPID | `/proc/PID/stat`: `pid`, `ppid` | Числовая identity процесса/родителя |
| Command | `/proc/PID/cmdline`; fallback `/proc/PID/comm` | Записанный текст команды |
| Real/effective user | Real/effective UID из `/proc/PID/status`; passwd mapping collector | Записанный username; числовой UID при отсутствии имени. GID/EGID — real/effective group IDs. |
| State | `/proc/PID/stat.state` | Kernel state letter; `R` runnable, `S` sleeping, `D` uninterruptible sleep, `Z` zombie, `T` stopped, `I` idle kernel thread |
| Threads | `num_threads` | Текущее число threads |
| TTY, exit signal | `tty_nr`, `exit_signal` из `stat` | Код controlling terminal и сигнал завершения |
| User CPU / System CPU | `R(utime) / H`, `R(stime) / H` | Core equivalents: 1 означает одну CPU second на wall second |
| Run delay | `R(rundelay_ns)` из `/proc/PID/schedstat` | ns/s ожидания CPU; форматируется как duration на секунду |
| Block delay | `R(blkdelay_ticks)` из `stat.delayacct_blkio_ticks` | Jiffies/s в таблице/истории; structured-search quantity `block_io_delay` преобразует в `1,000 × R(blkdelay_ticks) / H` ms/s |
| Voluntary / Involuntary switches | `R(nvcsw)`, `R(nivcsw)` из `status` | Switches/s |
| CPU, nice, priority, RT priority, policy | `curcpu`, `nice`, `prio`, `rtprio`, `policy` из `stat` | Текущий/последний CPU number и записанные scheduler settings |
| RSS | `rmem_kb × 1,024` | Resident bytes; shared resident pages входят в каждый отображающий их процесс |
| Virtual memory | `vmem_kb × 1,024` | Bytes виртуального address space |
| Swap | `vswap_kb × 1,024` из `status.VmSwap` | Swap bytes процесса |
| Minor / Major faults | `R(minflt)`, `R(majflt)` из `stat` | Faults/s; minor faults не требуют storage read, major faults требуют page-in I/O |
| Read / Write | `R(read_bytes)`, `R(write_bytes)` из `/proc/PID/io` | Bytes/s, учтённые как storage I/O |
| Read / Write syscalls | `R(syscr)`, `R(syscw)` из `io` | Calls/s |
| Logical read / write | `R(rchar)`, `R(wchar)` из `io` | Bytes/s учтённых read/write operations, включая traffic через cache |
| Cancelled writes | `R(cancelled_write_bytes)` из `io` | Bytes/s отменённой записи dirty pages |
| Tree CPU % | `100 × (R(utime) + R(stime)) / H` | Процент одного logical CPU; может превышать 100% |
| Tree memory % | `100 × rmem_kb / mem_total` | Доля записанного host `MemTotal`; нужен положительный знаменатель |
| Tree TIME | `(utime + stime) / H` из cumulative `cpu_time_ticks` | CPU seconds за жизнь процесса, формат clock duration |
| Tree start / STAT | Записанный Unix start timestamp; state, затем `<` при negative nice, `N` при positive nice, `l` при нескольких threads | Наблюдаемые start time и flags; порядок родителей использует выбранный process snapshot |

Поля `/proc/PID/io` nullable. Collector пробует filesystem credentials процесса и собственные исходные credentials; ошибка доступа оставляет counters недоступными. В успешно прочитанном I/O file отсутствующие или ошибочные numeric entries получают ноль. Нечитаемый `schedstat` записывает нулевой run delay; отсутствующие status counters сохраняют defaults parser. Источники: [process registry](../crates/kronika-registry/src/codec/os_process.rs), [proc parser](../crates/kronika-source-os/src/proc/process/parse.rs), [process reader](../crates/kronika-source-os/src/proc/process.rs), [I/O reader](../crates/kronika-source-os/src/proc/process/process_io.rs), [tree calculations](../bins/kronika-web/ui/src/process-tree.ts).

`rchar` измеряет logical read traffic; `read_bytes` — storage traffic, учтённый для процесса. Process lenses показывают оба rate отдельно.

### Summary и история

Summary обрабатывает полный process snapshot независимо от страницы таблицы и поиска. General и Tree показывают число процессов, сумму threads, число `state = R` и число PID, присутствующих в последнем записанном PostgreSQL activity snapshot не позже process timestamp. CPU показывает суммы user/system cores, run delay в ms/s и voluntary плюс involuntary switches/s. Memory — суммы RSS, virtual memory, swap и major faults/s. Disk — суммы read/write bytes/s и read/write calls/s.

Каждый rate вычисляется по PID перед суммированием. Rate добавляет только PID, присутствующий в непосредственно предыдущем process snapshot с тем же `starttime`. Доступные значения добавляются независимо; сумма без доступных значений равна null. Суммарный RSS учитывает shared pages для каждого записанного process mapping. Источник: [`summaries`, `add_row`, `ExactSum`, `RateSum`](../crates/kronika-query/src/hour/process_summary.rs).

Inspector history использует temporal fields выбранного lens. Scheduler settings, identities и command text — reference fields. CPU history также содержит minor/major faults; Tree — поля CPU, memory и disk history. Process activity heatmaps группируются по `comm`; CPU ранжируется по приросту CPU time, RSS — по [среднему с общим множеством timestamps](metrics-time.ru.md#среднее-rss-grid).

## Host CPU и pressure

Для aggregate host row `/proc/stat` обозначим восемь компонентов интервала `u,n,s,i,w,q,f,z` как разности `user,nice,system,idle,iowait,irq,softirq,steal`. Пусть `T = u+n+s+i+w+q+f+z`, `B = T−i−w`.

| Метрика | Формула или записанное поле | Единица |
|---|---|---|
| CPU usage / USE Busy | `100 × R(user+nice+system+irq+softirq+steal) / (H × N_lane)` | % host CPU capacity |
| CPU used, cores | `N(t) × B / T` | Core equivalents |
| CPU cores | Число записанных уникальных nonnegative `cpu_id` на timestamp, host scope | Logical CPUs |
| User CPU | `100 × (u+n) / T` | % |
| System CPU | `100 × s / T` | % |
| IRQ | `100 × (q+f) / T` | % |
| I/O wait / Steal / Idle | `100 × w/T`, `100 × z/T`, `100 × i/T` | % |
| Actual frequency | `Σ(actual_frequency_hz × online_cpus) / Σonline_cpus / 10⁶` | MHz, взвешено по CPUFreq policies |
| Scaling frequency | Та же взвешенная формула для `scaling_cur_freq_hz` | MHz |
| Procs running / Procs blocked | `/proc/stat.procs_running`, `procs_blocked` | Gauges runnable / I/O-blocked processes |
| Context switches | `R(ctxt)` из `/proc/stat` | Switches/s |
| Load 1m / 5m / 15m | `/proc/loadavg`: `load1`, `load5`, `load15` | Kernel load averages для runnable и uninterruptible tasks |
| Runnable tasks / Tasks | `/proc/loadavg`: `running`, `total` | Текущие scheduler task counts |
| CPU / Memory / I/O PSI, 10s | `/proc/pressure/{cpu,memory,io}`: `some_avg10` | Записанный kernel % времени с хотя бы одним stalled task, среднее 10 секунд |
| CPU PSI / I/O PSI interval lane | `100 × Δsome_total / (10⁶ × Δt)` | % фактического интервала samples |

CPU composition требует все восемь неотрицательных разностей и положительный `T`. Used cores дополнительно требует положительный `N(t)`. CPU usage lane использует recorded clock rate и host CPU count; composition использует сумму восьми counters в знаменателе. Actual frequency требует значение и online count каждой policy, положительный total online count и одинаковый записанный hardware source у всех policies. Reader предпочитает `cpuinfo_avg_freq`, затем `cpuinfo_cur_freq`; scaling frequency отдельно читает `scaling_cur_freq`. Источники: [host formulas](../bins/kronika-web/ui/src/system-view.tsx), [USE/timeline lanes](../crates/kronika-query/src/hour/lanes.rs), [CPUFreq](../crates/kronika-source-os/src/cpufreq.rs).

Для CPU usage lane `N_lane` — размер множества всех уникальных nonnegative `cpu_id`, встреченных в source segment; множество заполняется до проверок scope и timestamp строки. CPU composition и CPU cores gauge считают CPU на каждом timestamp. PSI chart за 10 секунд выбирает `os_psi` по resource; запрос не добавляет scope filter. Host/container interval PSI lanes используют явный выбор scope.

PSI также записывает `some_avg60`, `some_avg300`, cumulative `some_total`, а для memory/I/O — `full_avg10/60/300`, `full_total`. `some` считает время ожидания хотя бы одного task; `full` — время ожидания всех non-idle tasks. Записанные averages — kernel gauges; interval lanes и [health](metrics-time.ru.md#health) вычисляются из cumulative `some_total`. Источник: [PSI reader](../crates/kronika-source-os/src/proc/pressure.rs).

## Host memory

Все memory gauges ниже берутся из `/proc/meminfo`, в KiB до преобразования отображения.

| Метрика | Поле или формула | Значение |
|---|---|---|
| In use | `100 × (mem_total − mem_available) / mem_total` | Host memory utilization; total должен быть положительным |
| MemTotal / MemAvailable / MemFree | `mem_total`, `mem_available`, `mem_free` | Usable physical memory / оценка kernel доступной памяти без swapping / неиспользуемая память |
| AnonPages | `anon_pages` | Anonymous pages |
| Page cache | `cached + buffers` | File cache и block-device buffers |
| Reclaimable slab / Unreclaimable slab | `s_reclaimable`, `s_unreclaim` | Reclaimable / currently unreclaimable slab |
| Other memory | `mem_total − mem_free − cached − buffers − anon_pages − s_reclaimable − s_unreclaim` | Остаток; null при отсутствующем operand или отрицательном результате |
| Free swap / Total swap | `swap_free`, `swap_total` | Свободный / настроенный swap |
| Swapped pages | `R(pswpin + pswpout)` из `/proc/vmstat` | Pages/s в обоих направлениях |
| OOM kills | `R(oom_kill)` из `/proc/vmstat` | Kills/s |

`MemAvailable` пересекается с reclaimable memory categories и не является дополнительным компонентом memory composition. Источники: [memory parser](../crates/kronika-source-os/src/proc/meminfo.rs), [VM counters](../crates/kronika-source-os/src/proc/vmstat.rs), [composition](../bins/kronika-web/ui/src/system-view.tsx).

## Storage и filesystems

### Device I/O

Identity устройства — записанный `major:minor` с device name. Источник counters — `/proc/diskstats`; один sector в расчётах равен 512 bytes.

| Поле устройства | Формула | Единица |
|---|---|---|
| Reads / Writes | `R(reads)`, `R(writes)` | Завершённые operations/s |
| Read / Write bytes | `512 × R(read_sectors)`, `512 × R(write_sectors)` | B/s |
| Read / Write latency | `Δread_time_ms / Δreads`, `Δwrite_time_ms / Δwrites` | ms/operation; null при нуле операций |
| Device busy | `100 × Δio_time_ms / (1,000 × Δt)` | % интервала с активным I/O |
| Queue depth | `Δio_weighted_time_ms / (1,000 × Δt)` | Среднее число active или waiting requests |
| Active I/O | `io_in_progress` | Текущие requests |

Host charts **Device busy** и **Queue depth** берут максимальное значение устройства на каждом timestamp. Breakdown lines показывают отдельные devices; устройства с нулевыми значениями весь час скрываются, если есть хотя бы одно активное. USE Storage cells используют `min(100, 100 × R(Σio_time_ms)/1,000)` и `R(Σio_weighted_time_ms)/1,000`. **Active I/O** в host overview — `Σio_in_progress`; **Block devices** — число записанных device rows.

Источники: [diskstats parser](../crates/kronika-source-os/src/proc/diskstats.rs), [`SYSTEM_ENTITIES`, `latencyPoints`, `peakDeviceRate`](../bins/kronika-web/ui/src/system-view.tsx), [`read_disk`, `points`](../crates/kronika-query/src/hour/lanes.rs).

### Mounted filesystems

| Поле | Источник/формула | Единица и scope |
|---|---|---|
| Mount point, root, source, type, device | `/proc/self/mountinfo` | Identity видимого mount; root — subtree filesystem, доступное через mount |
| Total bytes | `f_blocks × f_frsize` | Bytes |
| Free bytes / Available | Записанный `free_bytes = f_bavail × f_frsize` | Bytes для unprivileged writes; reserved free blocks исключены |
| Available % | `100 × free_bytes / total_bytes` | Нужен положительный total |
| Used bytes в paired chart | `total_bytes − free_bytes` | Bytes за пределами available portion |
| Total / available inodes | `f_files`, `f_favail` | Inode/file-serial counts |
| Available inode % | `100 × available_inodes / total_inodes` | Нужен положительный total |
| Used inodes в paired chart | `total_inodes − available_inodes` | Inodes за пределами available portion |
| Minimum filesystem free | Минимальный available-byte percentage среди записанных mounts на timestamp | %; отсутствие необходимого mount value делает aggregate недоступным |
| Filesystems | Число записанных mount rows | Count |
| Kubernetes infrastructure | `is_k8s_infra` из записанного mount path | Классификация известных infrastructure bind mounts |

Reader исключает pseudo filesystems и mounts внутри `/proc` или `/sys`; data-bearing `tmpfs` и `overlay` остаются eligible. Произведения `statvfs` ограничиваются `i64::MAX`. Источники: [filesystem capacity](../crates/kronika-source-os/src/fs.rs), [выбор mounts](../crates/kronika-source-os/src/mount.rs), [paired charts](../bins/kronika-web/ui/src/system-view.tsx).

### Topology reference

CPU topology содержит logical CPU, socket, core, NUMA node, model и maximum MHz. CPUFreq reference — policy membership, driver, actual-frequency source и hardware limits. Storage topology связывает `major:minor` devices, partition/stack edges, parent/slave relationships и видимые mounts. Это identity/configuration records, выбранные на cursor; topology fields не становятся history metrics. Источники: [CPU topology](../crates/kronika-source-os/src/proc/cpuinfo.rs), [block topology](../crates/kronika-source-os/src/block_topology.rs), [reference views](../bins/kronika-web/ui/src/system-view.tsx).

## Network

| Метрика | Источник/формула | Единица |
|---|---|---|
| RX / TX interface | `R(rx_bytes)`, `R(tx_bytes)` из `/proc/net/dev` | B/s |
| RX / TX packets | `R(rx_packets)`, `R(tx_packets)` | Packets/s |
| RX / TX errors | `R(rx_errs)`, `R(tx_errs)` | Errors/s |
| RX / TX drops | `R(rx_drop)`, `R(tx_drop)` | Packets/s |
| Speed / duplex | Записанные sysfs link speed `speed_mbit`, `duplex` | Mbit/s и duplex setting; reference fields |
| Host/namespace RX / TX | `R(Σrx_bytes)`, `R(Σtx_bytes)` | B/s по записанным interfaces |
| Net errors | `R(Σ(rx_errs + tx_errs))` | Errors/s |
| Drops chart | `R(Σ(rx_drop + tx_drop))` | Drops/s |
| USE Drops | `R(Σ(rx_drop + tx_drop + rx_fifo + tx_fifo))` | Drops/FIFO errors на секунду |
| Interfaces | Число записанных interface rows | Count |

Aggregate суммирует записанные counters каждого timestamp перед дифференцированием. Link speed не переводит RX/TX в проценты. Источники: [network parser](../crates/kronika-source-os/src/proc/net_dev.rs), [aggregate charts](../bins/kronika-web/ui/src/system-view.tsx), [USE network lanes](../crates/kronika-query/src/hour/lanes.rs).

## Container cgroups

### Capacity и membership

`os_cgroup_context` записывает cgroup version, точные CPU/memory/I/O paths collector, effective cpuset count, минимальное применимое CPU quota/period ratio в hierarchy и effective memory ceiling. При положительных quota `Q`, period `P` и пригодном cpuset count `S` CPU capacity равна `min(Q/P, S)`; при отсутствии `S` — `Q/P`. Записанная quota `−1` выбирает `S`; unknown quota hierarchy оставляет capacity равной null. Memory capacity — записанный validated hierarchical ceiling. Положительные local controller limits и effective ancestor limits — отдельные поля.

Cgroup v2 проверяет применимые files от настроенного hierarchy root до точного membership. Отсутствующий mount-root control file считается unbounded только для non-root membership; необходимые descendant files должны быть валидными. Cgroup v1 использует однозначный controller root; memory проверяет `hierarchical_memory_limit` совместно с leaf limit. Effective capacity/context добавляется только к строке таблицы с совпадающими collector path и scope. Источники: [hierarchy reader](../crates/kronika-source-os/src/cgroup.rs), [context contract](../crates/kronika-registry/src/codec/os_cgroup_context.rs), [`cgroup_cpu_capacity`](../crates/kronika-query/src/hour/lanes.rs), [`systemEntityRows`](../bins/kronika-web/ui/src/system-view.tsx).

### Controller metrics

| Метрика или записанное поле | Расчёт/источник | Единица |
|---|---|---|
| CPU used / user / system | `R(usage_usec) / 10⁶`, `R(user_usec) / 10⁶`, `R(system_usec) / 10⁶` | Core equivalents; v2 `cpu.stat`, v1 `cpuacct` |
| Other CPU | `R(usage_usec − user_usec − system_usec) / 10⁶`, из трёх разностей | Cores; null при отрицательной разности компонента или остатке |
| CPU share | `100 × used_cores / effective_capacity` | %; недоступно без capacity |
| CPU quota / period | `quota_usec`, `period_usec`; quota cores отображаются как `Q/P` при положительных operands | Local controller ceiling; quota `−1` — unlimited |
| Throttled | `100 × R(throttled_usec) / 10⁶` | % wall interval; без деления на capacity и ограничения 100% |
| Throttling events | Записанный cumulative `nr_throttled` | Count; cgroup CPU record |
| CPU / memory / I/O PSI | `100 × R(some_total) / 10⁶` для pressure collector cgroup | % интервала samples |
| Memory current | v2 `memory.current`; v1 `memory.usage_in_bytes` | Bytes |
| Memory share | `100 × current / effective_memory_max` | % положительного hierarchical ceiling |
| Local memory max | v2 `memory.max`; v1 `memory.limit_in_bytes` | Bytes; unlimited представлен null |
| Anon / File / Slab | `anon`, `file`, `slab` из `memory.stat` | Bytes |
| Other kernel | `kernel − slab` | Bytes; null при отсутствующем operand или отрицательной разности |
| Unclassified memory | `current − anon − file − kernel` | Bytes; null при отсутствующем operand или отрицательной разности |
| Shared memory, если записана | `shmem` в новом memory layout | Bytes, включённые в `file` |
| Memory events | Cumulative `low_events`, `high_events`, `max_events`, `oom_events`, `oom_kill`; v1 `memory.failcnt` записывается в `max_events` | Counts; OOM lane — `R(oom_kill)` kills/s |
| I/O read/write | `R(rbytes)`, `R(wbytes)` из v2 `io.stat` или v1 blkio service-byte files | B/s на cgroup и device |
| I/O operations | `R(rios)`, `R(wios)` | Operations/s на cgroup и device |
| Потоки (TID) / Локальный pids.max | Прямые `pids.current`, `pids.max` | Threads (TIDs) в cgroup subtree и local subtree limit |
| К pids.max | `100 × current / max` при положительном local max | %; literal `max` записывается как null unlimited limit |

Cgroup I/O lane суммирует device counters точного I/O path collector перед вычислением read/write rates. Каждый I/O counter может оставаться доступным независимо. Device table содержит `major:minor`, stacked-device chain, visible mount associations и свёрнутые lower-layer counters в inspector. Associations сохраняют записанный cgroup/device scope. Число `pids.current` включает descendants и main thread каждого процесса; число process rows имеет другую единицу. Для записи строки потоков нужны валидные `pids.current` и `pids.max`. Источники: [controller parsing](../crates/kronika-source-os/src/cgroup.rs), [CPU](../crates/kronika-registry/src/codec/os_cgroup_cpu.rs), [memory](../crates/kronika-registry/src/codec/os_cgroup_memory.rs), [I/O](../crates/kronika-registry/src/codec/os_cgroup_io.rs), [Потоки](../crates/kronika-registry/src/codec/os_cgroup_pids.rs), [device associations](../bins/kronika-web/ui/src/cgroup-device.ts).

## USE table и verdicts

Колонки USE — Utilization (U), Saturation (S), Errors (E). Cells читают lane не позже cursor. Resource rows используют следующие значения:

| Resource | U | S | E |
|---|---|---|---|
| Host CPU | CPU usage % | CPU PSI interval % | Недоступно |
| Host memory | In use % | Swapped pages/s | OOM kills/s |
| Host storage | Capped summed busy % | Summed average queue | Недоступно |
| Host/namespace network | RX и TX B/s | Drops включая FIFO/s | RX + TX errors/s |
| Cgroup CPU | Capacity share %, fallback used cores | Throttled % и CPU PSI % | Недоступно |
| Cgroup memory | Effective-limit share %, fallback current bytes | Memory PSI % | OOM kills/s |
| Cgroup I/O | Read и write B/s | I/O PSI % | Недоступно |
| Cgroup Потоки | Local-limit share %, fallback current count | Недоступно | Недоступно |

| Verdict | Точный reducer |
|---|---|
| U | Наибольшая доступная U cell в процентах на cursor. Byte/count fallbacks и network throughput не участвуют. При равенстве выбирается первый resource в порядке строк. |
| S | Показывает каждый положительный максимум выбранного часа из доступных S lanes, включая secondary lanes. Ссылка ведёт к resource с наибольшим percentage maximum; lanes в других units используют comparison value `−1`. Доступные полностью нулевые lanes дают Quiet; отсутствие доступных S lanes даёт `—`. |
| E | Для каждой error lane суммирует `rate(tᵢ) × (tᵢ−tᵢ₋₁)/10⁶` по отсортированным points, затем суммирует resources. Null/nonfinite current rate ничего не добавляет. Результат отображается как округлённое число events. Ссылка ведёт к resource с наибольшим положительным integrated count. Доступные нулевые totals дают Quiet; отсутствие error lane даёт `—`. |

Источник: [`USE_RESOURCES`, `resolveCell`, `ledgerVerdicts`, `integrateRate`](../bins/kronika-web/ui/src/use-table.tsx).

Выбор primary или fallback U lane зависит от наличия хотя бы одного конечного sample за весь час. Если primary lane содержит sample часа, но равна null на cursor, cell остаётся null. Secondary S lane участвует только после разрешения primary lane. Порядок resources: cgroup CPU, memory, I/O, Потоки, затем host CPU, memory, storage, network; недоступные container rows исключаются. Строгие сравнения сохраняют этот порядок при равенстве verdict values.

## Фиксированные marks и цвета cells

Linux и overall-health timeline marks используют следующие predicates для поддерживаемых записанных layouts:

| Mark | Predicate |
|---|---|
| Host CPU | `100 × B ≥ 80 × T`, при положительном `T` и неотрицательных разностях восьми counters |
| Host load | `load1 ≥ 2 × online_CPU_count` на том же timestamp |
| Host memory | `100 × MemAvailable ≤ 10 × MemTotal`, валидные `0 ≤ available ≤ total`, положительный total |
| Host filesystem | `100 × (total_bytes − free_bytes) ≥ 90 × total_bytes`, валидные bounds, положительный total |
| Host / cgroup OOM | Более поздний записанный `oom_kill` превышает предыдущее значение для scope/identity |
| Overall health | Известное значение `< 50`; overall-health chart также рисует threshold 50 |

Цвета process cells задаются отдельно: state `R` good, `D` warning, `Z` critical, `I` inactive; Tree CPU — warning при `≥50%`, critical при `≥90%`; нулевой rate — inactive. Источники: [фиксированные predicates](../crates/kronika-index/src/detect/direct.rs), [cell tones](../bins/kronika-web/ui/src/value-tone.ts), [health chart threshold](../bins/kronika-web/ui/src/timeline.tsx).
