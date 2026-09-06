# kronika-dump

[English version](README.md) · [Установка](../../INSTALL.ru.md)

`kronika-dump` читает каталог записи или извлекает интервал в один отдельный
ZMS. Исходники: [inspection parser](src/main.rs), [команда slice](src/slice.rs),
[справка параметров](src/help.rs).

## Inspect

```sh
kronika-dump /var/lib/kronika
kronika-dump /var/lib/kronika --json
kronika-dump /var/lib/kronika --index
kronika-dump /var/lib/kronika --section 1100001 --limit 10
```

| Параметр | По умолчанию | Значение |
| --- | --- | --- |
| `DIR` | Обязателен | Обычный корень хранения collector с `YYYY/MM/DD/*.zms` и необязательным `active.wal`. Symlinks и пути отдельных ZMS отклоняются. |
| Вывод без flags | Сводка сегментов | Границы сегментов, physical section IDs, число строк, bytes секций и physical overhead bytes. |
| `--section ID` | Не задан | Декодирование одного physical numeric `type_id` с раскрытием dictionary references. |
| `--index` | Не задан | Сводки derived series/index; с `--json` — отдельные points и finding locators. Несовместим с `--section`; sidecar не создаётся. |
| `--json` | Текст | NDJSON; scan warnings выводятся как JSON objects в stdout. |
| `--limit N` | `20` | Неотрицательный лимит строк на сегмент; `0` — все строки. Требует `--section`. |
| `--from`, `--to` | Без границ | Inclusive signed Unix microseconds; выбирают пересекающиеся сегменты. Строки секций внутри выбранных сегментов не обрезаются. Можно задать одну границу. |

Inspection читает готовые сегменты и committed active journal. Нужен доступ на
чтение хранения; configuration environment не читается. Данные идут в stdout;
текстовые warnings/errors — в stderr. Закрытый output pipe завершает команду
успешно; остальные ошибки дают ненулевой exit status.

## Slice

```sh
KRONIKA_STORAGE_DIR=/var/lib/kronika kronika-dump slice \
  --from 2026-09-05T19:00:00Z \
  --to 2026-09-05T19:59:59Z \
  --out incident.zms
```

| Параметр | Значение |
| --- | --- |
| `KRONIKA_STORAGE_DIR` | Обязательный обычный корень хранения collector с доступом на чтение; вход — готовые сегменты и committed journal. |
| `--from RFC3339` | Обязательная первая целая секунда включительно, с `Z` или timezone offset. |
| `--to RFC3339` | Обязательная последняя целая секунда включительно, не раньше `--from`. Равные границы выбирают одну полную секунду. Fractional seconds и numeric Unix values отклоняются. |
| `--out FILE.zms` | Обязательный новый путь `.zms`. Родительский каталог существует и доступен на запись. Существующие пути отклоняются. |

Каждый параметр задаётся один раз, в любом порядке. Логический интервал —
`[from, to + 1 second)`. Результат может сохранять samples в пределах 30 секунд
с каждой стороны для интервальных вычислений. Интервал без записанных строк
завершается ошибкой.

Временные файлы и scratch data создаются рядом с результатом на той же
filesystem. Готовый ZMS проверяется перед публикацией. Stdout содержит bytes,
rows, sections и requested/actual bounds в Unix microseconds;
`requested_to_exclusive` на одну секунду больше `--to`. Ошибки идут в stderr и
дают ненулевой exit status. [kronika-report](../kronika-report/README.ru.md)
преобразует результат в HTML.

## Общие параметры

`-h` и `--help` выбирают общую справку; `slice -h` и `slice --help` — справку
slice. `--version` выводит версию binary. Эти вызовы завершаются до обращения
к хранилищу. `Ctrl+C` прерывает выполняемую команду.
