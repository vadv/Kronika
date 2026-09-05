# Build from source

[Русская версия](build.ru.md) · [Install a binary archive](../INSTALL.md)

Use a portable archive to record a machine. Source builds are useful for
working on Kronika or compiling a particular revision. The repository pins
Rust **1.96.0** and locks its dependencies. The web interface and report engine
are committed build assets: a normal native build needs no Node.js.

## Native Linux binaries

Install `rustup`, a C compiler, GNU make, and the musl C toolchain. On
Debian/Ubuntu the build packages are `build-essential`, `musl-tools`, and
`pkg-config`. Then:

```sh
git clone https://github.com/vadv/Kronika.git
cd Kronika
# For a review candidate, first: git checkout FULL_COMMIT_FROM_BUILDINFO
rustup target add x86_64-unknown-linux-musl
cargo build --release --locked --target x86_64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report -p kronika-demo
```

The binaries are in `target/x86_64-unknown-linux-musl/release/`. Run each with
`--version`, then follow [installation](../INSTALL.md) from the binary install
step. An ordinary `cargo build` defaults to the x86-64 musl target in
`.cargo/config.toml`. No build uses `target-cpu=native`; keep the target's
baseline instruction set when distributing the result to other machines.

On a **native arm64 Linux builder** with its native musl toolchain:

```sh
rustup target add aarch64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
CC_aarch64_unknown_linux_musl=musl-gcc \
CFLAGS_aarch64_unknown_linux_musl=-mno-outline-atomics \
cargo build --release --locked --target aarch64-unknown-linux-musl \
  -p kronika-collector -p kronika-web -p kronika-dump \
  -p kronika-report -p kronika-demo
```

The C flag keeps atomic operations in the baseline ARMv8 instruction set,
without GCC outline-atomic helper dependencies from a glibc toolchain.
The [release workflow](../.github/workflows/release-package.yml) uses the same
native build and validates actual ARM64 execution. A successful
cross-compilation alone does not establish that an archive runs.

To work with the host GNU toolchain instead of making a portable archive:

```sh
make build TARGET=x86_64-unknown-linux-gnu
```

This example is for an x86-64 host. `make build` otherwise selects rustc's host
target. GNU-linked development binaries are not the portable release artifact.

## Verify changes

The repository's [agent contract](../AGENTS.md) defines review and checks.
For host-target tests and linting:

```sh
export CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu
make fmt-check lint test
```

`lint` includes the pinned repository and Mordant Dylint rules as well as
strict workspace Clippy. See [lint tooling](../scripts/check-dylints.sh) for setup.
`test` runs the complete non-BDD suite. BDD and the container portability matrix
run in CI; portable archives also get real extracted-program checks.

## Rebuild browser assets only when changing them

Install Node.js **22.21.1** and the pinned WebAssembly target:

```sh
rustup target add wasm32-unknown-unknown --toolchain 1.96.0
make ui-install
make report-assets REPORT_ASSET_FLAGS=--download-bindgen
make report-assets-check REPORT_ASSET_FLAGS=--download-bindgen
```

This rebuilds and checks the committed web HTML, report shell, and WebAssembly
bindings. It is a development path, not an end-user installation prerequisite.
The static report runs its WebAssembly engine on the browser's **main thread**.

## Package a revision

Commit the intended source and documentation, then use
[`scripts/package-release.sh`](../scripts/package-release.sh) from that clean
checkout. It includes the locked dependency notices, bilingual guides, images,
and editable diagrams, with a commit-qualified filename and member checksums.
The [release guide](releases.md) explains native targets, prebuilt mode,
deterministic packaging, artifact downloads, and validation. Packaging does not
create a release or tag.
