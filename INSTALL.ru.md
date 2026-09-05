# Установка Kronika из архива

[English version](INSTALL.md)

В архиве находятся статические исполняемые файлы для Linux x86-64:
`kronika-collector`, `kronika-web`, `kronika-dump` и `kronika-report`. Некоторые
архивы также содержат `kronika-demo`, утилиту запуска нагрузки и сбора.
`BUILDINFO` указывает ревизию исходного кода упаковки и режим сборки,
`SHA256SUMS` позволяет проверить бинарные файлы. Они лежат прямо в
распакованном каталоге; команды ниже выполняются оттуда. Для хранения данных
самой Kronika не нужны Node.js или база данных.

## Проверить и распаковать

Положите архив и соответствующий `.tar.gz.sha256` в один каталог. Укажите
точное имя полученного архива и проверьте его перед распаковкой:

```sh
archive='kronika-1.0.0-COMMIT-x86_64-unknown-linux-musl.tar.gz'
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
```

Имя с commit обозначает подготовленный артефакт, а не опубликованный тег
версии. В исходном опубликованном архиве v1.0.0 ещё нет `kronika-report` и
HTML-экспорта; эта инструкция относится к текущей упаковке.

## Записать Linux и PostgreSQL

Создайте роль PostgreSQL на наблюдаемом сервере; замените пароль из примера:

```sh
sudo -u postgres psql <<'SQL'
CREATE ROLE kronika_monitor LOGIN PASSWORD 'replace-with-password';
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
SQL
```

Запустите коллектор на наблюдаемой машине:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  ./kronika-collector
```

Уберите `KRONIKA_PG_DSNS`, если нужна только запись Linux. Другой переключатель
для метрик PostgreSQL не нужен. Первый DSN задаёт сервер для сбора метрик по
всем доступным этой роли базам; `pg_stat_statements` и `pg_store_plans`
собираются при наличии поддерживаемой установки. Пути, полученные через log
discovery, должны быть доступны локально. Цель retention по умолчанию — 2 GiB,
включая текущий журнал, готовые записи и производные индексы.
`KRONIKA_RETENTION` принимает бюджет в байтах или `auto:P` — целевую долю
занятого места в файловой системе, в процентах. `Ctrl+C` останавливает сбор;
следующий запуск продолжает работу с сохранённым журналом.

В другом терминале, из каталога архива:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_SOURCES=3 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  ./kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите с этими учётными данными. По
умолчанию web слушает loopback. `KRONIKA_WEB_SOURCES=3` объявляет оба семейства
источников; `1` — только Linux. Переменная не включает сбор и не фильтрует
записанные данные. Web нужен доступ на запись в тот же каталог данных для
создания индексов. MCP доступен по адресу `http://127.0.0.1:8080/mcp` с той же
аутентификацией; настройка клиентов есть в панели MCP веб-интерфейса.

## Посмотреть запись и экспортировать интервал

Корень хранилища должен быть настоящим каталогом, не файлом и не symlink:

```sh
sudo ./kronika-dump /var/lib/kronika
```

Выберите записанный интервал. Обе границы среза включены и задаются целыми
секундами RFC 3339. Команда отказывается писать поверх существующего файла.

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika ./kronika-dump slice \
  --from 2026-09-05T19:00:00Z --to 2026-09-05T19:59:59Z --out incident.zms
sudo chown "$(id -u):$(id -g)" incident.zms
./kronika-report --from 1788634800000000 --to-exclusive 1788638400000000 \
  incident.zms incident.html
```

Границы report задаются Unix-микросекундами: здесь ровно 19:00–20:00 UTC,
соседние отсчёты для вычислений не попадают в навигацию. Без границ report
показывает весь интервал ZMS. Существующий HTML заменяется атомарно. Отчёт
открывается прямо в браузере без сервера, сети или сопутствующих файлов.
Кнопка «Экспорт» в web создаёт такой же отчёт. В статическом отчёте нет MCP и
живого обновления.

Полная документация находится в README репозитория, руководствах коллектора
и web, а также в `docs/mcp-clients.ru.md` на ревизии из `BUILDINFO`.
