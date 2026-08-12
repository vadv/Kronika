# Captured-hour interface fixture

`real-hour.json.gz` is the data object recovered from the owner-approved
standalone interface capture. It retains the captured OS process rows,
PostgreSQL activity rows, exact PID relationships, one-hour system series and
finding locators. It is used only by interface tests and the opt-in standalone
demo build; `kronika-web` does not ship the captured rows.

The fixture is deterministic unnamed gzip. `real-hour.manifest.json` records
its source and content hashes. `npm run fixture:check` validates structure,
cardinalities, deterministic compression and credential/DSN/authorization/key
signatures without printing command lines or SQL text.

To reproduce the fixture from the preserved owner-approved HTML:

```sh
node scripts/real-fixture.mjs --recover /path/to/index.html
```

To build a temporary, self-contained demo from the same React source:

```sh
node scripts/build.mjs --fixture-output /absolute/path/kronika-real-hour.html.gz
```
