# Standalone report fixture

`standalone.zms` is one finished segment written through `kronika-writer` with
the rich query parity rows used by `kronika-query`: metadata, CPU, process,
relations, activity, locks, vacuum, database, statement, plan, and one
`pg_log_errors` occurrence. Its explicit test identity is
`1709164800000000`.

`standalone.idx` is the canonical isolated index produced for that exact ZMS by
`kronika_index::build_from_reader` and `Index::encode`. No preceding segment
participated, so the pair exercises the standalone report contract and permits
the first predecessor-derived point to remain unavailable.
