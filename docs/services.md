# Keep the collector running with systemd

[Русская версия](services.ru.md) · [Install](../INSTALL.md)

Use these units after the foreground installation works. They run the installed
programs directly, with private storage and a loopback web listener. Run one
collector and one web process for a storage root. Stop the foreground processes
before starting their services.

## Private configuration

Create the directories and files, then edit them:

```sh
sudo install -d -m 0700 /etc/kronika /var/lib/kronika
sudo touch /etc/kronika/collector.env /etc/kronika/web.env
sudo chmod 0600 /etc/kronika/collector.env /etc/kronika/web.env
sudoedit /etc/kronika/collector.env /etc/kronika/web.env
```

`/etc/kronika/collector.env` for Linux only:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_RETENTION=2147483648
```

`/etc/kronika/web.env` — replace the password:

```ini
KRONIKA_STORAGE_DIR=/var/lib/kronika
KRONIKA_WEB_LISTEN=127.0.0.1:8080
KRONIKA_WEB_SOURCES=1
KRONIKA_WEB_USER=kronika
KRONIKA_WEB_PASSWORD=replace-with-a-random-password
```

These are systemd environment files, not shell scripts: do not add `export` or
shell substitutions. Quote a complete value when it contains spaces. To add
PostgreSQL after creating its monitoring role, append to `collector.env`:

```ini
KRONIKA_PG_DSNS="host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres"
```

Set `KRONIKA_POSTGRES_EFFECTIVE_CPUS` only to the known positive whole-number CPU capacity
of that PostgreSQL server. Change web's sources value to `3`, then restart both
services. Other variables are listed in the [collector](../bins/kronika-collector/README.md)
and [web](../bins/kronika-web/README.md) references.

## Units

Create `/etc/systemd/system/kronika-collector.service` with `sudoedit`:

```ini
[Unit]
Description=Kronika machine history collector
After=network.target

[Service]
Type=simple
User=root
UMask=0077
EnvironmentFile=/etc/kronika/collector.env
ExecStart=/usr/local/bin/kronika-collector
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

Create `/etc/systemd/system/kronika-web.service`:

```ini
[Unit]
Description=Kronika history web and MCP
After=network.target

[Service]
Type=simple
User=root
UMask=0077
EnvironmentFile=/etc/kronika/web.env
ExecStart=/usr/local/bin/kronika-web
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
```

The collector needs privileged process and log access. This simple setup keeps
web under the same account because collector files are private and web writes
indexes alongside them. It does not grant other users access to the directory.
Web can read a stopped collector's recording, so its unit has no dependency on
a running collector. Export uses temporary disk space; a restricted deployment
can set a writable `TMPDIR` as described in the web guide.

Validate and start:

```sh
sudo systemd-analyze verify /etc/systemd/system/kronika-collector.service \
  /etc/systemd/system/kronika-web.service
sudo systemctl daemon-reload
sudo systemctl enable --now kronika-collector.service kronika-web.service
sudo systemctl status kronika-collector.service kronika-web.service
sudo journalctl -u kronika-collector -u kronika-web --since '5 minutes ago'
```

Open <http://127.0.0.1:8080/>. For another machine, use the SSH forwarding
command in [Install](../INSTALL.md). The `/mcp` endpoint uses the same listener
and authentication; no third unit is needed.

## Everyday operations

| Need | Command / action |
| --- | --- |
| Follow collector errors and write cost | `sudo journalctl -u kronika-collector -f` |
| Follow web startup or request errors | `sudo journalctl -u kronika-web -f` |
| Take a collection window now | `sudo systemctl kill --kill-whom=main --signal=SIGUSR2 kronika-collector` |
| Apply edited environment files | `sudo systemctl restart kronika-collector kronika-web` |
| Stop collecting; retain files | `sudo systemctl stop kronika-collector` |
| Open web only when needed | `sudo systemctl disable --now kronika-web`; later `sudo systemctl start kronika-web` |
| Inspect disk consumption | `sudo du -sh /var/lib/kronika` |

## Replace binaries or remove services

Verify the new archive, its `BUILDINFO`, and all four `--version` outputs first.
Stop both services, keep the old binaries and archive, install the new binaries
using [Install](../INSTALL.md), and start the services again. Configuration and
recordings stay in their existing directories. Do not run two collectors over
one directory. Storage compatibility follows the release's documented format
contract; retaining an old executable alone does not guarantee downgrade support.

To uninstall the services, disable and stop them, remove only the two unit
files you installed, and run `systemctl daemon-reload`. Remove the four binaries
from `/usr/local/bin` only if they are no longer used. Configuration and
recordings are separate: removing programs never requires deleting recorded data.

## When the first run fails

| Observed result | Check |
| --- | --- |
| `Exec format error` | Match `uname -m` to the archive target; verify its checksum. |
| `Permission denied` launching a binary | Executable mode and a filesystem mounted with execution allowed. |
| Collector refuses storage | Exact `KRONIKA_STORAGE_DIR`, a real directory, privileges, and the existing writer process. |
| Web refuses configuration | All four required values: storage, sources, user, password. |
| Web cannot create an index | The account needs write access to the same private recording directory. |
| No PostgreSQL rows | Collector log, first DSN, `pg_monitor`, database `CONNECT`, and extension-local permissions. |
| No PostgreSQL health number | Positive `KRONIKA_POSTGRES_EFFECTIVE_CPUS` and recorded Activity data. |
| A resource is a dash | Open field help and inspect the collector's logged read error; absent fields remain null. |

`--version` never opens storage and remains available even when ordinary startup
configuration is missing or invalid.
