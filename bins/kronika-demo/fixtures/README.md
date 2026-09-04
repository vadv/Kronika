# GitHub Pages demo recording

[Русская версия](README.ru.md)

`github-pages-hour.zms` is the fixed production input for the Kronika Pages
report. The repository demo image recorded synthetic Linux, PostgreSQL, and
PgBouncer data under the public hostname `kronika-demo`. The production slicer
then created the standalone ZMS:

```sh
KRONIKA_STORAGE_DIR=CAPTURE kronika-dump slice \
  --from 2026-09-04T12:00:00Z \
  --to 2026-09-04T12:59:59Z \
  --out github-pages-hour.zms
```

The inclusive whole-second endpoints represent the half-open interval
`[12:00:00, 13:00:00)`, exactly 3,600,000,000 microseconds. Production slicing
may retain the nearest snapshot before the requested interval. The source
recording ended before the next hour, so the report opens the requested hour.

The live recorder uses runtime timestamps and counters, so CI keeps this ZMS as
its fixed input. `scripts/build-pages-report.sh` checks its SHA-256, passes this
exact file to `kronika-report`, compares two generated HTML files byte for byte,
and exercises the result directly from disk in Chromium.
