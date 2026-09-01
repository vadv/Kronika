# Интерактивная демоверсия Kronika

[English version](README.md)

Демоверсия запускает PostgreSQL 15, PgBouncer, коллектор Kronika,
ограниченную синтетическую нагрузку и web UI Kronika в одном сервисе Docker
Compose. Используются штатные пути сбора и хранения; внешняя база данных и
приватное окружение не нужны.

## Запуск и просмотр

Требования: Docker с Compose v2 на Linux-хосте amd64 или arm64.

```sh
make demo-up
```

Команда собирает образ, запускает сервис, ждёт успешный health check и
печатает URL. Откройте <http://127.0.0.1:8080/> и войдите:

```text
Username: demo
Password: forensics
```

По умолчанию открыт Processes. Host, Processes, PostgreSQL и Events показывают
собранный час через штатный UI Kronika. В PostgreSQL доступны Overview,
Activity, Vacuum, Locks, Statements, Plans, Databases, Tables и Indexes,
если соответствующие снимки присутствуют. PgBouncer представлен реальными
событиями его лога в Events; отдельного PgBouncer dashboard в Kronika сейчас
нет.

Перемещайте timeline cursor для просмотра записанного момента. Выбор строки
таблицы открывает Detail в Inspector, кнопка chart — график Inspector, Escape
закрывает его. Видимые элементы search фильтруют текущую поверхность; кнопка
Back браузера восстанавливает адресуемое состояние навигации.

Небольшая метка `DEMO · синтетические данные` отличает этот набор от данных
реального хоста. Нагрузка и её учётные данные локальны для сети Compose. Во
время работы UI не загружает внешние шрифты, ресурсы или пакеты.

Если порт 8080 занят, выберите другой loopback-порт:

```sh
DEMO_PORT=18081 make demo-up
```

Проверить health и следить за логами сервиса:

```sh
make demo-status
make demo-logs
```

## Остановка и удаление данных

Остановить контейнер, сохранив собранную историю Kronika:

```sh
make demo-stop
```

Повторный `make demo-up` снова запустит его. PostgreSQL и PgBouncer хранят
данные в эфемерных tmpfs и создаются заново при каждом старте контейнера;
именованный том содержит только историю Kronika. Retention ограничен 512 MiB.

Удалить контейнер, сеть и именованный том демоданных:

```sh
make demo-clean
```

Следующий `make demo-up` создаст чистую демоверсию. При сборке загружаются
закреплённые базовые образы, зависимости из Cargo.lock и точная ревизия
pg_store_plans. Для штатной работы сеть не нужна, кроме соединения браузера с
опубликованным loopback-портом.

## Бинарь `kronika-demo`

Бинарь запускает `kronika-collector` на ограниченный интервал и сообщает
размер сегмента и журнала, пиковый RSS и время CPU. В образе он управляет
коллектором и опциональной нагрузкой PostgreSQL.

| Переменная | По умолчанию | Назначение |
| --- | ---: | --- |
| `KRONIKA_DEMO_DIR` | `demo-data` | Куда пишутся лог коллектора и `report.json`. |
| `KRONIKA_STORAGE_DIR` | `$KRONIKA_DEMO_DIR/segments` | Каталог хранения коллектора. |
| `KRONIKA_DEMO_DURATION_S` | 60 | Длительность в секундах. `0` — работать до `SIGTERM` или `SIGINT`. |
| `KRONIKA_DEMO_COLLECTOR_LOG` | `file` | `file` пишет `collector.log`; `stderr` использует унаследованный stderr. В образе задан `stderr` с ограниченной ротацией логов Docker. |
| `KRONIKA_COLLECTOR_BIN` | `kronika-collector` рядом с бинарём | Какой бинарь коллектора запускать. |

Остальные переменные коллектора `KRONIKA_*` передаются без изменений.

### Опциональная нагрузка PostgreSQL

`KRONIKA_DEMO_WORKLOAD_DSN` включает нагрузку. Если переменная не задана,
`kronika-demo` сохраняет прежнее поведение только с коллектором.

