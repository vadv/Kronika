# Переносимые архивы для Linux

[English version](releases.md) · [Установка и запуск](../INSTALL.ru.md) · [README](../README.ru.md)

## Выбрать нужный артефакт

Текущие архивы для проверки содержат все пять программ: `kronika-collector`,
`kronika-web`, `kronika-dump`, `kronika-report` и `kronika-demo`. Начните с
[установки на свою машину](../INSTALL.ru.md); demo — необязательная генерация
нагрузки. Для установки из архива и сбора только OS-метрик не нужны компилятор,
Docker, сервер PostgreSQL или отдельный каталог web-ресурсов.

Существующий публичный [релиз v1.0.0](https://github.com/vadv/Kronika/releases/tag/v1.0.0)
старее. В нём есть collector, web, dump и demo, но **нет `--version`,
`kronika-dump slice`, `kronika-report` и HTML-экспорта**. Вложенные инструкции
описывают именно ту версию. Версия workspace пока остаётся `1.0.0`; архив для
проверки с commit в имени — отдельный артефакт, а не замена опубликованного
релиза. Workflow упаковки не создаёт новый публичный релиз или тег.

[Workflow Release package](../.github/workflows/release-package.yml) загружает
архивы и checksum в Actions artifacts со сроком хранения 14 дней. Выберите
успешный запуск для нужного commit; имя артефакта содержит полный commit
исходного кода и target. Команды скачивания, проверки checksum, распаковки и
запуска приведены в [INSTALL](../INSTALL.ru.md). Скачивание через Actions
требует доступа к GitHub; это артефакты для проверки, не постоянные ссылки
на публичный релиз.

### Скачать кандидата из Actions

Войдите через [GitHub CLI](https://cli.github.com/manual/gh_run_download), выведите
успешные запуски упаковки и выберите запуск для проверяемого изменения:

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

Для ARM64 задайте `target=aarch64-unknown-linux-musl`. Выберите запуск, в котором
прошли **оба задания сборки и все строки переносимости**, а не только задание
загрузки архива. На странице запуска также можно скачать ZIP соответствующего
артефакта через браузер; распакуйте ZIP, чтобы получить `.tar.gz` и
`.tar.gz.sha256`. Продолжите проверку и установку по [INSTALL](../INSTALL.ru.md).

В каждом архиве один верхний каталог:

```text
kronika-<cargo-version>-<12-character-commit>-<target>/
  kronika-collector   kronika-web   kronika-dump   kronika-report   kronika-demo
  BUILDINFO          SHA256SUMS    LICENSE       THIRD_PARTY_LICENSES.html
  README.md          README.ru.md  INSTALL.md    INSTALL.ru.md
  docs/              bins/        crates/       licenses/
```

В `bins/` и `crates/` лежат связанные руководства, не исходники или результаты
сборки. В архив включены оба языка, настоящие иллюстрации документации, SVG-схемы
для светлой и тёмной тем с редактируемыми исходниками draw.io, уведомления
о зависимостях и лицензии шрифтов. Ссылки между руководствами остаются локальными;
ссылки на невложенные исходники открывают commit упаковки на GitHub. Записи
нагрузки и сведения о машине в архив не попадают.

## Архитектуры и проверки userspace

Два нативных задания собирают `x86_64-unknown-linux-musl` и
`aarch64-unknown-linux-musl` на `ubuntu-24.04` и `ubuntu-24.04-arm`
соответственно. Стандартные ARM64 runners для публичных репозиториев перечислены
в [справочнике GitHub](https://docs.github.com/en/actions/reference/runners/github-hosted-runners).
В этом workflow нет кросс-компиляции или эмуляции.

Каждое задание собирает пять бинарных файлов **один раз**. Затем каждая строка
ниже скачивает тот же архив для своей архитектуры, сверяет checksum архива
и каждого файла, проверяет архитектуру ELF и отсутствие динамического
интерпретатора и зависимостей от shared libraries, после чего запускает каждый
бинарный файл с `--version`.

| Userspace | Образ контейнера | Архитектуры |
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

Теги и manifests обеих архитектур проверены 2026-09-05 в реестрах издателей.
Каталоги тегов ведут
[Ubuntu](https://github.com/docker-library/official-images/blob/master/library/ubuntu),
[Debian](https://github.com/docker-library/official-images/blob/master/library/debian),
[Fedora](https://github.com/docker-library/official-images/blob/master/library/fedora),
[Alpine](https://github.com/docker-library/official-images/blob/master/library/alpine),
[CentOS](https://quay.io/repository/centos/centos?tab=tags),
[Rocky Linux](https://hub.docker.com/r/rockylinux/rockylinux/tags) и
[openSUSE](https://registry.opensuse.org/). Для Rocky взят поддерживаемый namespace
издателя: его [старый Docker Official Image](https://hub.docker.com/_/rockylinux)
больше не получает актуальных обновлений.

Теги могут изменяться. Каждый запуск сохраняет фактический digest образа,
`/etc/os-release`, ядро/архитектуру и вывод проверок бинарных файлов в отдельный
артефакт `portability-*`. Успешные строки конкретного запуска — проверенные
сочетания для этого кандидата. Ошибка или недоступность образа роняет его строку;
Ubuntu 26.04 не пропускается молча. В контейнерах устанавливаются только нативные
инструменты проверки; Alpine не получает пакет совместимости с glibc, а сторонний
динамический загрузчик для запуска Kronika не добавляется.

Регрессионная проверка версии запускает непривилегированный процесс в рабочем
каталоге только для чтения, с пустым и некорректным окружением конфигурации,
таймаутом и точным сравнением exit status, stdout и stderr всех пяти программ.
Ожидаемый stdout — одна строка, например `kronika-web 1.0.0`, с переводом строки
в конце. Обычный CI также трассирует эти реальные запуски на ранние побочные
эффекты.

Контейнеры используют ядро своего runner. Проверки устанавливают факт запуска
в перечисленных Linux **userspace и архитектурах**; они не определяют минимальную
версию ядра или одинаковую доступность метрик на всех машинах. Сборка использует
базовый target CPU x86-64 или generic AArch64, без `target-cpu=native`.
Архив musl для той же архитектуры не зависит от версии glibc на машине;
это не исполняемый файл Windows/macOS и не обещание работы на любом CPU или ядре.

## Собрать и изучить кандидата

Нужен чистый checkout на нативном Linux с закреплённым Rust toolchain,
соответствующим target musl, нативным `musl-gcc`, GNU tar, gzip, binutils и
Python 3.11 или новее. Настройка toolchain описана в
[сборке из исходного кода](build.ru.md).

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl
# На нативной машине ARM64 с Linux:
scripts/package-release.sh --target aarch64-unknown-linux-musl
```

Target по умолчанию — x86-64. Включены все пять бинарных файлов; старый параметр
`--with-demo` принимается, но ничего не добавляет. Результат:

```text
dist/kronika-<cargo-version>-<12-character-commit>-<target>.tar.gz
dist/kronika-<cargo-version>-<12-character-commit>-<target>.tar.gz.sha256
```

`BUILDINFO` содержит commit исходного кода упаковки, версию workspace, target,
время commit, команду сборки, компилятор, флаги Rust и C. Уведомления о зависимостях
сверяются с `licenses/dependency-inputs.sha256`; при изменении закреплённых
зависимостей обновляйте уведомления вместе с хешами их входных файлов.

Чтобы использовать файлы, уже собранные из нужной ревизии:

```sh
scripts/package-release.sh --target x86_64-unknown-linux-musl \
  --bin-dir target/x86_64-unknown-linux-musl/release \
  --output-dir dist-review
```

В этом режиме записываются `build_mode=prebuilt` и ревизия checkout упаковки;
ревизия исходников бинарных файлов и компилятор не угадываются. Сборка из
исходников записывает `build_mode=source`. Скрипт отклоняет незакоммиченные
изменения checkout, неподдерживаемые targets, динамические файлы или файлы
другой архитектуры, а также существующие выходные пути. Версия, теги и релиз
не меняются.

Порядок файлов, права, владелец/группа, время и метаданные gzip фиксированы.
Одинаковые бинарные файлы, документы, иллюстрации, уведомления, ревизия исходников
и режим сборки дают одинаковые архивы. CI побайтово сравнивает две упаковки
тех же готовых бинарных файлов. Это детерминированность упаковки, не обещание
одинакового результата независимых компиляций.

## Проверить работу после запуска

При установленных Node.js 22 и Chromium или Google Chrome:

```sh
scripts/check-release.sh dist/*.tar.gz
```

Передайте ровно один архив. Помимо проверок выше, скрипт запускает распакованный
collector с явным временным хранилищем, записывает настоящие OS-данные в готовый
сегмент, штатно останавливает процесс и читает секции CPU/processes через
распакованный dump. Затем фиксированная тестовая запись проходит dump, срез
по времени, генерацию двух побайтово одинаковых HTML-отчётов, web-каталог
с аутентификацией и получение списка MCP-инструментов. У каждого подпроцесса
есть срок ожидания и очистка. Сервер PostgreSQL и локальный запуск BDD не нужны.

Существующая браузерная проверка открывает полученный HTML прямо с диска,
проверяет интерактивное чтение и отсутствие внешних запросов. Оба нативных задания
сборки выполняют проверки collector/dump/report/web; x86-64 также запускает
браузерную проверку. ARM64 использует `--no-browser`, поскольку на его runner
нет предустановленного Chromium. Матрица контейнеров использует `--versions-only`:
проверки архива, документации, checksum, статических ELF и версий всех пяти
программ сохраняются, а функциональная нагрузка повторно не запускается.
BDD остаётся отдельной проверкой CI.
