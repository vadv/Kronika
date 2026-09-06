# Standalone report fixture

[Русская версия](README.ru.md)

`standalone.zms` is one finished segment written through `kronika-writer` with
the rich query parity rows used by `kronika-query`: metadata, CPU, process,
relations, activity, locks, vacuum, database, statement, plan, and one
`pg_log_errors` occurrence. Its explicit test identity is
`1709164800000000`.

`standalone.idx` is the canonical isolated index produced for that exact ZMS by
`kronika_index::build_from_reader` and `Index::encode`. No preceding segment
participated, so the pair exercises the standalone report contract and permits
the first predecessor-derived point to remain unavailable.

The ZMS SHA-256 is
`ba8dd3deae058dfd1580e81b4abc534dee71688b708347db09b6877eeecac58e`; the IDX
SHA-256 is
`33d48ba4dc4726fd80dd8901f1956baaedf0de1639b01f65349c9e381b992033`.
The integration test reads a non-null `transactions_per_second` point from the
IDX and rows and events from the ZMS, in addition to byte-for-byte comparison
with the direct query composition.
