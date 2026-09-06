# Измерения справочника пользователей процессов

[English version](process-user-measurements.md)

Исторические измерения sections `os_user` и `dict.strings` на Linux
6.17.10-100.fc41.x86_64, AMD Ryzen 9 8945HS, optimized build, process CPU clock
100 Hz. Дата измерения, source revision, compiler version, число повторных
запусков и вариативность вместе с этими результатами не записаны.

Воспроизведение текущим toolchain репозитория:

```bash
CARGO_BUILD_TARGET=x86_64-unknown-linux-gnu cargo test --release -p kronika-collector process_user_references_report_production_storage_and_resource_costs -- --nocapture --test-threads=1
```

Тест выполняет три случая последовательно в одном изолированном дочернем
тестовом процессе. Артефакты содержат user references и их string dictionary;
process rows отсутствуют. Источник: [`user_cost_artifact` и тест измерений](../bins/kronika-collector/src/tests/zms.rs).

| Случай | Наблюдения UID | Строки справочника | Сырое тело `os_user` | Сырое тело словаря | Сырой WAL | Готовый `os_user` | Готовый словарь | Добавленные байты готового сегмента | Весь ZMS |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| 1 000 процессов с одним UID на 120 тактах | 120 000 | 1 | 1 171 B | 463 B | 1 794 B | 517 B | 245 B | 826 B | 870 B |
| Несколько обычных UID | 16 | 16 | 1 343 B | 648 B | 2 151 B | 694 B | 428 B | 1 186 B | 1 230 B |
| Максимум различных наблюдаемых UID и имён | 4 096 | 4 096 | 55 368 B | 43 684 B | 99 212 B | 42 414 B | 43 219 B | 85 697 B | 85 741 B |

`Добавленные байты = готовое тело os_user + готовое тело словаря + 2 × catalog_entry_bytes`.
Для общего UID: `517 + 245 + 2 × 32 = 826 B`; полный ZMS занимает `870 B`.
Из 120,000 наблюдений UID получается одна mapping row. Размеры WAL и полного
ZMS включают file framing и metadata.

| Измерение | Общий UID | 16 UIDs | 4,096 UIDs |
|---|---:|---:|---:|
| Capture elapsed, µs | 131 | 5 | 1,240 |
| Writer elapsed, µs | 9,248 | 7,017 | 10,630 |
| Test-process peak RSS, KiB | 15,832 | 17,048 | 20,496 |
| Прирост peak RSS при capture, KiB | 128 | 0 | 0 |

Capture elapsed — сумма `Instant::elapsed().as_micros()` вокруг `prepare_rows`
для каждого sample. Writer elapsed суммирует encoding и WAL append, затем
запись итогового segment. Проверка reader/dictionary выполняется после timed
writer block. Частота 100 Hz относится к отдельному process CPU-time counter;
она не задаёт разрешение elapsed measurements через `Instant`.

Peak RSS — high-water mark тестового процесса, включая harness, allocator state
от предыдущих случаев и Parquet writer. Значение 25,600 KiB в design проекта —
collector RSS budget; это не runtime memory cap.

Тест проверяет одну row на recorded UID, отсутствие row для unresolved UID и
отклонение oversized passwd source. Ошибочная passwd line может соседствовать
с сохранённой корректной line. Отдельные recovery и forced-rollover tests
проверяют segment-local mappings и dictionary resolution. Источник записанных
имён — `/etc/passwd`; identities, доступные только через NSS, LDAP или SSSD,
остаются числовыми. Первое записанное UID mapping остаётся неизменным в пределах
segment.
