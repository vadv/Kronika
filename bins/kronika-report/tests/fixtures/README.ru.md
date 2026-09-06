# Fixture автономного отчёта

[English version](README.md)

`standalone.zms` — завершённый segment, записанный через `kronika-writer`.
Он содержит строки rich query parity из `kronika-query`: metadata, CPU, process,
relations, activity, locks, vacuum, database, statement, plan и одно событие
`pg_log_errors`. Явный test identity — `1709164800000000`.

`standalone.idx` — canonical isolated index для этого ZMS, построенный
`kronika_index::build_from_reader` и `Index::encode`. Предыдущий segment
не участвует: fixture проверяет автономный отчёт и допускает отсутствие первой
точки, вычисляемой по предыдущему sample.

SHA-256 ZMS:
`ba8dd3deae058dfd1580e81b4abc534dee71688b708347db09b6877eeecac58e`.
SHA-256 IDX:
`464497ed3528ddb5628086362f978c15d621d9bd07e268985337735a137c451d`.
Integration test читает точку `transactions_per_second` со значением, отличным от null, из IDX,
строки и события из ZMS и побайтно сравнивает результат с прямой query composition.
