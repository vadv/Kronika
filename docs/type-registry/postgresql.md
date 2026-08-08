# Class 2: PostgreSQL log events

Log events occupy `2_001_001`–`2_099_999`. The schemas are declared in
[`crates/kronika-registry/src/codec/pg_log.rs`](../../crates/kronika-registry/src/codec/pg_log.rs);
this file says what the sections are for, which lines produce them, and what
the collector cannot see.

The log to follow is `KRONIKA_PG_LOG`. Nothing is followed unless it is set.

## Registered types

| `type_id` | Section | Semantics | Sort key |
|-----------|---------|-----------|----------|
| `2_001_001` | `pg_log_errors` | `event_stream` | `(severity, category, pattern, ts)` |
| `2_002_001` | `pg_log_checkpoints` | `event_stream` | `(ts, phase)` |
| `2_003_001` | `pg_log_autovacuum` | `event_stream` | `(ts, kind, relation)` |
| `2_004_001` | `pg_log_slow_queries` | `event_stream` | `(pattern, ts)` |
| `2_005_001` | `pg_log_lock_waits` | `event_stream` | `(ts, kind, pid)` |
| `2_006_001` | `pg_log_lifecycle` | `event_stream` | `(ts, kind)` |
| `2_007_001` | `pg_log_temp_files` | `event_stream` | `(ts, size_bytes)` |

## The three formats

`log_destination` decides the shape of a record, and the file name decides how
the collector reads it: `.csv` is `csvlog`, `.json` is `jsonlog`, anything else
is `stderr`.

**`csvlog`** has the same 23 columns in every release since PG 9.0; PG 13 and
PG 14 appended `backend_type`, `leader_pid` and `query_id` after them. The
collector reads by position and ignores what a newer release appends next. A
statement with a newline in it spans lines, and a record continues while a
quoted field is still open.

**`jsonlog`** exists from PG 15 and is one object per line. Newlines inside a
value are escaped, so a record never spans lines.

**`stderr`** has no fixed shape: what precedes `SEVERITY:` is whatever
`log_line_prefix` says. The setting is read from `pg_settings` over
`KRONIKA_PG_DSN`, because it is the server's setting and declaring it a second
time in the collector's environment would be a second place to get it wrong.
Without a connection the time, severity and message are still read; the
database and user, which only the prefix carries, are `NULL`.

A `stderr` record's `DETAIL:`, `HINT:`, `CONTEXT:`, `STATEMENT:` and `QUERY:`
lines are separate lines carrying the prefix again. A line the server wrapped
inside one of them starts with a tab. `QUERY:` and `STATEMENT:` both land in
`statement`: both name the SQL the record was raised under.

## Grouping

Two sections arrive grouped, because a log that repeats one message ten
thousand times says what a count of ten thousand says and costs ten thousand
rows to say it.

`pg_log_errors` groups on `(severity, category, pattern)`, where `pattern` is
the message with its values replaced: quoted names, parenthesized and bracketed
lists, the number after `transaction`/`relation`/`process`/`PID`/`signal`, and
WAL addresses. `sample` keeps the first occurrence with its values intact.

`pg_log_slow_queries` groups on the statement with its literals replaced as
well, and keeps the slowest occurrence as `sample` with its duration.

The window is one read. A group that spans two reads is two rows, which is what
`event_stream` means.

## Which records produce rows

Everything at `WARNING` and above becomes an error group. At `LOG`, only the
six shapes below produce rows, and a `LOG` record that matches none of them is
dropped: on a default install the log also carries a line per connection
authorized, which is traffic, not an event.

| Section | The record it reads | Setting that produces it |
|---|---|---|
| `pg_log_checkpoints` | `checkpoint starting:`, `checkpoint complete:`, `checkpoints are occurring too frequently` | `log_checkpoints`, on by default from PG 15 |
| `pg_log_autovacuum` | `automatic vacuum of table`, `automatic analyze of table` | `log_autovacuum_min_duration` |
| `pg_log_slow_queries` | `duration: <ms> ms  statement: <sql>` | `log_min_duration_statement` |
| `pg_log_lock_waits` | `process <pid> still waiting for`, `process <pid> acquired` | `log_lock_waits` |
| `pg_log_lifecycle` | `server process (PID …) was terminated`, `received … shutdown request`, `database system is ready to accept connections` | always |
| `pg_log_temp_files` | `temporary file: path …, size …` | `log_temp_files` |

## Error categories

`category` is the first family whose phrases the pattern matches, so a deadlock
reported alongside a permission problem counts as a lock.

| Code | Category | What it covers |
| ---: | --- | --- |
| `0` | lock | deadlocks, lock timeouts, waits |
| `1` | constraint | duplicate keys, foreign keys, not-null, check, exclusion |
| `2` | serialization | `could not serialize access` |
| `3` | timeout | statement, transaction and idle-session timeouts, cancellation |
| `4` | resource | memory, connection slots, disk, and an OOM kill |
| `5` | data corruption | bad pages, unreadable blocks, every `PANIC` |
| `6` | system | files, I/O, crashes, and any otherwise uncategorized `FATAL` |
| `7` | connection | resets, unexpected EOF, broken pipes |
| `8` | auth | password failures, `pg_hba.conf`, permissions |
| `9` | syntax | syntax errors, missing objects, bad input |
| `10` | other | everything else |

## What the collector cannot see

- **A localized log.** The messages above are matched in English. A server with
  `lc_messages` set to another locale still yields error groups, but its
  patterns are that locale's text and no `LOG` record is recognized as a typed
  shape.
- **The extended protocol's slow statements.** `duration: … ms  execute …` and
  `bind …` are not read; only `statement:` is.
- **A record older than the collector's first read.** A log file that already
  exists when the collector starts is read from its beginning; one that has
  been read before resumes at its offset.
- **A `log_timezone` that differs from the host's.** Both `PostgreSQL` and the
  collector print and read a local wall clock, and the collector reads it in the
  host's timezone. A server configured with a different one shifts every
  event's `ts` by the difference.

## Bounds

A line longer than 64 KiB is cut there and the rest of it dropped. A stored
message, statement or continuation is cut at 5 KiB, a grouping pattern at
256 bytes. One read takes at most 4 MiB from a file; the rest waits for the
next tick.
