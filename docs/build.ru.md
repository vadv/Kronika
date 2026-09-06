# Сборка из исходников

[English version](build.md) · [Установка готового архива](../INSTALL.ru.md)

Можно установить [готовую сборку](../INSTALL.ru.md) или собрать Kronika из
исходников по инструкции ниже.

Репозиторий фиксирует Rust **1.96.0** и зависимости. Web-интерфейс и движок
отчётов хранятся как готовые артефакты: обычной нативной сборке Node.js не нужен.

## Нативные программы Linux

Установите `rustup`, компилятор C, GNU make и musl toolchain. В Debian/Ubuntu
пакеты сборки — `build-essential`, `musl-tools` и `pkg-config`. Затем:

```sh
git clone https://github.com/vadv/Kronika.git
cd Kronika
# For a review candidate, first: git checkout FULL_COMMIT_FROM_BUILDINFO
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report -p kronika-demo
```

Программы находятся в `target/x86_64-unknown-linux-musl/release/`. Запустите
каждую с `--version`, затем продолжите [установку](../INSTALL.ru.md) с шага
копирования программ. Обычный `cargo build` использует x86-64 musl из
`.cargo/config.toml`. Сборки не задают `target-cpu=native`; сохраняйте базовый
набор инструкций target при передаче результата на другие машины.

На **нативной машине Linux arm64** с её нативным musl toolchain:

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
CC_aarch64_unknown_linux_musl=musl-gcc \
CFLAGS_aarch64_unknown_linux_musl=-mno-outline-atomics \
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report -p kronika-demo
```

Флаг C оставляет атомарные операции в базовом наборе ARMv8, без зависимости
от outline-atomic helpers GCC из glibc toolchain.
[Release workflow](../.github/workflows/release-package.yml) использует ту же
нативную сборку и проверяет настоящий запуск ARM64. Успешная кросс-компиляция
сама по себе не подтверждает работу архива.

Для разработки с GNU toolchain хоста, без переносимого архива:

```sh
make build TARGET=x86_64-unknown-linux-gnu
```

Пример рассчитан на x86-64. Без настройки `make build` выбирает host target
rustc. Бинарники с GNU linkage не являются переносимым релизным архивом.

## Проверить изменения

[Контракт репозитория](../AGENTS.md) определяет ревью и проверки.
Для тестов и линтов под host target:

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
make fmt-check lint test
```

`lint` включает фиксированные правила Dylint репозитория и Mordant, а также
строгий Clippy по workspace. Установка описана в [руководстве линтов](../scripts/check-dylints.sh).
`test` запускает весь набор без BDD. BDD и контейнерная матрица переносимости
работают в CI; для архивов также проверяются реальные распакованные программы.

## Пересобирать браузерные файлы только при их изменении

Установите Node.js **22.21.1** и фиксированный WebAssembly target:

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

Так пересобираются и проверяются сохранённые web HTML, оболочка отчёта и
WebAssembly bindings. Это путь разработки, не условие установки для
пользователя. Движок WebAssembly статического отчёта работает в **главном
потоке браузера**.

## Упаковать ревизию

Закоммитьте нужные исходники и документацию, затем запустите
[`scripts/package-release.sh`](../scripts/package-release.sh) из чистого
checkout. Архив содержит лицензии фиксированных зависимостей, парные
руководства, изображения и редактируемые диаграммы; имя содержит commit,
а файлы имеют контрольные суммы. [Руководство по архивам](releases.ru.md)
описывает нативные target, режим готовых бинарников, детерминированную
упаковку, скачивание артефактов и проверку. Упаковка не создаёт релиз или тег.
