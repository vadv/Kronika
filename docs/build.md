# Build from source

[Русская версия](build.ru.md) · [Install](../INSTALL.md)

You can install a [prebuilt binary archive](../INSTALL.md) or build Kronika
from source using the instructions below.

## Toolchain

| Component | Version or requirement |
| --- | --- |
| Rust | `1.96.0`, pinned by [rust-toolchain.toml](../rust-toolchain.toml). Dependencies use `Cargo.lock`. |
| Native tools | C compiler, GNU make, `pkg-config`; static builds use native `musl-gcc`. Debian/Ubuntu packages: `build-essential musl-tools pkg-config`. |
| Browser assets | Committed HTML and WebAssembly assets are used by native Cargo builds. |
| Asset rebuild | Node.js `22.21.1`, `wasm32-unknown-unknown`, locked npm dependencies. |

## Native Linux binaries

On native x86-64 Linux with rustup and the native tools installed:

```sh
git clone https://github.com/vadv/Kronika.git
cd Kronika
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Outputs: `target/x86_64-unknown-linux-musl/release/kronika-{collector,web,dump,report}`.
The default Cargo target in [.cargo/config.toml](../.cargo/config.toml) is
`x86_64-unknown-linux-musl`. To build an exact revision, run
`git checkout FULL_COMMIT` before `cargo build`.

On native ARM64 Linux:

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
CC_aarch64_unknown_linux_musl=musl-gcc \
CFLAGS_aarch64_unknown_linux_musl=-mno-outline-atomics \
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report
```

Outputs: `target/aarch64-unknown-linux-musl/release/`. The C flag emits inline
ARMv8 atomic operations. [Install](../INSTALL.md#2-install) provides the binary
installation commands.

To build every workspace binary, including development tooling, with the host
GNU toolchain on x86-64:

```sh
make build TARGET=x86_64-unknown-linux-gnu
```

`make build` defaults to rustc's host target.

## Checks

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
make fmt-check lint test
```

`lint` runs repository/Mordant Dylint rules and workspace Clippy with warnings
denied. [Dylint setup](../scripts/check-dylints.sh) defines its dependencies.
`test` runs non-BDD unit and integration tests. BDD runs in CI.
The [agent contract](../AGENTS.md) defines review requirements.

## Browser assets

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

These targets build and reproduce the committed web HTML, report shell and
WebAssembly bindings. The report's query engine executes on the browser's main
thread. Target definitions: [Makefile](../Makefile).

## Archive

From a clean committed checkout:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

[Archive reference](releases.md) defines targets, output names, build metadata,
checksums, packaged documents and CI checks.
