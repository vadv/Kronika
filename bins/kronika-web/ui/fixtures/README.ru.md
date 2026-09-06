# Fixture интерфейса за записанный час

[English version](README.md)

`real-hour.json.gz` — data object из автономного HTML-снимка интерфейса.
Он содержит записанные строки OS processes и PostgreSQL Activity, связи по PID,
часовые системные ряды и locators отметок. Fixture используется в тестах интерфейса
и отдельно включаемой автономной demo-сборке; строки fixture не входят в `kronika-web`.

Файл использует deterministic unnamed gzip. `real-hour.manifest.json` содержит
source и content hashes. `npm run fixture:check` проверяет структуру, количество
записей, deterministic compression и сигнатуры credentials/DSN/authorization/keys,
не выводя command lines или SQL text.

Восстановление fixture из сохранённого HTML:

```sh
node scripts/real-fixture.mjs --recover /path/to/index.html
```

Сборка временного автономного demo из тех же React sources:

```sh
node scripts/build.mjs --fixture-output /absolute/path/kronika-real-hour.html.gz
```
