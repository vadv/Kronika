# GitHub Pages demo recording

[Русская версия](README.ru.md)

`github-pages-hour.zms` is the fixed production input for the Kronika Pages
report. It contains 20 minutes of synthetic Linux, PostgreSQL, and PgBouncer
data under the public hostname `kronika-demo`: short concurrent commerce
transactions, lock waits, query plans, and bounded CPU, memory, disk, and
network activity. The production slicer created the standalone ZMS:

```sh
KRONIKA_STORAGE_DIR=CAPTURE kronika-dump slice \
  --from 2026-09-05T15:32:00Z \
  --to 2026-09-05T15:51:59Z \
  --out github-pages-hour.zms
```

The inclusive whole-second endpoints represent the half-open interval
`[15:32:00, 15:52:00)`, exactly 1,200,000,000 microseconds. Production slicing
may retain nearby snapshots outside the requested interval. The report uses
the explicit bounds in `github-pages-hour.slice`; its timeline shows the
15:00–16:00 calendar hour.

The live recorder uses runtime timestamps and counters, so CI keeps this ZMS as
its fixed input. `scripts/build-pages-report.sh` checks its SHA-256, passes this
exact file and range to `kronika-report`, checks the embedded range, compares
two generated HTML files byte for byte, and exercises the result directly from
disk in Chromium.
