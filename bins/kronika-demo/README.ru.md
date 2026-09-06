# Demo Kronika для разработки

[English version](README.md)

Source/Compose fixture запускает PostgreSQL 15, PgBouncer, collector, web и
синтетические нагрузки в одном сервисе. `kronika-demo` — инструмент разработки.
Продуктовые архивы содержат collector, web, dump и report.

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

Навигация и определения метрик: [controls](../../docs/features.ru.md),
[Linux](../../docs/metrics-linux.ru.md), [PostgreSQL](../../docs/metrics-postgresql.ru.md).
Интерфейс помечает запись как `DEMO · синтетические данные`.

Задать другой loopback-порт:

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
данные в эфемерных tmpfs и создаются заново при каждом старте контейнера.
Именованный том содержит историю Kronika, а во время работы демоверсии — ещё
один ограниченный scratch-файл системной нагрузки. При штатной остановке файл
удаляется. Целевой retention истории — 512 MiB.

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
коллектором, включённой по умолчанию системной нагрузкой и опциональной
нагрузкой PostgreSQL.

| Переменная | По умолчанию | Назначение |
| --- | ---: | --- |
| `KRONIKA_DEMO_DIR` | `demo-data` | Куда пишутся лог коллектора и `report.json`. |
| `KRONIKA_STORAGE_DIR` | `$KRONIKA_DEMO_DIR/segments` | Каталог хранения коллектора. |
| `KRONIKA_DEMO_DURATION_S` | 60 | Длительность в секундах. `0` — работать до `SIGTERM` или `SIGINT`. |
| `KRONIKA_DEMO_COLLECTOR_LOG` | `file` | `file` пишет `collector.log`; `stderr` использует унаследованный stderr. В образе задан `stderr` с ограниченной ротацией логов Docker. |
| `KRONIKA_COLLECTOR_BIN` | `kronika-collector` рядом с бинарём | Какой бинарь коллектора запускать. |

Остальные переменные коллектора `KRONIKA_*` передаются без изменений.

### Ограниченная системная нагрузка

Эта нагрузка включена по умолчанию и не зависит от
`KRONIKA_DEMO_WORKLOAD_DSN`. Entrypoint Compose включает её явно, поэтому она
продолжает работать и при выключенной нагрузке PostgreSQL. Некорректное или
пустое значение останавливает `kronika-demo` при запуске с указанием имени
переменной.

| Переменная | По умолчанию | Допустимые значения |
| --- | ---: | --- |
| `KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED` | `true` | Только `true` или `false`. |
| `KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR` | `$KRONIKA_DEMO_DIR/system-activity` | Непустой каталог отдельно от `KRONIKA_STORAGE_DIR`. В Compose используется `/var/lib/kronika/data/system-activity`. |
| `KRONIKA_DEMO_SYSTEM_CPU_PERCENT` | 12 | Пиковый процент одного ядра CPU, 1–25. |
| `KRONIKA_DEMO_SYSTEM_MEMORY_MIB` | 32 | Анонимный working set, 8–128 MiB. |
| `KRONIKA_DEMO_SYSTEM_FILE_MIB` | 8 | Фиксированный размер scratch-файла, 1–32 MiB. |
| `KRONIKA_DEMO_SYSTEM_DISK_KIB_PER_S` | 32 | Пиковая полезная нагрузка отдельно для чтения и записи, 1–256 KiB/s. |
| `KRONIKA_DEMO_SYSTEM_NETWORK_KIB_PER_S` | 32 | Пиковая односторонняя loopback-нагрузка, 1–256 KiB/s. |
| `KRONIKA_DEMO_SYSTEM_FLUSH_INTERVAL_S` | 5 | Интервал flush только этого файла, 1–10 с. Произведение пиковой скорости диска на интервал должно помещаться в scratch-файл. |

