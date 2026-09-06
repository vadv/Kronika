# kronika-report

[English version](README.md) · [Установка](../../INSTALL.ru.md)

`kronika-report` преобразует один готовый отдельный ZMS в интерактивный HTML.
Исходники: [команда и проверка аргументов](src/main.rs),
[справка параметров](src/help.rs).

```sh
kronika-report incident.zms incident.html
```

## Параметры

| Параметр | По умолчанию | Контракт |
| --- | --- | --- |
| `INPUT.zms` | Обязателен | Корректный готовый отдельный ZMS с любым basename. |
| `OUTPUT.html` | Обязателен | Точный путь `.html`; существующий результат заменяется атомарно. Родительский каталог существует и доступен на запись. |
| `--from MICROSECONDS` | Первая записанная микросекунда | Inclusive начало видимого navigation window. |
| `--to-exclusive MICROSECONDS` | Последняя записанная микросекунда + 1 | Exclusive конец видимого navigation window. |
| `-h`, `--help` | — | Справка параметров; завершение до обращения к файлам. |
| `--version` | — | Версия binary; завершение до обращения к файлам. |

Явные границы задаются вместе в этом порядке перед обоими путями и удовлетворяют
`0 < from < to-exclusive <= 9007199254740991`. Единицы — целые Unix microseconds.
Видимый интервал — `[from, to-exclusive)`.

## Exact report interval

```sh
kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

Это 5 сентября 2026, 19:00–20:00 UTC.
[Срез ZMS](../kronika-dump/README.ru.md#slice) может сохранять соседние samples
для интервальных вычислений; явные границы ограничивают навигацию отчёта,
а samples остаются доступны query engine. Первый rate, требующий предыдущего
sample, остаётся null при отсутствии этого sample.

## Результат и выполнение

Команда проверяет ZMS, получает его внутреннюю segment identity, строит
canonical IDX и включает ZMS/IDX, production interface и Rust/WebAssembly
query engine в HTML. Engine выполняется на основном потоке браузера. Таблицы,
heatmaps, поиск и charts работают локально; в интерфейсе отсутствуют
authentication, MCP, live refresh и Export.

Временный HTML создаётся рядом с `OUTPUT.html`; IDX sidecar не создаётся.
Конфигурация report не читает environment variables. Успешное завершение —
exit 0 с пустым stdout; ошибки идут в stderr с ненулевым exit status. `Ctrl+C`
прерывает преобразование. **Export** в web создаёт этот HTML и передаёт границы
видимого окна из выбранного интервала.
