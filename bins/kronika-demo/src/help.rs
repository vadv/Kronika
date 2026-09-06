pub(crate) const HELP: &str = r"kronika-demo - measure a collector run with bounded synthetic activity

Usage: kronika-demo
       kronika-demo -h | --help | --version

Run with the defaults:
  sudo kronika-demo

This runs the adjacent kronika-collector for 60 seconds, generates modest CPU,
memory, disk and loopback activity, then prints collection size, peak RSS and
CPU time. It writes report.json, collector.log and recordings under demo-data
in the current directory. No PostgreSQL is required.
Linux /proc and a runnable collector are required; sudo supplies the collector's
required privileges. Use a separate directory from your regular recordings.

Run controls (all optional; values below are defaults):
  KRONIKA_DEMO_DIR=demo-data
      Writable directory for collector.log and report.json. If storage is
      outside this directory, the report directory must already exist.
  KRONIKA_STORAGE_DIR=$KRONIKA_DEMO_DIR/segments
      Private collector storage root, a real directory, not a symlink.
  KRONIKA_COLLECTOR_BIN=<kronika-collector beside kronika-demo>
      Override the collector executable path.
  KRONIKA_DEMO_DURATION_S=60
      Unsigned whole seconds; 0 runs until Ctrl+C, SIGINT or SIGTERM.
  KRONIKA_DEMO_COLLECTOR_LOG=file
      file replaces collector.log on each run. stderr inherits the terminal
      output streams. The final summary goes to stdout; errors go to stderr.

Collector configuration is inherited, except this harness sets its storage
root as above. For example KRONIKA_RETENTION=536870912 targets 512 MiB and
KRONIKA_LOG_LEVEL=info controls collector logging. The default collector
retention is 2147483648 bytes (2 GiB). PostgreSQL recording is enabled only by
KRONIKA_PG_DSNS, independently of the demo workload DSNs described below.

System activity (enabled by default; independent of PostgreSQL):
  KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED=true
      Exactly true or false. false disables all four system workers.
  KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR=$KRONIKA_DEMO_DIR/system-activity
      Nonblank scratch directory; neither inside nor containing storage.
      Owns one fixed kronika-demo-system-activity.bin file. A stale regular
      file of that name is replaced; symlinks/non-files are refused.
  KRONIKA_DEMO_SYSTEM_CPU_PERCENT=12
      Peak percent of one core, integer 1..25.
  KRONIKA_DEMO_SYSTEM_MEMORY_MIB=32
      Fixed anonymous working set, integer 8..128 MiB.
  KRONIKA_DEMO_SYSTEM_FILE_MIB=8
      Fixed scratch ring, integer 1..32 MiB.
  KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S=32
      Peak payload in each read/write direction, integer 1..256 KiB/s.
  KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S=32
      Peak one-way UDP loopback payload, integer 1..256 KiB/s.
  KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S=5
      File-local flush cadence, integer 1..10 seconds. Disk rate times cadence
      must fit in the ring (KiB/s * seconds <= MiB * 1024).
  CPU, disk and network vary over a 60-second wave averaging 5/8 of peak.
  Loopback uses ephemeral 127.0.0.1 UDP ports, with no external traffic.
  These budgets describe generated activity, not the collector's own cost.

Optional PostgreSQL workload:
  KRONIKA_DEMO_WORKLOAD_DSN             Unset: no database workload.
  KRONIKA_DEMO_WORKLOAD_DIRECT_DSN      Required when WORKLOAD_DSN is set.
  Use a disposable database: this workload creates and changes commerce tables,
  drops/rebuilds an index, runs Vacuum, and deliberately generates errors and
  lock waits. Both DSNs must reach the same database with a role that can create
  schemas and own/modify these objects. No TLS transport is implemented.
  The workload DSN can use PgBouncer transaction pooling; the direct DSN must
  reach PostgreSQL directly for session settings. Neither starts a database.

  Example using existing workload and monitoring connections:
  Replace the DSNs with your configured connection strings:
    sudo env KRONIKA_DEMO_DIR=demo-data \
      KRONIKA_DEMO_DURATION_S=240 \
      KRONIKA_DEMO_WORKLOAD_DSN='host=127.0.0.1 dbname=kronika_demo user=kronika_demo password=replace-workload-password' \
      KRONIKA_DEMO_WORKLOAD_DIRECT_DSN='host=127.0.0.1 dbname=kronika_demo user=kronika_demo password=replace-workload-password' \
      KRONIKA_PG_DSNS='host=127.0.0.1 dbname=kronika_demo user=kronika_monitor password=replace-monitor-password' \
      kronika-demo

PostgreSQL workload controls (only read when WORKLOAD_DSN is set):
  KRONIKA_DEMO_WORKLOAD_SCHEMAS=1
      Commerce schemas, 1..8.
  KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA=8
      Tables per schema, 8..64 (the first eight are the commerce tables).
  KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY=4
      Concurrent setup connections, 1..16.
  KRONIKA_DEMO_WORKLOAD_SESSIONS=4
      Long-lived OLTP clients, 1..16.
  KRONIKA_DEMO_WORKLOAD_TPS=20
      Maximum aggregate OLTP transactions per second, 1..64.
  KRONIKA_DEMO_WORKLOAD_MAX_ORDERS=10000
      Reusable live order slots, from SESSIONS through 50000.
  KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS=1
      Independent lock chains per round, 1..4.
  KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH=4
      Transactions per chain, 2..8.
  KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS=4000
      Milliseconds per link, positive and below 10000. Hold time multiplied by
      (depth - 1) must exceed 10000 to produce the timed-out tail waiter.
  KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S=120
      Positive whole seconds of quiet after each lock round.
  KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S=180
      Positive whole seconds of quiet after each error/slow-query round.
  KRONIKA_DEMO_WORKLOAD_PLAN_ROWS=300000
      Rows in the plan-change story, 1..500000.
  KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS=4
      Concurrent checkout query sessions, 1..8.
  KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S=12
      Positive whole seconds for each indexed baseline/recovery window.
  KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S=30
      Positive whole seconds without the supporting checkout index.
  KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S=120
      Positive whole seconds of quiet after each plan-change round.
  KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS=100000
      Rows in the dedicated Vacuum fixture, 1..250000.
  KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S=180
      Positive whole seconds of quiet after each Vacuum episode.
  KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S=30
      Positive whole seconds per update/Vacuum statement.
  Defaults use four OLTP clients and at most 20 transactions/s; story clients
  and setup connections are additional. Missed pacing ticks are not replayed.
  Locks start after 65 seconds, Vacuum after 95, and events after 140, so the
  default 60-second run does not reach every story. Use 240 seconds as above.

Stopping and output:
  At the deadline or on SIGINT/SIGTERM, the harness stops its workloads, sends
  SIGTERM to collector, allows up to 30 seconds for collector exit, and writes
  report.json. PostgreSQL workload shutdown can first take up to 25 seconds.
  Clean system shutdown removes its one scratch file, preserving recordings.
  report.json is a measurement summary, not an HTML history report. Use
  kronika-report on a finished .zms to create standalone HTML.
  Exit 0 means the harness completed; startup, early collector exit, or
  report-writing failures exit nonzero. Individual workload failures are logged
  and can leave it running.
";