Внутри `kronika-demo` работают четыре именованных потока:
`krn-demo-cpu`, `krn-demo-memory`, `krn-demo-disk` и `krn-demo-loop`. Работа
CPU ограничена кадрами по 100 мс; поток памяти владеет одним массивом
фиксированного размера и раз в секунду касается каждой страницы ОС. Loopback-
поток соединяет два UDP-сокета на эфемерных портах `127.0.0.1`. Он не открывает
внешний маршрут или сервисный порт.

CPU, диск и сеть следуют фиксированной 60-секундной волне из шести
10-секундных фаз: 25%, 50%, 75%, 100%, 75% и 50% настроенного пика. При
значениях по умолчанию CPU проходит уровни 3%, 6%, 9%, 12%, 9% и 6% одного
ядра, а диск и loopback — 8, 16, 24, 32, 24 и 16 KiB/s. При идеальном scheduling средний вес phases равен
`(0.25 + 0.50 + 0.75 + 1 + 0.75 + 0.50) / 6 = 0.625` пика:

- CPU: 270 CPU-секунд в час, в среднем 7,5% одного ядра или 3,75% от двух ядер
  Compose.
- Память: 32 MiB затронутой анонимной памяти. Файл размером 8 MiB жёстко
  ограничивает scratch-данные и их страницы в page cache.
- Диск: 73 728 000 байт (70,3125 MiB) записи в час и столько же чтения, не
  более 140,625 MiB суммарно. Метаданные файловой системы могут добавить
  небольшой объём. Задержка планировщика способна только уменьшить нагрузку.
- Loopback: 73 728 000 байт (70,3125 MiB) полезной нагрузки в час попадает в
  каждый namespace-счётчик RX и TX, суммарно 140,625 MiB плюс заголовки UDP/IP.

Дисковый поток владеет ровно одним файлом
`kronika-demo-system-activity.bin`. Он один раз задаёт длину и перезаписывает
страницы по кольцу, ничего не дописывая в конец. На каждом интервале поток
вызывает `sync_data` только для этого файла, просит ядро выгрузить только уже
записанные страницы и читает их снова, чтобы росли физические счётчики чтения
и записи. Глобальный `sync` не используется. Оставшийся после аварийного
завершения обычный файл с точным именем заменяется при старте; symlink и другие
типы файлов отвергаются. Штатная остановка дожидается всех потоков и удаляет
только этот файл. Ошибка одного потока записывается в лог и не останавливает
коллектор, нагрузку PostgreSQL или остальные системные потоки.

#### Системная smoke-проверка за 75 секунд

Проверка изолирует `kronika-demo` от PostgreSQL и внешней сети. Она сравнивает
два снимка PID демо, loopback-интерфейса и фиксированного scratch-файла, а затем
проверяет штатное удаление файла.

