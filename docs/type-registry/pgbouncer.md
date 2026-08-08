# Class 2: PgBouncer log events

`PgBouncer` occupies `2_100_001`–`2_199_999`. The schema is declared in
[`crates/kronika-registry/src/codec/pgbouncer_events.rs`](../../crates/kronika-registry/src/codec/pgbouncer_events.rs).
Every line below was read out of the `PgBouncer` sources, not out of its
documentation.

A pooler is named either by `KRONIKA_PGBOUNCER_DSNS`, and then `SHOW CONFIG`
says where its `logfile` is, or by `KRONIKA_PGBOUNCER_LOGS`, which takes paths
and patterns. `SHOW CONFIG` needs the account in `stats_users`; nothing else is
asked of the console, and `SHOW POOLS`, `SHOW STATS` and `SHOW CLIENTS` are not
read at all.

**`logfile` is empty by default.** `PgBouncer` then writes to stderr, and under
systemd that goes to the journal without the line layout below. The section
works when `pgbouncer.ini` sets `logfile = <path>`; a pooler asked over its
console says so outright, which is one line in the log rather than a guess.

## `pgbouncer_events`

One row per recognized line.

| Column | Class | Nullable | Meaning |
|---|---|---|---|
| `ts` | t | no | Line time |
| `source_file` | l | no | The file the line was read from, which is the only identity a pooler has |
| `level` | l | no | `0` fatal, `1` error, `2` warning, `3` log, `4` debug, `5` noise |
| `database` | l | yes | The `pgbouncer.ini` section name |
| `username` | l | yes | The login user |
| `host` | l | yes | Client or server address, without the port |
| `text` | l | no | What happened |

There is no `kind` column. The message text is the category, identical texts
already cost one dictionary entry between them, and a taxonomy maintained by
hand goes stale against a moving upstream.

## The line layout

`lib/usual/logging.c:231` writes every line the same way:

```
2026-08-07 12:34:56.789 MSK [12345] LOG C-0x55f1: db/user@10.0.0.1:41537 closing because: query timeout (age=42s)
```

Time, pid, level, then the socket context, then the message. What that costs a
reader:

- **The time is local, not UTC.** `lib/usual/time.c:39,44` calls `localtime_r`
  and prints the zone abbreviation, so the collector reads it in the host's
  timezone.
- **The socket context is not on every line.** Lines from `janitor.c`,
  `main.c` and `pooler.c`, and the `stats:` line, carry no socket. Those rows
  have `database`, `username` and `host` missing.
- **When it is there, nothing in it is empty.** `src/util.c:52` substitutes the
  literals `(nodb)` and `(nouser)`, so those are what the columns hold. A unix
  socket's host is `unix(<pid>)` and its port `0`. A peer connection is written
  `peer-<id>@host:port` and has no database or user at all.
- **The port is not stored.** It is the client's ephemeral port, different on
  every connection, and keeping it would add one dictionary entry per
  connection. Only the host is kept.
- **`db` is not `dbname`.** It is the section name in `pgbouncer.ini`
  (`src/util.c:52`), which may point at a different database on the server.
- **A message with a newline in it is written as `\n\t`**
  (`lib/usual/logging.c:177-189`) and the whole line is cut at 2048 bytes. A
  line starting with a tab continues the line before it.
- **One event can be two lines.** `disconnect_client(notify=true)` writes
  `closing because: <reason>` and then `WARNING pooler error: <reason>` through
  `send_pooler_error` (`src/proto.c:266`), and `log_pooler_errors` is `1` by
  default. The second is dropped; keeping it would count every such event
  twice.
- **The reason is formatted into `char buf[128]`** (`src/objects.c:1349`), so a
  long one arrives cut.
- **`launch_new_connection: … full` is not there on a default install.** Those
  lines are `log_debug` and need `verbose >= 1`.

`text` is the message with the `closing because:` wrapper and the trailing
` (age=Ns)` removed, so a timeout that fires a thousand times costs one
dictionary entry instead of a thousand.

## What is recognized

A line becomes a row when its message, or the reason inside its
`closing because:`, starts with one of these. Everything else is dropped: a
default install also logs a line per connection opened and closed, which is
traffic, not an event.

### The database is not giving out connections

`cannot connect`, `connect failed`, `server conn crashed?`,
`server DNS lookup failed`, `server login failed: <LEVEL> <text>`,
`server login has been failing, cached error: <text> (server_login_retry)`

### The pooler is turning connections away or evicting live ones

`evicted`, `bouncer resources exhaustion`, `out of memory`,
`no memory for pool`, `no memory for authentication pool`,
`too many servers in the pool`, `no more connections allowed (max_client_conn)`,
`client connections exceeded (max_db_client_connections)`,
`client connections exceeded (max_user_client_connections)`

### The queue and the timeouts that empty it

`query_wait_timeout`, `query_timeout` (the client side, `janitor.c:462`),
`query timeout` (the server side, `janitor.c:667` — a different string in a
different place, so both are listed), `idle transaction timeout`,
`transaction timeout`, `cancel_wait_timeout`, `connect timeout`,
`client_login_timeout`, `client_login_timeout (server down)`, `suspend_timeout`

### The server connection cannot be handed out again

`idle server got dirty`, `SV_IDLE server got dirty`, `SV_USED server got dirty`,
`reset query failed`, `test query failed`, `exec_on_connect query failed`,
`var change failed`, `invalid server parameter`

### The pooler process itself

`pooler is shutting down`, `client connections dropped, exiting`,
`server connections dropped, exiting`, `accept() failed`, `cannot listen on`,
`kernel file descriptor limit`, `process up: PgBouncer <version>`,
`TLS configuration could not be reloaded, keeping old configuration`,
`RELOAD Failed, see logs for more details`

### Authentication and protocol

`password authentication failed`, `SASL authentication failed`,
`LDAP authentication failed`, `PAM authentication failed`,
`certificate authentication failed`, `no such user`, `no such database`,
`broken auth file`, `error response from auth_query`, `unable to send
auth_query`, `bad packet`, `bad pkt header`, `failed to parse packet`,
`old V2 protocol not supported`, `TLS handshake error`

## Not collected

The `stats:` line. It carries the averages `PgBouncer` computes over
`stats_period`, and the collector takes events only for now.
