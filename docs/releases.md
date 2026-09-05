# Portable Linux archives

[Русская версия](releases.ru.md) · [Install and run](../INSTALL.md) · [README](../README.md)

## Choose the artifact you mean to install

Current review archives contain all five programs: `kronika-collector`,
`kronika-web`, `kronika-dump`, `kronika-report`, and `kronika-demo`. Start with
[installation on your own host](../INSTALL.md); the demo is an optional workload.
No compiler, Docker, PostgreSQL server, or separate web asset directory is needed
for an OS-only installation from an archive.

The existing public [v1.0.0 release](https://github.com/vadv/Kronika/releases/tag/v1.0.0)
is older. It has collector, web, dump, and demo, but **no `--version`,
`kronika-dump slice`, `kronika-report`, or HTML export**. Its bundled instructions
describe that version. The current workspace version is still `1.0.0`; a
commit-qualified review archive is a separate artifact, not a replacement of
that release. No new public release or tag is created by the packaging workflow.

The [Release package workflow](../.github/workflows/release-package.yml) uploads
archives and their checksums as Actions artifacts retained for 14 days. Select a
successful run for the commit you intend to review; its artifact name contains
the full source commit and target. [INSTALL](../INSTALL.md) gives the download,
checksum, extraction, and launch commands. Actions downloads require GitHub
access; they are review artifacts, not permanent public release links.

### Download an Actions candidate

With the [GitHub CLI](https://cli.github.com/manual/gh_run_download) authenticated,
list successful packaging runs and choose the run for the change being reviewed:

```sh
gh run list --repo vadv/Kronika --workflow release-package.yml --status success --limit 10
run_id=REPLACE_WITH_RUN_ID
gh run view "$run_id" --repo vadv/Kronika
source_revision=$(gh run view "$run_id" --repo vadv/Kronika --json headSha --jq .headSha)
target=x86_64-unknown-linux-musl
gh run download "$run_id" --repo vadv/Kronika \
  --name "kronika-$source_revision-$target" --dir kronika-download
cd kronika-download
```

Use `target=aarch64-unknown-linux-musl` for ARM64. Select a run that passed
**both build jobs and every portability row**, not a job that only uploaded an
archive. The run page also permits downloading the matching artifact ZIP in
the browser; unpack that ZIP to get the `.tar.gz` and `.tar.gz.sha256`. Continue
with the checks and installation in [INSTALL](../INSTALL.md).

Each archive has one top-level directory:

```text
kronika-<cargo-version>-<12-character-commit>-<target>/
  kronika-collector   kronika-web   kronika-dump   kronika-report   kronika-demo
  BUILDINFO          SHA256SUMS    LICENSE       THIRD_PARTY_LICENSES.html
  README.md          README.ru.md  INSTALL.md    INSTALL.ru.md
  docs/              bins/        crates/       licenses/
```

`bins/` and `crates/` contain linked guides, not source or build trees. The archive
includes both languages, real documentation images, the light/dark SVG diagrams
and their editable draw.io sources, and dependency/font notices. Guide links
stay local; links to unbundled source open the packaging commit on GitHub.
Recordings and host metadata are never included.

## Architectures and userspace checks

Two native jobs build `x86_64-unknown-linux-musl` and
`aarch64-unknown-linux-musl`, respectively on `ubuntu-24.04` and
`ubuntu-24.04-arm`. Standard ARM64 runners are available for public repositories
in [GitHub's runner reference](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
There is no cross compilation or emulation in this workflow.

Each job builds the five binaries **once**. Every row below then downloads the
same archive for its architecture, checks the archive checksum and every member
checksum, verifies the ELF architecture and absence of a dynamic interpreter or
shared-library dependency, and executes every binary with `--version`.

| Userspace | Container image | Architectures |
| --- | --- | --- |
| Ubuntu 22.04 LTS | `ubuntu:22.04` | x86-64, ARM64 |
| Ubuntu 24.04 LTS | `ubuntu:24.04` | x86-64, ARM64 |
| Ubuntu 26.04 LTS | `ubuntu:26.04` | x86-64, ARM64 |
| Debian 12, oldstable | `debian:bookworm-slim` | x86-64, ARM64 |
| Debian 13, stable | `debian:trixie-slim` | x86-64, ARM64 |
| CentOS Stream 9 | `quay.io/centos/centos:stream9` | x86-64, ARM64 |
| CentOS Stream 10 | `quay.io/centos/centos:stream10` | x86-64, ARM64 |
| Fedora 44 | `fedora:44` | x86-64, ARM64 |
| Alpine 3.24 | `alpine:3.24` | x86-64, ARM64 |
| Rocky Linux 9 | `rockylinux/rockylinux:9` | x86-64, ARM64 |
| openSUSE Leap 16.0 | `registry.opensuse.org/opensuse/leap:16.0` | x86-64, ARM64 |

Tags and both architecture manifests were checked on 2026-09-05 against the
publishers' registries. The tag catalogs are maintained by
[Ubuntu](https://github.com/docker-library/official-images/blob/master/library/ubuntu),
[Debian](https://github.com/docker-library/official-images/blob/master/library/debian),
[Fedora](https://github.com/docker-library/official-images/blob/master/library/fedora),
[Alpine](https://github.com/docker-library/official-images/blob/master/library/alpine),
[CentOS](https://quay.io/repository/centos/centos?tab=tags),
[Rocky Linux](https://hub.docker.com/r/rockylinux/rockylinux/tags), and
[openSUSE](https://registry.opensuse.org/). Rocky's maintained publisher namespace
is used because its [older Docker Official Image](https://hub.docker.com/_/rockylinux)
is no longer receiving current updates.

Tags can move. Each run saves the resolved image digest, `/etc/os-release`,
kernel/architecture, and binary-check output as a separate `portability-*`
artifact. The successful rows of that run are the tested combinations for that
candidate. A failed or unavailable image fails its row; Ubuntu 26.04 is never
silently omitted. Only native validation tools are installed in the containers;
Alpine receives no glibc compatibility package, and no foreign dynamic loader
is added to make Kronika run.

The version regression uses an unprivileged process, a read-only working
directory, empty and invalid configuration environments, a timeout, and exact
exit status, stdout, and stderr comparisons for all five executables. Expected
stdout is one line, for example `kronika-web 1.0.0`, followed by a newline. Normal
CI also traces these actual executions for startup side effects.

Containers share their runner's kernel. These checks establish execution in the
listed Linux **userspaces and architectures**; they do not establish a minimum
kernel version or identical metric availability on every host. The build uses
the baseline x86-64 or generic AArch64 CPU target, never `target-cpu=native`.
A same-architecture musl archive avoids dependence on the host's glibc version;
it is not a Windows/macOS executable or a promise to run on every CPU or kernel.

## Build and inspect a candidate

Use a clean native Linux checkout with the pinned Rust toolchain, the matching
musl target, native `musl-gcc`, GNU tar, gzip, binutils, and Python 3.11 or newer.
[Source build](build.md) covers toolchain setup.

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
# On a native ARM64 Linux machine:
scripts/package-release.sh --target aarch64-unknown-linux-musl
```

The default target is x86-64. All five binaries are included; the old
`--with-demo` option remains accepted and does nothing extra. Outputs are:

```text
dist/kronika-<cargo-version>-<12-character-commit>-<target>.tar.gz
dist/kronika-<cargo-version>-<12-character-commit>-<target>.tar.gz.sha256
```

`BUILDINFO` records the package source commit, workspace version, target, source
commit time, build command, compiler, Rust flags, and C compiler flags. Dependency notices are
checked against `licenses/dependency-inputs.sha256`; update notices and their
input hashes together when locked dependencies change.

To reuse binaries already built from the intended revision:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl \
  --bin-dir target/x86_64-unknown-linux-musl/release \
  --output-dir dist-review
```

This mode records `build_mode=prebuilt` and the packaging checkout's revision;
it does not infer the binaries' source revision or compiler. Source-build mode
records `build_mode=source`. The script rejects uncommitted checkout changes,
unsupported targets, dynamic/wrong-architecture executables, and existing output
paths. It changes no version, tag, or release.

Archive order, modes, owner/group, timestamps, and gzip metadata are fixed.
Identical binaries, documents, assets, notices, source revision, and build mode
yield identical archives. CI compares two packages of the same existing binaries
byte for byte. This is packaging determinism, not a claim that independent
compiler builds produce identical executables.

## Validate more than startup

With Node.js 22 and Chromium or Google Chrome available:

```sh
scripts/check-release.sh dist/*.tar.gz
```

Pass exactly one archive. Beyond the checks above, this starts the extracted
collector with explicit temporary storage, records real OS data into a finished
segment, stops it cleanly, and reads the resulting CPU/process sections through
the extracted dump program. A fixed test recording then exercises dump, slicing,
two byte-identical HTML reports, an authenticated web catalog, and MCP discovery.
Each subprocess has a deadline and is cleaned up. No PostgreSQL server or local
BDD execution is required.

The generated HTML is opened directly from disk by the existing browser smoke,
which checks interactive reads and absence of external requests. Both native
build jobs run the collector/dump/report/web checks; x86-64 also runs the browser
smoke. ARM64 uses `--no-browser` because its runner has no preinstalled Chromium.
The container matrix uses `--versions-only`, which retains archive, documentation,
checksum, static ELF, and all-five version checks without running the functional
workload again. BDD remains a separate CI gate.