```bash
set -eu
make demo-image
smoke_dir=$(mktemp -d /var/tmp/kronika-system-smoke.XXXXXX)
smoke_name="kronika-system-smoke-$$"
cleanup() { docker rm -f "$smoke_name" >/dev/null 2>&1 || true; }
trap cleanup EXIT

cid=$(docker run --detach \
  --name "$smoke_name" \
  --no-healthcheck \
  --network none \
  --read-only \
  --cpus 2 \
  --memory 1g \
  --pids-limit 512 \
  --tmpfs /tmp:rw,nosuid,nodev,noexec,size=64m \
  --mount type=bind,src="$smoke_dir",dst=/data \
  -e KRONIKA_DEMO_DIR=/data \
  -e KRONIKA_STORAGE_DIR=/data/segments \
  -e KRONIKA_DEMO_DURATION_S=0 \
  -e KRONIKA_DEMO_COLLECTOR_LOG=stderr \
  -e KRONIKA_DEMO_SYSTEM_WORKLOAD_ENABLED=true \
  -e KRONIKA_DEMO_SYSTEM_WORKLOAD_DIR=/data/system-activity \
  --entrypoint /usr/local/bin/kronika-demo \
  kronika-demo:local)

sample() {
  docker exec "$cid" sh -ceu '
    test "$(cat /proc/1/comm)" = kronika-demo
    set -- $(cat /proc/1/stat)
    cpu=$(( ${14} + ${15} ))
    rss=$(awk "/^VmRSS:/ { print \$2 }" /proc/1/status)
    anon=$(awk "/^RssAnon:/ { print \$2 }" /proc/1/status)
    threads=$(awk "/^Threads:/ { print \$2 }" /proc/1/status)
    rb=$(awk "/^read_bytes:/ { print \$2 }" /proc/1/io)
    wb=$(awk "/^write_bytes:/ { print \$2 }" /proc/1/io)
    rx=$(cat /sys/class/net/lo/statistics/rx_bytes)
    tx=$(cat /sys/class/net/lo/statistics/tx_bytes)
    size=$(stat -c %s /data/system-activity/kronika-demo-system-activity.bin)
    printf "%s %s %s %s %s %s %s %s %s\n" \
      "$cpu" "$rss" "$anon" "$rb" "$wb" "$threads" "$rx" "$tx" "$size"
  '
}

sleep 10
read -r cpu0 rss0 anon0 rb0 wb0 threads0 rx0 tx0 size0 <<EOF
$(sample)
EOF
sleep 65
read -r cpu1 rss1 anon1 rb1 wb1 threads1 rx1 tx1 size1 <<EOF
$(sample)
EOF

test "$cpu1" -gt "$cpu0"
test "$rss1" -gt 0
test "$anon1" -ge 30000
test "$rb1" -gt "$rb0"
test "$wb1" -gt "$wb0"
test "$threads1" -gt 1
test "$rx1" -gt "$rx0"
test "$tx1" -gt "$tx0"
test "$size0" -eq 8388608
test "$size1" -eq "$size0"

docker exec "$cid" sh -c 'for f in /proc/1/task/*/comm; do cat "$f"; done'
docker exec "$cid" sh -c 'test ! -r /sys/fs/cgroup/io.stat || cat /sys/fs/cgroup/io.stat'
docker stop --time 50 "$cid" >/dev/null
test "$(docker inspect --format '{{.State.ExitCode}}' "$cid")" -eq 0
test ! -e "$smoke_dir/system-activity/kronika-demo-system-activity.bin"
docker logs "$cid"
docker rm "$cid" >/dev/null
trap - EXIT
printf 'Smoke data retained at %s\n' "$smoke_dir"
```

### Опциональная нагрузка PostgreSQL

`KRONIKA_DEMO_WORKLOAD_DSN` включает нагрузку PostgreSQL. Если не задана,
нагрузка PostgreSQL выключена; системная нагрузка по умолчанию продолжает работать.

