# kronika-dump

[Русская версия](README.ru.md)

Inspect a Kronika **storage directory** or extract an interval into one
standalone ZMS. The storage root must be a real directory; a standalone `.zms`
file or symlink is not a storage root. Run from the built binary's directory,
or use its full path.

`kronika-dump --version` prints `kronika-dump 1.0.0` and exits
without reading configuration, accessing storage, or starting services.
It needs no root access; pass `--version` as the only argument.

## Inspect

```sh
./kronika-dump /var/lib/kronika
./kronika-dump /var/lib/kronika --json
./kronika-dump /var/lib/kronika --index
./kronika-dump /var/lib/kronika --section 1100001 --limit 10
```

The default output lists each segment and its section sizes. `--index` shows
typed identities and summaries; `--section` shows decoded rows for a physical
`type_id` with dictionary references resolved. The default row limit is 20 per
segment; `--limit 0` prints all rows and requires `--section`. `--json` selects
machine-readable output. Inspection `--from` and `--to` accept inclusive Unix
microseconds. Use `sudo` when the storage directory requires it.

## Slice

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika ./kronika-dump slice \
  --from 2026-09-05T19:00:00Z \
  --to 2026-09-05T19:59:59Z \
  --out incident.zms
```

Slice accepts inclusive whole seconds in RFC 3339 with a timezone. It reads
finished segments and the current journal, and writes the exact new `.zms`
path. Existing output files and ranges with no recorded rows are errors.
The output can retain up to 30 seconds of nearby samples on either side for
interval calculations. The command validates the new ZMS and prints requested
and actual bounds, rows, sections, and bytes. Temporary files use the output
filesystem.

Feed `incident.zms` to [kronika-report](../kronika-report/README.md) to create
an offline HTML report. If slice ran under `sudo`, give your user ownership of
the output before reading it without privileges:

```sh
sudo chown "$(id -u):$(id -g)" incident.zms
```
