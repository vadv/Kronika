# Release archives

[Русская версия](releases.ru.md) · [README](../README.md)

## Current published archive

[v1.0.0](https://github.com/vadv/Kronika/releases/tag/v1.0.0) is a static
x86-64 Linux musl archive. It contains `kronika-collector`, `kronika-web`,
`kronika-dump`, and `kronika-demo` directly inside its top-level directory.
**It predates `kronika-dump slice`, `kronika-report`, and HTML export.** To
use the current features, follow the
[source build](../README.md#build-the-current-binaries).

These URLs download that existing release, not a future package:

```sh
curl --fail --location --remote-name \
  https://github.com/vadv/Kronika/releases/download/v1.0.0/kronika-1.0.0-x86_64-unknown-linux-musl.tar.gz
curl --fail --location --remote-name \
  https://github.com/vadv/Kronika/releases/download/v1.0.0/SHA256SUMS
sha256sum --check SHA256SUMS
tar -xzf kronika-1.0.0-x86_64-unknown-linux-musl.tar.gz
cd kronika-1.0.0-x86_64-unknown-linux-musl
```

Use the instructions bundled in that archive for that version. There is no
published arm64 archive. The Docker demo builds from source on amd64 or arm64.

## Prepare the current package

Run from a clean Linux x86-64 checkout with the pinned Rust toolchain, musl target,
`musl-gcc`, GNU tar, gzip, binutils, and Python 3.11 or newer:

```sh
scripts/package-release.sh
```

The script builds the locked release versions of **collector, web, dump, and
report**, checks that each is a static x86-64 ELF executable, and creates:

```text
dist/kronika-<cargo-version>-<12-character-commit>-x86_64-unknown-linux-musl.tar.gz
dist/kronika-<cargo-version>-<12-character-commit>-x86_64-unknown-linux-musl.tar.gz.sha256
```

Add `--with-demo` to include `kronika-demo`, as the CI package does. The
executables remain directly inside one top-level directory alongside `LICENSE`,
`INSTALL.md`, `INSTALL.ru.md`, binary `SHA256SUMS`, `BUILDINFO`, and the embedded
font licenses. Installation instructions are self-contained and use relative
binary paths. No development checkout, workload recording, or machine metadata
is copied into the archive.

To package binaries already built from this checkout:

```sh
scripts/package-release.sh \
  --bin-dir target/x86_64-unknown-linux-musl/release \
  --output-dir dist
```

`--bin-dir` skips compilation; the caller supplies binaries built from the
intended source revision. `BUILDINFO` records `build_mode=prebuilt` and the
packaging checkout's commit, not an inferred binary revision. By default the
script builds from source and records `build_mode=source`.

Packaging fixes archive order, modes, owner/group, timestamps (the commit's
Unix time), and gzip metadata. Identical binaries, installation documents,
license files, revision, and build mode yield an identical archive. This does
not promise that independent compiler builds produce identical binaries.
Existing archive or checksum paths are refused; use a fresh output directory
for another packaging run. The script refuses uncommitted checkout changes and changes no version, tag, or release.

## Validate a prepared archive

With Node.js 22 and Chromium or Google Chrome available:

```sh
scripts/check-release.sh dist/*.tar.gz
```

Pass exactly one archive. The check verifies the archive and binary checksums,
extracts it into a temporary directory, checks collector configuration failure,
and runs dump inspection, a time slice, deterministic report generation, an
authenticated web catalog, and MCP tool discovery. The existing report browser
smoke opens the generated HTML directly from disk and checks interactive data
reads and absence of external requests. All temporary files and the local web
process are removed on exit. No PostgreSQL server or local BDD run is needed.

The [Release package workflow](../.github/workflows/release-package.yml) runs
on pull requests and manual dispatch. It builds all five executables, performs
these checks, verifies repeated packaging byte-for-byte, and uploads the archive
and checksum as a GitHub Actions artifact retained for 14 days. It has only
`contents: read` permission and contains no tag creation or release publication.
The Cargo version remains unchanged; commit-qualified artifacts are for review.