| Переменная | По умолчанию | Назначение |
| --- | ---: | --- |
| `KRONIKA_DEMO_WORKLOAD_DSN` | не задана | Подключение нагрузки, обычно через PgBouncer. |
| `KRONIKA_DEMO_WORKLOAD_DIRECT_DSN` | обязательно с нагрузкой | Прямое подключение к PostgreSQL для сценария смены плана и настроек Vacuum в рамках сессии. Нельзя направлять его в PgBouncer с transaction pooling. Образ подключается к встроенному PostgreSQL. |
| `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 1 | Число схем предметной области. |
| `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 8 | Число узнаваемых таблиц интернет-магазина. |
| `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 4 | Параллельных соединений при настройке. |
| `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 4 | Долгоживущих DML-сессий. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 1 | Независимых цепочек блокировок в каждом ограниченном раунде. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 4 | Число транзакций в каждой цепочке. Вместе со временем удержания значение должно позволить одному ожидающему получить строку, а следующему — достичь `statement_timeout` через 10 с. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 4000 | Время удержания блокировки звеном одного раунда, мс. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 120 | Период без демонстрационных блокировок после каждого раунда, с. |
| `KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S` | 180 | Пауза после одного медленного запроса, одного ошибочного оператора и одной попытки подключиться к несуществующей БД, с. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROWS` | 300000 | Строк в `shop.orders` для истории со сменой плана. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS` | 4 | Параллельных сессий `checkout-api`, выполняющих один запрос. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S` | 12 | Длительность индексного baseline и восстановления, с. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S` | 30 | Длительность работы без вспомогательного индекса checkout, с. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S` | 120 | Пауза после полного раунда смены плана, с. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS` | 100000 | Строк в отдельной таблице для демонстрации Vacuum. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S` | 180 | Пауза после каждого эпизода Vacuum, с. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S` | 30 | Конечный тайм-аут каждого оператора обновления и Vacuum, с. |

Нагрузка по умолчанию изображает один интернет-магазин: `shop.orders`,
`customers`, `order_items`, `products`, `inventory`, `payments`, `event_log` и
`sessions`. Имена клиентов — `checkout-api`, `catalog-api`, `payments-worker`,
`vacuum-worker` и другие — позволяют связать наблюдения с компонентами
приложения. Постоянные сессии выполняют ограниченный поток `INSERT`, точечных
`UPDATE`, `SELECT` и `DELETE`.

В начале прогона один checkout-запрос работает на `shop.orders` до, во время и
после удаления и восстановления вспомогательного индекса. Поэтому Kronika
сохраняет для одного Query ID два плана: быстрый индексный baseline и
восстановление вокруг более медленного интервала с последовательным сканированием.
Через 65 с начинается конечная очередь блокировок строк, через 95 с — Vacuum,
через 140 с — явные события логов и ошибок. У каждого инцидента есть тайм-ауты
операторов и транзакций и длинная пауза после завершения: исторические экраны
показывают проблему и восстановление, а текущая база остаётся работоспособной.
Образ снимает состояние PostgreSQL раз в 5 с, поэтому каждый ограниченный
эпизод пересекается хотя бы с одним опросом коллектора. Ни один сценарий не
отключает `statement_timeout` или `idle_in_transaction_session_timeout`.

Прямой запуск бинаря:

```sh
KRONIKA_COLLECTOR_BIN=target/x86_64-unknown-linux-gnu/debug/kronika-collector \
KRONIKA_DEMO_WORKLOAD_DSN='host=127.0.0.1 port=6432 user=kronika_demo dbname=kronika_demo' \
KRONIKA_DEMO_WORKLOAD_DIRECT_DSN='host=127.0.0.1 port=5432 user=kronika_demo dbname=kronika_demo' \
    kronika-demo
```

`SIGTERM` и `SIGINT` останавливают нагрузку и коллектор, закрывают активный
сегмент и записывают итоговый отчёт перед выходом.
