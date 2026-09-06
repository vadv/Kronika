# Работа с записанным часом

[English version](operator-guide.md) · [Контролы](features.ru.md#контролы) · [Справочник метрик](features.ru.md)

Четыре примера используют записанную demo-нагрузку Linux/PostgreSQL за **5 сентября 2026, 19:00–20:00 UTC**. Иллюстрации получены из указанного HTML; отображаемые числа округлены. [Установка и запись собственного host](../INSTALL.ru.md).

## Выбрать время и объект

1. Выбрать **UTC**, затем записанный день/час.
2. Установить cursor через timeline или ←/→. Диапазон графика — выбранный час; каждый источник выбирает своё наблюдение по [правилам времени](metrics-time.ru.md).
3. Выбрать view и lens. Применить Search через Enter; выбрать строку для Detail или Chart. Метрика графика задаёт ординату; cursor выбирает её чтение.
4. Открыть Activity для ranking полного часа. Global использует общий максимум интенсивности, Per row — максимум строки. Ячейка выбирает `cell.to−1 µs`; поддерживаемое имя строки применяет фильтр объекта/группы.

| Операция | Полученный выбор |
| --- | --- |
| Processes → строка → Activity | Ближайший к cursor PostgreSQL sample для этого точного PID. |
| Statements → Query ID / Open plans | Plans с фильтром database, role и public Query ID при тех же часе/cursor. |
| Plans → Related statements | Statements с фильтром database, role и записанного связанного Query ID. |
| Tables → Indexes | Indexes выбранного записанного relation. |
| Locks → строка | Blocker PIDs, lock target и backend context в Inspector. |
| Events → группа → представительная запись | Полная записанная строка журнала и её timestamp. |

## 1. Знаменатели ресурсов container и host

[Host, 19:00:33 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788634833931637&view=host)

![Чтения Container, Network namespace и Host](images/host-scopes.png)

1. Открыть **Host → Container → CPU**. Прочитать использование CPU и записанный effective CPU limit; выбрать историю CPU.
2. Открыть **Host → CPU**, затем Container Memory и Host Memory. Каждый выбранный показатель имеет свою область измерения и знаменатель.
3. Открыть **Network namespace** для RX/TX у того же cursor.

| Чтение | Расчёт / область |
| --- | --- |
| Container CPU **66.8%** | `100 × used cgroup cores / effective cgroup CPU capacity`. |
| Host CPU **17.5%** | `100 × R(user+nice+system+irq+softirq+steal)/(H×N)`, с записанными ticks/s `H` и числом online CPU `N`. |
| Container memory **53.8%** | `100 × memory.current / effective memory limit`. |
| Host memory **12.9%** | Доля использованной памяти host; операнды в [Linux](metrics-linux.ru.md). |
| Throttled **34.9%** | Время throttling cgroup / наблюдаемый интервал реального времени × 100. |
| CPU PSI **4.3%** | Интервальная разность CPU `some_total` cgroup / наблюдаемый интервал реального времени × 100. |
| RX **284 KiB/s**, TX **284 KiB/s** | Отдельные rates byte counters записанного network namespace. |

[Справочник Linux](metrics-linux.ru.md) определяет effective ceilings, единицы PSI, identities устройств и агрегацию USE.

## 2. Вклад команды и отдельный PID

[Processes CPU, 19:03:39 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788635019201666&lens=cpu)

![Activity команд за час и Processes CPU](images/processes.png)

1. Открыть **Processes → CPU → Activity → CPU time** со шкалой **Global**.
2. Нажать `postgres` для фильтра команды. Удалить chip, выбрать PID **64**, command `/usr/local/bin/kronika-demo`, открыть его историю CPU.
3. Выбрать **Activity → RSS** для средних команд; **Memory** для значения PID 64 у cursor. Выбрать **Disk** для его counters storage/logical I/O.

| Чтение | Расчёт / набор объектов |
| --- | --- |
| `postgres`: **517 PIDs**, **9.44 min** | Уникальные числовые PID под этой командой за час; сумма разностей counters CPU процессов, делённая на записанные clock ticks/s. |
| `kronika-demo`: **4.75 min**; Total **14.6 min** | Сумма CPU команды; Total включает все команды. |
| Ячейка `postgres` **953 ms/s**, `kronika-demo` **79.8 ms/s** | CPU seconds за наблюдаемый интервал ячейки: отображаемые rates соответствуют примерно **0.953** и **0.0798** занятого ядра. |
| PID 64 user **0.12 cores**, system **0.006 cores** | Соседние same-PID/same-starttime `Δutime/(HZ×Δt)` и `Δstime/(HZ×Δt)`; сумма отображаемого вклада ≈ **0.126 cores**. |
| RSS **Average** | Сумма записанного RSS команды по всем process snapshot timestamps, делённая на их общее количество. |

Сводка команды охватывает час; таблица PID использует его соседнюю пару наблюдений. [Операнды heatmap и RSS](metrics-time.ru.md).

## 3. Интервал statement и записанный plan

[Statements, 19:00:33 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788634833931637&view=pg.statements)

1. Открыть **Activity → Execution time**. Применить `query_id:-665077864269413128`; выбрать поиск заказов клиента.

```sql
select id, status, total_cents from shop.orders
where customer_id = $1 order by placed_at desc limit $2
```

![SQL statement, интервальные rates и вклад за час](images/statements.png)

| Чтение | Формула |
| --- | --- |
| Вклад execution за час **16.7 min** | Накопленная разность `total_exec_time`, переведённая из миллисекунд в минуты. |
| Интервал **19:00:28 → 19:00:33** | Два записанных наблюдения statement; точные timestamps определяют `Δt`. |
| **120 calls/s** | `Δcalls / Δt`. |
| **1.42 s/s** | `Δtotal_exec_time / (1000×Δt)`. Параллельные длительности исполнения складываются. |
| **11.9 ms/call** | `Δtotal_exec_time / Δcalls`; рассчитывается перед округлением для отображения. |

2. Выбрать **Per call**, **I/O**, **Resources** для интервальных величин. **Stability** показывает записанные Mean/Min/Max/Stddev и их CV за статистический период расширения; [все операнды](metrics-postgresql.ru.md).
3. Нажать **Open plans** и выбрать Plan ID **`1544266440`**. Inspector содержит `Parallel Seq Scan on orders`, `Sort` по `placed_at DESC`, `Gather Merge`, `Limit`, `Workers Planned: 1` и `(customer_id = 4244)`.
4. Использовать **Related statements** или Back браузера. Открыть **Tables**, применить `schema:shop AND table_name:orders`, выбрать **Access**, **Size and buffers**, затем Indexes relation.

![Записанный plan text и связанный query](images/plans.png)

В этой записи Plans Activity завершается ошибкой из-за повторяющихся записанных identities `pg_store_plans`; таблица Plans и показанный Inspector выбранного plan работают.

## 4. Цепочка blockers и состояние backend

[Locks, 19:00:33 UTC](https://vadv.github.io/kronika-reports/reports/kronika-demo-hour-b3ac3ee.html?at=1788634833931637&view=pg.locks)

![Корневой holder и два waiters](images/locks.png)

| Записанное поле | Значение |
| --- | --- |
| Корневой PID / state | **3765**, `idle in transaction`. |
| Waiting PIDs / state | **4761**, **4762**, `active`. |
| Wait type / event | `Lock` / `transactionid`. |
| Mode / target | `ShareLock` / transaction **4700**. |
| Wait starts | **19:00:19** у обоих waiters. |
| Application | `checkout-api`. |

1. Выбрать waiting-строку PID 4761; прочитать точные blocker PIDs, target и backend text в Inspector. Выбрать корневую строку для её state/context.
2. Открыть **Activity** вручную при сохранённом cursor и применить `pid:4761`. Query time = `sample−query_start` для `active`; transaction time = `sample−xact_start`; time in state = `sample−state_change`, кроме точного `idle`.
3. Открыть **Events**, выбрать lock waits и представительную запись с logged wait duration в миллисекундах и текстом holder PIDs.
4. Перейти к следующему записанному моменту через →. Activity и Locks выбирают наблюдения своих источников.

## Дополнительные операции

| Последовательность | Точное чтение |
| --- | --- |
| Tables → schema/database/tablespace group → объект → Maintenance / Freeze | Агрегация групп и counters обслуживания, timestamps, XID ages каждой таблицы; [формулы PostgreSQL](metrics-postgresql.ru.md). |
| Vacuum → episode → phase / Process | Записанные серии фаз, доля просканированного heap и разности процесса с привязкой к границам episode. |
| Host → Storage → I/O / Filesystems / Topology | Rates counters устройства, gauges ёмкости mount, записанные связи устройств. |
| PostgreSQL → Overview → `pg_wal` | Текущий размер каталога с историей; генерация WAL имеет отдельный counter rate. |
| Events → источник → группа → minute strip | Суммарный вес occurrences этой минуты, затем представительный текст. |
| Export → This hour / From / To → Download | Включительный диапазон целых секунд и автономный HTML, [входы Export](features.ru.md#export). |
| Connect an AI agent → клиент → Copy | Конфигурация подключения клиента к MCP записанных данных, [входы tools](features.ru.md#mcp). |
