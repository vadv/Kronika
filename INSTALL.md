# Install and record your own Linux host

[Русская версия](INSTALL.ru.md) · [README](README.md)

Kronika needs two processes: **collector** records this machine continuously;
**web** opens its history when you need it. Start with Linux alone. PostgreSQL
is an optional data source, not Kronika's storage engine.

The portable archive contains four static Linux programs: `kronika-collector`,
`kronika-web`, `kronika-dump`, and `kronika-report`. No Rust, Node.js, Docker,
or database is needed to run them.

## 1. Download the right archive

**The published v1.0.0 is older than this guide.** It has no `--version`,
`kronika-report`, `kronika-dump slice`, or browser Export. Until a new release
is published, download a **Release package** workflow artifact from the change
being reviewed. The [release guide](docs/releases.md) gives the download path,
architecture matrix, and exact checks. GitHub requires a signed-in account to
download Actions artifacts.

Choose `x86_64-unknown-linux-musl` for `uname -m` = `x86_64`, or the separately
built `aarch64-unknown-linux-musl` candidate for `aarch64`. Use a successful
workflow run for that architecture. These are Linux executables; the archive
does not install a kernel or add support for Windows or macOS.

Keep the `.tar.gz` and its matching `.tar.gz.sha256` together in a new directory.
Use the **exact filename you downloaded** below. A checksum checks that the
bytes match the selected artifact; obtain both files from the same trusted run.

```sh
archive='kronika-1.0.0-REPLACE_WITH_COMMIT-x86_64-unknown-linux-musl.tar.gz'
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
cat BUILDINFO
```

`BUILDINFO` records the packaging revision and build mode. Candidate filenames
include a commit because the source version remains `1.0.0`; version output
alone does not distinguish this candidate from a different build of that source
version. Keep `BUILDINFO` with the archive when reporting a problem.

## 2. Verify and install the binaries

Run from the extracted directory, without `sudo` or any configuration:

```sh
for binary in kronika-collector kronika-web kronika-dump kronika-report; do
  "./$binary" --version
done
```

Each line is the program name followed by `1.0.0`, for example
`kronika-collector 1.0.0`. These commands exit immediately without starting
collection or a listener.

Install the four programs on your PATH:

```sh
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
/usr/local/bin/kronika-collector --version
```

You can instead keep them in the extracted directory and use absolute paths.
There is no installer script to fetch or pipe into a shell.

## 3. Record Linux

On the host you want to examine:

```sh
sudo install -d -m 0700 /var/lib/kronika
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Leave this terminal running. Privileged access lets the collector read process
I/O and protected local logs. Its files are private; use the same privileged
account for web and dump rather than making the recording world-readable.
The path is explicit and must be a real directory, not a symlink.

Core metrics arrive about every 10 seconds, process snapshots every 5 seconds;
slower inventories have their own intervals. Collection begins on startup.
The active journal is already readable by web: there is no need to wait for the
default 15-minute segment write. `Ctrl+C` stops collection and preserves its
journal; rerunning the same command resumes from that directory.

The default retention target is **2 GiB** for journals, finished segments, and
indexes. For a fixed 10 GiB target, add `KRONIKA_RETENTION=10737418240` to the
collector command. Read the [storage rules](bins/kronika-collector/README.md#storage)
before choosing a filesystem percentage target or estimating retained days.

## 4. Open the recording

In another terminal, replace the example password and run:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Open **<http://127.0.0.1:8080/>**, sign in, choose the recorded hour, and open
Processes or Host. The [operator guide](docs/operator-guide.md) walks through
finding a busy interval, selecting an object, and following its history.

Web needs write access to the same directory for derived indexes and a lock.
It binds to loopback and keeps authentication enabled. To inspect a remote
host from your workstation, forward that loopback port over SSH:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@monitored-host
```

Then open the same localhost URL on your workstation. If port 8080 there is
occupied, use `-L 8081:127.0.0.1:8080` and open `http://127.0.0.1:8081/`.

MCP is already served by this web process at **`http://127.0.0.1:8080/mcp`**,
with the same credentials. Open the **AI** panel for client setup or use the
[MCP client guide](docs/mcp-clients.md). MCP retrieves recorded data; it does not
connect to the running PostgreSQL server or inspect the current host itself.

For continuous startup and private configuration files, use the
[systemd guide](docs/services.md). Both foreground commands above remain useful
for a first run or a temporary investigation.

## 5. Add PostgreSQL when needed

On the PostgreSQL server, create a dedicated monitoring role. In `psql` as an
administrator, the password prompt avoids storing the password in the SQL text:

```sql
CREATE ROLE kronika_monitor LOGIN;
\password kronika_monitor
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
```

Connect directly to PostgreSQL or through **session pooling**. Metric collection
uses session settings and does not support PgBouncer transaction or statement
pooling. The base grants, database-local extension permissions, and hardened
cluster exceptions are in the [collector guide](bins/kronika-collector/README.md#postgresql-role).
There is no grant on application tables and no superuser requirement for this role.

Stop the foreground collector with `Ctrl+C`, then restart it with the same
storage path and your DSN. This example uses PostgreSQL on the same host:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

`KRONIKA_PG_DSNS` enables PostgreSQL metrics itself. The first DSN supplies
metrics from that server's connectable databases; additional DSNs only discover
logs. No DSN per database is needed. Installed supported `pg_stat_statements`
and `pg_store_plans` interfaces are discovered automatically; installing those
extensions and changing PostgreSQL preload settings are separate DBA operations.
Activity, Locks, database statistics, and relation inventories do not require
those extensions.

If you know the monitored PostgreSQL server's effective CPU capacity, also set
`KRONIKA_POSTGRES_EFFECTIVE_CPUS` to that positive whole number. Kronika uses it for
PostgreSQL health and never guesses it from the collector's CPU count. Without
it, PostgreSQL metrics remain available but the PostgreSQL health value is null.

Restart web with the same command as above, changing **`KRONIKA_WEB_SOURCES=1`
to `KRONIKA_WEB_SOURCES=3`**. This declares OS and PostgreSQL as configured;
it neither starts collection nor filters saved data.

Log discovery returns paths that must be readable **on the collector host**.
For mounted remote logs, use `KRONIKA_PG_LOGS`; for PgBouncer logs use
`KRONIKA_PGBOUNCER_DSNS` or `KRONIKA_PGBOUNCER_LOGS`. The collector guide lists
the permissions, supported log formats, and collection intervals. The native
PostgreSQL client currently uses `NoTls`; use a local connection or a protected
transport rather than expecting a DSN to enable TLS.

For services, keep DSNs and web credentials in the root-readable environment
files from the [systemd guide](docs/services.md), instead of command history.

## Next steps

- [Investigate an hour](docs/operator-guide.md): cursor, charts, processes,
  backend waits, SQL, plans, tables, and events.
- [All views and controls](docs/features.md): dimensions, lenses, and formulas.
- **Export** in web saves an interval as one interactive offline HTML file.
  [Dump](bins/kronika-dump/README.md) and [report](bins/kronika-report/README.md)
  also work with saved recordings from the command line.
- [Build from source](docs/build.md) for development or a custom build.
- [Demo](bins/kronika-demo/README.md) for an optional synthetic environment.
