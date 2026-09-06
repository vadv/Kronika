# Установка на Linux

[English version](INSTALL.md) · [README](README.ru.md)

Архив содержит четыре статических исполняемых файла. Collector пишет запись;
web читает её и создаёт indexes. Dump и report работают с сохранёнными данными.
Требования к запуску: Linux с архитектурой архива и доступ к каталогу записи.
Для сбора PostgreSQL дополнительно нужно подключение для мониторинга.

## 1. Скачивание и распаковка

[Скачайте архив кандидата](docs/releases.ru.md#download) и его
`.tar.gz.sha256`. Соответствие архитектур:

| `uname -m` | Target архива |
| --- | --- |
| `x86_64` | `x86_64-unknown-linux-musl` |
| `aarch64` | `aarch64-unknown-linux-musl` |

В каталоге загрузки подставьте имя скачанного файла и выполните:

```sh
archive='kronika-1.0.0-REPLACE_WITH_COMMIT-x86_64-unknown-linux-musl.tar.gz'
sha256sum --check "$archive.sha256"
tar -xzf "$archive"
cd "${archive%.tar.gz}"
sha256sum --check SHA256SUMS
cat BUILDINFO
```

## 2. Установка

```sh
for binary in kronika-collector kronika-web kronika-dump kronika-report; do
  "./$binary" --version
done
sudo install -d -m 0755 /usr/local/bin
sudo install -m 0755 kronika-collector kronika-web kronika-dump \
  kronika-report /usr/local/bin/
```

## 3. Сбор Linux

```sh
sudo install -d -m 0700 /var/lib/kronika
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  /usr/local/bin/kronika-collector
```

Корень хранилища — обычный каталог. Root может читать защищённые process I/O и
локальные логи. Интервалы processes/core по умолчанию — 5/10 секунд; порог
возраста сегмента — 900 секунд. Web читает `active.wal` до его преобразования
в готовый сегмент. `Ctrl+C` останавливает сбор и сохраняет журнал; та же
команда повторно открывает запись.

`KRONIKA_RETENTION` по умолчанию — `2147483648` bytes (2 GiB). Для фиксированной
цели 10 GiB добавьте `KRONIKA_RETENTION=10737418240`.
[Хранение](bins/kronika-collector/README.ru.md#storage) определяет учитываемые
файлы и порядок удаления.

## 4. Запуск web

Во втором терминале замените пароль и выполните:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_WEB_LISTEN=127.0.0.1:8080 \
  KRONIKA_WEB_SOURCES=1 \
  KRONIKA_WEB_USER=kronika \
  KRONIKA_WEB_PASSWORD='replace-with-a-random-password' \
  /usr/local/bin/kronika-web
```

Откройте <http://127.0.0.1:8080/> и войдите. Web нужен доступ на запись в
каталог данных для файлов `.idx` и index ownership lock. В этом примере обе
программы работают от root с закрытым каталогом хранения.

Для доступа с другой машины выполните на ней:

```sh
ssh -N -L 8080:127.0.0.1:8080 user@monitored-host
```

Откройте на ней <http://127.0.0.1:8080/>. MCP использует тот же listener и
учётные данные на `/mcp`; [настройка клиентов](docs/mcp-clients.ru.md) также
доступна в панели **AI**. [Systemd](docs/services.ru.md) определяет постоянные
сервисы.

## 5. PostgreSQL

В `psql` от администратора PostgreSQL:

```sql
CREATE ROLE kronika_monitor LOGIN;
\password kronika_monitor
GRANT pg_monitor TO kronika_monitor;
GRANT EXECUTE ON FUNCTION pg_catalog.pg_current_logfile() TO kronika_monitor;
```

Роли нужны наследуемое членство в `pg_monitor`, `CONNECT` к каждой собираемой
database и локальные для database права extension из раздела
[PostgreSQL role](bins/kronika-collector/README.ru.md#postgresql-role).

Остановите collector через `Ctrl+C`, затем запустите с первым metric DSN:

```sh
sudo env KRONIKA_STORAGE_DIR=/var/lib/kronika \
  KRONIKA_PG_DSNS='host=127.0.0.1 port=5432 user=kronika_monitor password=replace-with-password dbname=postgres' \
  /usr/local/bin/kronika-collector
```

| Параметр или подключение | Контракт |
| --- | --- |
| `KRONIKA_PG_DSNS` | Первый DSN включает метрики доступных для подключения databases этого сервера. Дополнительные DSNs через `;` обнаруживают логи. |
| `KRONIKA_POSTGRES_EFFECTIVE_CPUS` | Необязательное целое `1..4294967295`: эффективная CPU capacity наблюдаемого PostgreSQL. Нужно для числового PostgreSQL health. |
| Обнаружение extensions | Поддерживаемые интерфейсы `pg_stat_statements` и `pg_store_plans` обнаруживаются в доступных databases. Activity, Locks и статистика relations используют встроенные views PostgreSQL. |
| Transport | Native client использует `NoTls`; поддерживаются прямое подключение к PostgreSQL и PgBouncer session pooling. Metric sessions сохраняют состояние `SET`. |
| Пути логов | Обнаружение возвращает пути на машине collector. Смонтированные логи задаются через `KRONIKA_PG_LOGS`; PgBouncer использует `KRONIKA_PGBOUNCER_DSNS` или `KRONIKA_PGBOUNCER_LOGS`. |

Перезапустите web с `KRONIKA_WEB_SOURCES=3`, чтобы отметить OS и PostgreSQL как
настроенные в его каталоге. Сбор включают DSNs collector; bitset web задаёт
метаданные каталога. User и password обязательны и при
`KRONIKA_WEB_AUTH=disabled`.

[Конфигурация сервисов](docs/services.ru.md) хранит DSNs и учётные данные web в
environment files, доступных root. [Справочник collector](bins/kronika-collector/README.ru.md)
определяет интервалы, поддерживаемые layouts extensions и форматы логов.

## Справочники

[Controls](docs/features.ru.md) · [Расчётные примеры](docs/operator-guide.ru.md) ·
[Сборка из исходников](docs/build.ru.md) · [Dump](bins/kronika-dump/README.ru.md) ·
[HTML reports](bins/kronika-report/README.ru.md)
