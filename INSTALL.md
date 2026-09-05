# Install the Kronika archive

[Русская версия](INSTALL.ru.md)

This package contains static x86-64 Linux executables: `kronika-collector`,
`kronika-web`, `kronika-dump`, and `kronika-report`. Some packages also include
`kronika-demo`, the workload and collection runner. `BUILDINFO` identifies the
packaging source revision and build mode. `SHA256SUMS` checks the binaries.
The executables sit directly in the extracted directory; run the commands
below there. No Node.js or database is required for Kronika's own storage.

## Verify and unpack

Place the archive and its matching `.tar.gz.sha256` in the same directory.
Set the exact received filename, then verify before extracting:

```sh
archive='kronika-1.0.0-COMMIT-x86_64-unknown-linux-musl.tar.gz'
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
```

The commit-qualified filename identifies a prepared artifact, not a published
version tag. The original published v1.0.0 archive predates `kronika-report`
and HTML export; these instructions describe the current package.

## Record Linux and PostgreSQL

Create a PostgreSQL role on the monitored server; replace the sample password:

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE kronika_monitor LOGIN PASSWORD 'replace-with-password';
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
SQL
```

Start the collector on the monitored host:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  ./kronika-collector
```

Omit `KRONIKA_PG_DSNS` for Linux only. PostgreSQL metrics need no other enable
switch. The first DSN supplies metrics across its connectable databases;
`pg_stat_statements` and `pg_store_plans` are collected when a supported
installation is available. Log discovery names files that must be readable
locally. The default retention target is 2 GiB, including the current journal,
finished recordings, and derived indexes. `KRONIKA_RETENTION` accepts a decimal
byte budget or `auto:P`, the backing filesystem's target used-space percentage.
`Ctrl+C` stops collection; a later start resumes from the journal.

In another terminal, from this package directory:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_SOURCES=3 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  ./kronika-web
```

Open <http://127.0.0.1:8080/> and sign in with these credentials. The default
listener is loopback. `KRONIKA_WEB_SOURCES=3` declares both source families;
use `1` for Linux only. It does not enable collection or filter stored data.
Web needs write access to the same storage directory to create indexes.
MCP is available at `http://127.0.0.1:8080/mcp` with the same authentication;
use the web interface's MCP panel for client configuration.

## Inspect and export

The storage root must be a real directory, not a file or symlink:

```sh
sudo ./kronika-dump /var/lib/kronika
```

Choose a recorded interval; the two slice endpoints are inclusive RFC 3339
whole seconds. The command refuses an existing output file.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika ./kronika-dump slice \
  --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z --out incident.zms
sudo chown "$(id -u):$(id -g)" incident.zms
./kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

Report bounds use Unix microseconds: these select exactly 19:00–20:00 UTC,
keeping the slice's nearby calculation samples outside navigation. Without
bounds, report exposes the entire ZMS interval. It atomically replaces an
existing HTML output. The resulting HTML opens directly in a browser without
a server, network, or sidecar files. Web's Export action creates the same kind
of report. Static reports have no MCP or live refresh.

Full source documentation is in the repository's README, collector and web
guides, and `docs/mcp-clients.md` at the revision named in `BUILDINFO`.
