//! Operational help, printed before configuration or server startup.

pub(crate) const HELP: &str = r"kronika-web - browse a Kronika recording and serve its HTTP API and MCP tools

Usage: kronika-web
       kronika-web --help | -h | --version

Runs in the foreground. Configuration is environment-only. Required variables:
KRONIKA_STORAGE_DIR, KRONIKA_WEB_USER, KRONIKA_WEB_PASSWORD, KRONIKA_WEB_SOURCES.
The default address is 127.0.0.1:8080; sign in at http://127.0.0.1:8080/.

EXAMPLE
  Run over an existing recording with your configured web credentials:

  sudo env KRONIKA_STORAGE_DIR=/path/to/recording KRONIKA_WEB_SOURCES=1 \
    KRONIKA_WEB_USER=kronika KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
    kronika-web

  Use KRONIKA_WEB_SOURCES=3 to declare Linux and PostgreSQL sources.
  The process needs read/write access to the recording directory.

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
  KRONIKA_PG_DSNS on kronika-collector enables PostgreSQL metric collection.

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

  MCP uses http://127.0.0.1:8080/mcp with the same HTTP Basic credentials.
  This is an HTTP endpoint, not a stdio command.

LOGS AND STOPPING
  Readiness (ready IP:PORT) goes to stdout; request/connection/export errors and
  export timings go to stderr. There is no web log-level environment setting.
  Ctrl+C or SIGTERM terminates web; the stored recording remains available on
  restart. Invalid configuration or listener failure exits nonzero.
  -h/--help and --version print to stdout and exit 0 before configuration,
  storage access, threads, or a network listener are started.
";
