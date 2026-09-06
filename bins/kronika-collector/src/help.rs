//! Operational help, printed before configuration or collector startup.

pub(crate) const HELP: &str = r"kronika-collector - record Linux metrics, PostgreSQL metrics, and local logs

Usage: kronika-collector
       kronika-collector --help | -h | --version

Runs in the foreground. Configure it with environment variables; there are no
collection flags or public subcommands. Only KRONIKA_STORAGE_DIR is required.

FIRST LAUNCH: LINUX ONLY
  Run these examples from the directory containing the unpacked binaries.
  Create private shared storage on the Linux machine you want to record:

  sudo install -d -m 0700 /var/lib/kronika
  sudo env -u KRONIKA_PG_DSNS -u KRONIKA_POSTGRES_EFFECTIVE_CPUS \
    KRONIKA_STORAGE_DIR=/var/lib/kronika ./kronika-collector

  In another terminal, replace the example web password and start the viewer:

  sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_USER=kronika KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
    ./kronika-web

  Open http://127.0.0.1:8080/ and sign in with those web credentials. Web can
  read the active journal immediately; no finished segment is needed. sudo gives
  the collector access to process details and logs. Both commands use the same
  account so the shared recording stays private. One collector owns a data root.
  Ctrl+C stops either process; the recording remains.

ADD POSTGRESQL
  KRONIKA_PG_DSNS alone enables PostgreSQL metric collection. Leave it unset for
  Linux only. Set up a login on the PostgreSQL server: in psql as an administrator,
  run these commands and enter a new password at the prompt:

  sudo -u postgres psql

  Then at the psql prompt:
  CREATE ROLE kronika_monitor LOGIN INHERIT;
  \password kronika_monitor
  GRANT pg_monitor TO kronika_monitor;
  GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
  \q

  Stop the first collector with Ctrl+C. Restart it against local PostgreSQL,
  replacing the DSN password with the one just entered:

  sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
    KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
    ./kronika-collector

  Stop web and restart it declaring both families as configured:

  sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika KRONIKA_WEB_SOURCES=3 \
    KRONIKA_WEB_USER=kronika KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
    ./kronika-web

  The first DSN supplies metrics from all connectable non-template databases on
  that server, starting with its dbname. Additional DSNs discover logs only;
  do not add a DSN per database. One persistent connection per database is reused
  for up to one hour. Database and extension discovery repeats every 300 seconds
  by default. Activity, Locks, and relation statistics need no extensions;
  installed supported pg_stat_statements and pg_store_plans are discovered.

  The monitoring role needs inherited pg_monitor membership, CONNECT on each
  database, and normal pg_catalog read/function access. It needs no superuser
  or SELECT on application tables. If standard grants were revoked, grant USAGE
  on each extension schema and EXECUTE on its installed reader functions:
  pg_stat_statements(boolean), pg_store_plans() or pg_store_plans(boolean).
  The latter interface also needs pg_store_plans_get_plan(oid,oid,bigint,bigint)
  and pg_store_plans_textplan(text). Installed *_info views need SELECT and
  EXECUTE on their same-named zero-argument functions. Log discovery needs
  pg_current_logfile() and pg_control_system() execution in each DSN database.

  Connect directly or through PgBouncer session pooling. Transaction/statement
  pooling cannot preserve the metric sessions' settings. The native client has
  no TLS support; use a local connection or an independently protected transport.
  Each metric session sets a 30-second statement timeout; the client deadline
  is 35 seconds. DSNs and credentials in these examples are placeholders; keep
  persistent service environment files private.

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      Data root containing active.wal, writer ownership, and finished segments
      under YYYY/MM/DD/<segment-id>.zms. No default. Use a directory, not a ZMS
      filename or symlink; collector and web need the same real data root.

