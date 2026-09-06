# Сборка из исходников

[English version](build.md) · [Установка](../INSTALL.ru.md)

Можно установить [готовую сборку](../INSTALL.ru.md) или собрать Kronika из исходников по инструкции ниже.

## Toolchain

| Компонент | Версия или требование |
| --- | --- |
| Rust | `1.96.0`, закреплён в [rust-toolchain.toml](../rust-toolchain.toml). Зависимости используют `Cargo.lock`. |
| Native tools | C compiler, GNU make, `pkg-config`; статическая сборка использует native `musl-gcc`. Пакеты Debian/Ubuntu: `build-essential musl-tools pkg-config`. |
| Browser assets | Native Cargo builds используют сохранённые в репозитории HTML и WebAssembly assets. |
| Пересборка assets | Node.js `22.21.1`, `wasm32-unknown-unknown`, закреплённые npm dependencies. |

## Native binaries для Linux

На native x86-64 Linux с установленными rustup и native tools:

```sh
git clone https://github.com/vadv/Kronika.git
cd Kronika
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Результат: `target/x86_64-unknown-linux-musl/release/kronika-{collector,web,dump,report}`.
Cargo target по умолчанию в [.cargo/config.toml](../.cargo/config.toml) —
`x86_64-unknown-linux-musl`. Для сборки точной ревизии выполните
`git checkout FULL_COMMIT` перед `cargo build`.

На native ARM64 Linux:

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
CC_aarch64_unknown_linux_musl=musl-gcc \
CFLAGS_aarch64_unknown_linux_musl=-mno-outline-atomics \
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Результат: `target/aarch64-unknown-linux-musl/release/`. C flag создаёт inline
ARMv8 atomic operations. Раздел [установки](../INSTALL.ru.md#2-установка)
содержит команды установки binaries.

Сборка всех workspace binaries, включая инструменты разработки, с host GNU
toolchain на x86-64:

```sh
make build TARGET=x86_64-unknown-linux-gnu
```

`make build` по умолчанию использует host target rustc.

## Проверки

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
make fmt-check lint test
```

`lint` запускает repository/Mordant Dylint rules и workspace Clippy с запретом
warnings. [Настройка Dylint](../scripts/check-dylints.sh) определяет зависимости.
`test` запускает unit и integration tests без BDD. BDD выполняется в CI.
[Контракт агента](../AGENTS.md) определяет требования review.

## Browser assets

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

Эти targets собирают и воспроизводят сохранённые web HTML, report shell и
WebAssembly bindings. Query engine отчёта выполняется на основном потоке
браузера. Определения targets: [Makefile](../Makefile).

## Архив

В чистом checkout с сохранёнными commits:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

[Справочник архивов](releases.ru.md) определяет targets, имена результатов,
build metadata, checksums, вложенные документы и проверки CI.
