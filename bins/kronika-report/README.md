# kronika-report

[Русская версия](README.ru.md)

Create one self-contained interactive HTML report from one finished standalone
ZMS. Run the built binary directly; no web process or database is needed:

```sh
./kronika-report incident.zms incident.html
```

`kronika-report --version` prints `kronika-report 1.0.0` and exits
without reading configuration, accessing storage, or starting services.
It needs no root access; pass `--version` as the only argument.

The input may have any `.zms` basename. The command validates it, derives its
internal segment identity from the ZMS catalog, builds the canonical IDX, and
atomically replaces the HTML output. It creates no sidecars and needs no
storage root or earlier segment. A first rate that needs an earlier sample
remains `null`.

The HTML embeds the production React interface, the Rust `kronika-query` engine
compiled to WebAssembly and running in the browser, the ZMS, and
its canonical IDX. Open the file directly in an ordinary browser. Tables,
heatmaps, search, and charts run locally without external assets or network
requests. There is no authentication, MCP, live refresh, or second Export
control in the offline interface.

## Exact report interval

An [already-sliced ZMS](../kronika-dump/README.md#slice) can retain nearby
samples for interval calculations. Limit report navigation to the requested
interval with `--from` and `--to-exclusive`, before the two file paths:

```sh
./kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

This is exactly 2026-09-05 **19:00–20:00 UTC**, a half-open interval in Unix
microseconds. Both bounds must be positive JavaScript-safe integers. Without
them, navigation uses the entire ZMS time range. Nearby samples remain available
for calculations but do not add hours to the report picker.

The web interface's **Export** creates the same kind of HTML directly from the
storage directory and passes the chosen interval automatically.