OPTIONAL POSTGRESQL AND LOG ENVIRONMENT (all unset by default)
  KRONIKA_PG_DSNS
      Semicolon-separated keyword DSNs or PostgreSQL connection URLs. The first
      enables metrics; all entries discover PostgreSQL log paths and formats.
  KRONIKA_POSTGRES_EFFECTIVE_CPUS
      Positive whole CPU count (1..4294967295) available to the monitored
      PostgreSQL server, e.g. 4 for a target with four effective CPUs. Requires
      KRONIKA_PG_DSNS. Never inferred from the collector host. Without this,
      PostgreSQL metrics work but PostgreSQL health is null. Set the target's
      actual capacity; do not copy an arbitrary example count.
  KRONIKA_PG_LOGS
      Semicolon-separated local PostgreSQL log paths or globs. Example:
      '/var/log/postgresql/*.csv;/srv/pg-logs/*.json'. Only the last path
      component supports * and ?. Path-only input cannot parse stderr prefixes.
  KRONIKA_PGBOUNCER_DSNS
      Semicolon-separated PgBouncer admin-console DSNs (dbname=pgbouncer), for
      SHOW CONFIG/logfile discovery. The account needs stats_users membership.
      This enables log discovery, not PostgreSQL metric collection.
  KRONIKA_PGBOUNCER_LOGS
      Semicolon-separated local PgBouncer log paths or final-component globs.

  Blank lists mean no sources; blank entries between semicolons are errors.
  A discovered log path must exist on the collector host. Mount remote logs here
  and name them with KRONIKA_PG_LOGS. Paths reached twice are followed once.
  Discovery retries every five minutes; an unavailable source logs a warning
  while other collection continues. Each log read has a 4 MiB buffer and reads
  at most 256 MiB per file per collection.

OPTIONAL STORAGE ENVIRONMENT (sizes are unsigned decimal bytes)
  KRONIKA_SEGMENT_MAX_BYTES       default 67108864 (64 MiB), greater than 0
      Write a finished segment once the journal reaches this many raw bytes.
  KRONIKA_SEGMENT_MAX_AGE_S       default 900 seconds
      Write the open segment at this age; 0 makes it eligible immediately.
  KRONIKA_JOURNAL_MAX_BYTES       default 1073741824 (1 GiB), range 36..1073741824
      Hard active.wal size cap; reaching it writes the segment early. A segment
      threshold larger than this cap logs a warning and the journal cap wins.
  KRONIKA_RETENTION               default 2147483648 (2 GiB)
      Local rotation target: byte count, auto (= auto:80), or auto:P (P=1..99).
      A fixed budget must be at least twice KRONIKA_SEGMENT_MAX_BYTES and counts
      the journal, segments, indexes, and recognized temporaries. For example,
      10737418240 sets 10 GiB. auto:P targets used space on the whole filesystem.
      Rotation removes old finished segments/indexes, preserving active.wal and
      the newest finished segment. It checks after publication and every minute;
      a running collection can delay the check. This is not an exact disk cap.

OPTIONAL COLLECTION INTERVALS (unsigned whole seconds)
  KRONIKA_INTERVAL_S                  default 5; maximum timer sleep
      0 disables timed collection; SIGUSR2 still collects. Positive per-source
      intervals can wake the timer earlier. A per-source 0 reads every timer
      cycle; it does not disable that source.
  KRONIKA_OS_CORE_INTERVAL_S          default 10; CPU, memory, disks, network, PSI
  KRONIKA_OS_MOUNTTOPO_INTERVAL_S     default 60; mounts, capacity, device topology
  KRONIKA_OS_PROCESS_INTERVAL_S       default 5; process counters
  KRONIKA_OS_PROCESS_STATUS_INTERVAL_S default 30; process status details
  KRONIKA_OS_CGROUP_INTERVAL_S        default 30; container cgroup controllers
  KRONIKA_OS_CGROUP_MAPPING_INTERVAL_S default 30; process-to-cgroup mappings
  KRONIKA_LOG_INTERVAL_S              default 10; configured PostgreSQL/PgBouncer logs
  KRONIKA_PG_INTERVAL_S               default 30; PostgreSQL metrics and settings
  KRONIKA_PG_RELATIONS_INTERVAL_S     default 300; relations and database/extension discovery

OPTIONAL LOGGING AND MOUNT PATHS
  KRONIKA_LOG_LEVEL   default info; error, warn (or warning), info, debug, trace
      Case-insensitive. Structured logs go to stderr; readiness and written
      segment paths go to stdout. Segment-write logs include peak rss_kib.
  KRONIKA_PROC_ROOT   default /proc; procfs mount to read
      Setting this limits container detection to that root's cgroup file.
  KRONIKA_SYS_ROOT    default /sys; sysfs mount to read

STOPPING AND ERRORS
  SIGINT (Ctrl+C) and SIGTERM stop collection and retain active.wal. Restart
  with the same data root to recover it. SIGUSR2 forces a collection window and
  segment publication. Invalid configuration and unrecoverable storage failures
  exit nonzero; individual source errors are logged and retried.
  -h/--help and --version print to stdout and exit 0 without starting collection.
";
