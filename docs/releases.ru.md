# Справочник архивов Linux

[English version](releases.md) · [Установка](../INSTALL.ru.md)

## Доступные сборки

Публичный [релиз v1.0.0](https://github.com/vadv/Kronika/releases/tag/v1.0.0)
содержит collector, web, dump и demo; в нём ещё нет `--help`, `--version`, dump
slice, report и HTML-экспорта. Текущая версия исходников остаётся `1.0.0`.
Текущий пакет из четырёх программ доступен как Actions artifact
[Release package](../.github/workflows/release-package.yml) с commit в имени
и сроком хранения 14 дней. Workflow создаёт архивы без tags и releases.

<a id="download"></a>
## Скачивание

С авторизованным [GitHub CLI](https://cli.github.com/manual/gh_run_download)
выберите успешный запуск workflow:

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

Для ARM64 задайте `target=aarch64-unknown-linux-musl`. `gh run view` показывает
оба native build jobs и 22 проверки userspace. Скачанный artifact содержит
`.tar.gz` и `.tar.gz.sha256`; продолжите [распаковку и установку](../INSTALL.ru.md#1-скачивание-и-распаковка).

## Состав и идентификация

```text
kronika-<cargo-version>-<12-character-commit>-<target>/
  kronika-collector   kronika-web   kronika-dump   kronika-report
  BUILDINFO          SHA256SUMS    LICENSE       THIRD_PARTY_LICENSES.html
  README.md          README.ru.md  INSTALL.md    INSTALL.ru.md
  DESIGN.md          DESIGN.ru.md
  docs/              bins/        crates/       licenses/
```

Четыре исполняемых файла — статические Linux ELF. Документация включает парные
руководства, реальные PNG-иллюстрации, светлые/тёмные SVG-схемы, редактируемые
исходники draw.io и license notices. `bins/` и `crates/` содержат связанные
руководства. Ссылки на исходники вне архива ведут на commit упаковки в GitHub.

| Файл или поле | Значение |
| --- | --- |
| Имя архива | Версия workspace, первые 12 hexadecimal characters commit упаковки, target. |
| `BUILDINFO.package_source_revision` | Полный commit чистого checkout упаковки. |
| `BUILDINFO.build_mode` | `source`: компиляция командой упаковки. `prebuilt`: binaries переданы через `--bin-dir`; этот режим не записывает source/compiler identity binaries. |
| `BUILDINFO.source_date_epoch` | Timestamp commit упаковки, Unix seconds. |
| Поля `BUILDINFO` при сборке из исходников | Build command, Rust compiler, Rust flags и C flags. |
| `SHA256SUMS` | SHA-256 каждого файла, кроме самого manifest. |
| `<archive>.sha256` | SHA-256 сжатого архива. |

## Native targets и матрица userspace

| Target | Native build runner | CPU target |
| --- | --- | --- |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | `x86-64` |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | `generic` |

Каждый job собирает четыре исполняемых файла один раз. Его архив используется
во всех userspaces той же архитектуры:

| Userspace | Образ контейнера | Архитектуры |
| --- | --- | --- |
| Ubuntu 22.04 LTS | `ubuntu:22.04` | x86-64, ARM64 |
| Ubuntu 24.04 LTS | `ubuntu:24.04` | x86-64, ARM64 |
| Ubuntu 26.04 LTS | `ubuntu:26.04` | x86-64, ARM64 |
| Debian 12 | `debian:bookworm-slim` | x86-64, ARM64 |
| Debian 13 | `debian:trixie-slim` | x86-64, ARM64 |
| CentOS Stream 9 | `quay.io/centos/centos:stream9` | x86-64, ARM64 |
| CentOS Stream 10 | `quay.io/centos/centos:stream10` | x86-64, ARM64 |
| Fedora 44 | `fedora:44` | x86-64, ARM64 |
| Alpine 3.24 | `alpine:3.24` | x86-64, ARM64 |
| Rocky Linux 9 | `rockylinux/rockylinux:9` | x86-64, ARM64 |
| openSUSE Leap 16.0 | `registry.opensuse.org/opensuse/leap:16.0` | x86-64, ARM64 |

Каждая строка проверяет checksums архива/файлов, состав документации, архитектуру
ELF, отсутствие `INTERP`/`NEEDED`, help/version и обработку аргументов четырёх
программ. Artifact `portability-*` записывает resolved image digest,
`/etc/os-release`, kernel/architecture и вывод проверки. Контейнеры используют
ядро native runner; матрица проверяет запуск в этих userspaces с этим ядром.
Определение матрицы: [release-package.yml](../.github/workflows/release-package.yml).

## Упаковка

Требования: чистый checkout с сохранёнными commits, native Linux,
[закреплённый build toolchain](build.ru.md), GNU tar, gzip, binutils и Python 3.11+.

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
```

На native ARM64 используйте `--target aarch64-unknown-linux-musl`. Target по
умолчанию: `x86_64-unknown-linux-musl`. Каталог результата по умолчанию: `dist`.

Упаковка готовых статических binaries:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl \
  --bin-dir target/x86_64-unknown-linux-musl/release \
  --output-dir dist-review
```

Скрипт отклоняет dirty checkouts, неподдерживаемые targets, динамические
исполняемые файлы, другую архитектуру и существующие выходные пути.
Dependency notices проверяются по `licenses/dependency-inputs.sha256`.

Порядок файлов, permissions, owner/group, timestamps и gzip metadata
фиксированы. Одинаковые bytes binaries/documents, source revision и build mode
дают одинаковые архивы. CI побайтово сравнивает две упаковки одних binaries.
Исходник: [package-release.sh](../scripts/package-release.sh).

## Проверки

При установленных `strace`, Node.js 22 и Chromium/Google Chrome передайте один архив:

```sh
scripts/check-release.sh dist/kronika-1.0.0-REPLACE_WITH_COMMIT-x86_64-unknown-linux-musl.tar.gz
```

| Режим | Проверки |
| --- | --- |
| По умолчанию | Проверка архива; CLI; реальный сбор OS и чтение dump; срез fixture; два одинаковых HTML reports; web catalog с аутентификацией; MCP discovery; интерактивность и сетевые запросы HTML, открытого с диска. |
| `--no-browser` | Все проверки по умолчанию, кроме browser step; используется native ARM64 CI. |
| `--cli-only` | Проверки архива и CLI; используется каждой строкой userspace. |

CLI checks запускают непривилегированные процессы в read-only рабочем каталоге
с пустым и некорректным environment, deadlines и точными проверками stdout,
stderr и exit status. Native modes также трассируют help/version на обращения
к хранилищу, запуск логирования, threads, processes и сети. Исходники:
[check-release.sh](../scripts/check-release.sh), [check-cli.py](../scripts/check-cli.py).