| Переменная | По умолчанию | Назначение |
| --- | ---: | --- |
| `KRONIKA_DEMO_WORKLOAD_DSN` | не задана | Подключение нагрузки, обычно через PgBouncer. |
| `KRONIKA_DEMO_WORKLOAD_DIRECT_DSN` | обязательно с нагрузкой | Прямое подключение к PostgreSQL для сценария смены плана и настроек Vacuum в рамках сессии. Нельзя направлять его в PgBouncer с transaction pooling. Образ подключается к встроенному PostgreSQL. |
| `KRONIKA_DEMO_WORKLOAD_SCHEMAS` | 1 | Число схем предметной области: от 1 до 8. |
| `KRONIKA_DEMO_WORKLOAD_TABLES_PER_SCHEMA` | 8 | Число таблиц в схеме: от 8 таблиц интернет-магазина до 64 таблиц всего. |
| `KRONIKA_DEMO_WORKLOAD_DDL_CONCURRENCY` | 4 | Параллельных соединений при настройке: от 1 до 16. |
| `KRONIKA_DEMO_WORKLOAD_SESSIONS` | 4 | Долгоживущих OLTP-клиентов: от 1 до 16. |
| `KRONIKA_DEMO_WORKLOAD_TPS` | 20 | Максимальное общее число OLTP-транзакций в секунду: от 1 до 64. |
| `KRONIKA_DEMO_WORKLOAD_MAX_ORDERS` | 10000 | Переиспользуемых слотов активных OLTP-заказов: от числа клиентов до 50000. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAINS` | 1 | Независимых цепочек блокировок в каждом ограниченном раунде: от 1 до 4. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_CHAIN_DEPTH` | 4 | Число транзакций в каждой цепочке: от 2 до 8. Вместе со временем удержания значение должно позволить одному ожидающему получить строку, а следующему — достичь `statement_timeout` через 10 с. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_HOLD_MS` | 4000 | Время удержания блокировки звеном одного раунда, мс. |
| `KRONIKA_DEMO_WORKLOAD_LOCK_ROUND_INTERVAL_S` | 120 | Период без демонстрационных блокировок после каждого раунда, с. |
| `KRONIKA_DEMO_WORKLOAD_EVENT_ROUND_INTERVAL_S` | 180 | Пауза после одного медленного запроса, одного ошибочного оператора и одной попытки подключиться к несуществующей БД, с. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROWS` | 300000 | Строк в `shop.orders` для истории со сменой плана: от 1 до 500000. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_WORKERS` | 4 | Параллельных сессий `checkout-api`, выполняющих один запрос: от 1 до 8. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_BASELINE_S` | 12 | Длительность индексного baseline и восстановления, с. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_REGRESSION_S` | 30 | Длительность работы без вспомогательного индекса checkout, с. |
| `KRONIKA_DEMO_WORKLOAD_PLAN_ROUND_INTERVAL_S` | 120 | Пауза после полного раунда смены плана, с. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROWS` | 100000 | Строк в отдельной таблице для демонстрации Vacuum: от 1 до 250000. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_ROUND_INTERVAL_S` | 180 | Пауза после каждого эпизода Vacuum, с. |
| `KRONIKA_DEMO_WORKLOAD_VACUUM_STATEMENT_TIMEOUT_S` | 30 | Конечный тайм-аут каждого оператора обновления и Vacuum, с. |

Постоянную нагрузку по умолчанию создают четыре долгоживущих клиента
`shop-oltp-*` через соединение PgBouncer. В сумме они выполняют не более 20
коротких транзакций в секунду. Каждая транзакция читает по ключам покупателя и
товар, блокирует и изменяет одну строку остатков, затем записывает связанные
заказ, позицию, платёж, событие и прикладную сессию. Каждый клиент
переиспользует только собственные слоты заказов. Поэтому таблицы заказов,
позиций, платежей и связанных событий хранят не более 10000 активных
OLTP-строк каждая, продолжая создавать нагрузку на WAL, буферы, таблицы и
индексы. Медленная транзакция снижает фактическую частоту; пропущенная работа
не воспроизводится всплеском.

Reference data содержит 20,000 customers и 2,048 products. OLTP order slots
занимают IDs выше fixtures планов и Vacuum.

| Fixture | Операция и время начала |
| --- | --- |
| Plans | Повторяет один checkout query на `shop.orders` до, во время и после удаления/восстановления его вспомогательного индекса. |
| Locks | Цепочка row locks начинается через 65 секунд. |
| Vacuum | Maintenance episode начинается через 95 секунд. |
| Events | Явные slow-query/error/connection events начинаются через 140 секунд. |

Образ собирает PostgreSQL каждые 5 секунд. У statements и transactions нагрузки
конечные timeouts. Исходник: [workload](src/workload).

Прямой запуск бинаря:

```sh
KRONIKA_COLLECTOR_BIN=target/x86_64-unknown-linux-gnu/debug/kronika-collector \
KRONIKA_DEMO_WORKLOAD_DSN='host=127.0.0.1 port=6432 user=kronika_demo dbname=kronika_demo' \
KRONIKA_DEMO_WORKLOAD_DIRECT_DSN='host=127.0.0.1 port=5432 user=kronika_demo dbname=kronika_demo' \
    kronika-demo
```

`SIGTERM` и `SIGINT` останавливают нагрузку и коллектор, сохраняют журнал
collector и записывают итоговый отчёт перед выходом.

Исходники: [supervisor](src/main.rs), [системная нагрузка](src/system_activity), [Compose](../../compose.demo.yml).
