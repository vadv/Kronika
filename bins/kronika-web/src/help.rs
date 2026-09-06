//! Operational help, printed before configuration or server startup.

pub(crate) const HELP: &str = r"kronika-web - browse a Kronika recording and serve its HTTP API and MCP tools

Usage: kronika-web
       kronika-web --help | -h | --version

Runs in the foreground. Configuration is environment-only. Required variables:
KRONIKA_STORAGE_DIR, KRONIKA_WEB_USER, KRONIKA_WEB_PASSWORD, KRONIKA_WEB_SOURCES.
The default address is 127.0.0.1:8080; sign in at http://127.0.0.1:8080/.

FIRST LAUNCH: LINUX ONLY
  Use your chosen recording directory. To start collection:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording kronika-collector

  In another terminal, replace the password and start web over the same storage:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_USER=kronika KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
    kronika-web

  Open http://127.0.0.1:8080/ and sign in as kronika with that password. The
  active journal is readable immediately; no finished segment is needed. sudo
  lets the collector read process details/logs, and using the same account for
  web keeps storage private. A service account with the required data-directory
  permissions can also run web. Ctrl+C stops either process, keeping the data.

CHOOSE KRONIKA_WEB_SOURCES (required; no default)
  0  Neither family configured. Accepted, but not a normal first-launch choice.
  1  Linux OS only. Use this for a collector without PostgreSQL DSNs.
  2  PostgreSQL only. Use when declaring only PostgreSQL as configured.
  3  Linux OS + PostgreSQL. Use this when the collector has KRONIKA_PG_DSNS.

  These values declare source families in the web/API/MCP catalog. They do not
  start collection or filter saved sections, and are not an access-control list.
  All navigation tabs remain available. In the browser, declaring PostgreSQL
  suppresses its no-data tooltip; saved PostgreSQL metrics do that too. Source
  declarations do not change recorded health. Web reads recordings; it does not
  connect to PostgreSQL.
  KRONIKA_PG_DSNS on the collector is what enables PostgreSQL metric collection.

ADD POSTGRESQL TO THE LINUX RECORDING
  In psql on the database server as an administrator, create a monitoring login
  and enter a new password at the prompt:

  sudo -u postgres psql

  Then at the psql prompt:
  CREATE ROLE kronika_monitor LOGIN INHERIT;
  \password kronika_monitor
  GRANT pg_monitor TO kronika_monitor;
  GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
  \q

  Stop the existing collector with Ctrl+C and restart it, replacing the DSN
  password with the one just entered. This example uses local PostgreSQL:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording \
    KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
    kronika-collector

  Stop web and restart it with both source families:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=3 \
    KRONIKA_WEB_USER=kronika KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
    kronika-web

  The first collector DSN supplies metrics from that server's connectable
  databases; additional semicolon-separated DSNs discover logs only. No DSN per
  database is needed. The role needs inherited pg_monitor, CONNECT on each
  database, and normal catalog permissions. If defaults were revoked, grant
  extension schema USAGE and reader-function EXECUTE where installed; installed
  *_info views also need SELECT. kronika-collector --help lists these grants.
  No application-table SELECT or monitoring-role superuser is needed.
  Use a direct connection or PgBouncer session pooling; transaction/statement
  pooling is unsupported. The native client has no TLS support; use a local
  connection or an independently protected transport. Log paths must be readable
  on the collector host; KRONIKA_PG_LOGS can name mounted remote logs.

  PostgreSQL health also needs KRONIKA_POSTGRES_EFFECTIVE_CPUS on the collector:
  a positive whole count of the monitored server's effective CPUs (for example,
  4 only if that target has four). It requires KRONIKA_PG_DSNS and has no default.
  Without it, PostgreSQL metrics work but PostgreSQL health is null. This capacity
  is never inferred from the collector host; the web source choice cannot set it.

REQUIRED ENVIRONMENT
  KRONIKA_STORAGE_DIR
      No default. Shared data root: active.wal and YYYY/MM/DD/<segment-id>.zms.
      Use the collector's directory, not an individual ZMS or a flat collection
      of segment files. The directory must exist and be writable by web for
      derived .idx sidecars and index ownership locks.
  KRONIKA_WEB_USER
      No default. Nonempty login name, used by browser login and HTTP Basic auth.
  KRONIKA_WEB_PASSWORD
      No default. Nonempty password for that account. Replace example passwords.
      Both credentials remain required even with KRONIKA_WEB_AUTH=disabled.
  KRONIKA_WEB_SOURCES
      No default. Accepted values: 0, 1, 2, 3; meanings and examples above.

OPTIONAL ENVIRONMENT
  KRONIKA_WEB_LISTEN   default 127.0.0.1:8080
      IP address and port, e.g. 127.0.0.1:8080, 0.0.0.0:8080, or [::1]:8080.
      Hostnames are not accepted. The default accepts local connections only.
      The listener serves plain HTTP; use a TLS reverse proxy for remote access.
  KRONIKA_WEB_AUTH     default required; accepted: required, disabled
      required enforces browser sessions and API/MCP authentication. disabled
      permits unauthenticated access; keep that listener on a trusted boundary.
      Setting disabled does not remove the required user/password configuration.
  KRONIKA_WEB_DEMO     unset by default; the only set value is synthetic
      Marks this recording as a synthetic demo in catalog responses. It does
      not generate data or start a collector, workload, or Docker containers.
  TMPDIR              default the system temporary directory (normally /tmp)
      Writable disk for temporary ZMS/HTML files during browser exports. Allow
      space for the selected data and generated HTML. Temporary files are removed
      when closed. Serving an existing recording needs no demo environment.

LOGIN, API, AND MCP
  Browser: http://127.0.0.1:8080/ shows the sign-in form. The configured account
  creates a browser session. API and MCP clients use the same account via HTTP
  Basic authentication; a browser session is also accepted for API requests.

  curl --user kronika http://127.0.0.1:8080/api/catalog

  curl prompts for the configured web password. MCP uses the HTTP endpoint
  http://127.0.0.1:8080/mcp with that account's Basic Authorization header.
  It is an HTTP MCP endpoint, not a stdio command. MCP requests with a browser
  Origin header are rejected. MCP tools read recorded data, not live databases.

LOGS AND STOPPING
  Readiness (ready IP:PORT) goes to stdout; request/connection/export errors and
  export timings go to stderr. There is no web log-level environment setting.
  Ctrl+C or SIGTERM terminates web; the stored recording remains available on
  restart. Invalid configuration or listener failure exits nonzero.
  -h/--help and --version print to stdout and exit 0 before configuration,
  storage access, threads, or a network listener are started.
";
