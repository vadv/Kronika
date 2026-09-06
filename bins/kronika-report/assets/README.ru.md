# Генерируемые ресурсы отчёта

[English version](README.md)

UI shell собирается из production React sources в `bins/kronika-web/ui`.
JavaScript bindings и сжатый WebAssembly собираются из `crates/kronika-report-wasm`
версиями Rust и wasm-bindgen, зафиксированными в репозитории. Проверка воспроизводимости
сравнивает полученные файлы побайтно с файлами репозитория.

Команда `scripts/report-assets.sh build` использует `WASM_BINDGEN` — путь к
исполняемому файлу `wasm-bindgen 0.2.127`. Параметр `--download-bindgen` загружает
зафиксированный static x86_64 Linux musl release и проверяет SHA-256 перед запуском.
`scripts/report-assets.sh check` сравнивает новую сборку с сохранёнными JavaScript
и deterministic gzip. `CARGO_BIN` и `NODE_BIN` задают пути к Cargo и Node вместо
поиска в `PATH`. Сборка фиксирует remap путей репозитория и Cargo home, seed
`const-random` и идентификатор C compiler для воспроизводимости между хостами.

wasm-bindgen создаёт target `web`. Скрипт добавляет ограниченную точку входа
`initEmbedded` по фиксированным маркерам сгенерированного кода; esbuild сохраняет
`initEmbedded` и `ReportSession` в classic-script global `KronikaReportWasm`.
Сохранённый binding не содержит URL или сетевого загрузчика. Отчёт компилирует
встроенные байты и передаёт `WebAssembly.Module` в `initEmbedded` для асинхронного
создания экземпляра.

Размер WebAssembly — 9 910 988 байт, gzip — 2 395 947 байт, SHA-256 gzip:
`36100b0739d1dd73d373f61f8ab557592b7f2871284037bd4dfc628d5b22dd04`.
Размер JavaScript binding — 3 885 байт, SHA-256:
`4635ae734e8c1e1aeb463ae1096f4fdc2a65d98e715b55cee9fe46956f29cba8`.

Сгенерированный binding один раз копирует каждый входной `Uint8Array` в linear
memory WebAssembly. Rust принимает эти allocations как `Vec<u8>` и передаёт их
в сохраняемый `ReportEngine` без дополнительной полной копии ZMS или IDX.
NDJSON собирается из потока записей и один раз копируется из WebAssembly в JavaScript.
