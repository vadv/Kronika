# PgBouncer log sections

Proposed registry types for what a PgBouncer log carries. Every field below was
taken from the PgBouncer sources, not from its documentation: `src/stats.c`,
`src/objects.c`, `src/janitor.c`, `src/client.c`, `src/server.c`, `src/admin.c`.

The collector reads the log file. It does not connect to the admin console.
`log_stats` defaults to `1` and `stats_period` to `60`, so the numbers below
arrive on a default install with no configuration and no credentials.

The line prefix is fixed, unlike PostgreSQL's configurable `log_line_prefix`:

```
2026-08-07 12:34:56.789 UTC [12345] LOG C-0x55f1: db/user@10.0.0.1:5432 closing because: query timeout (age=42s)
```

Timestamp, pid, level, socket handle, then `database/user@address:port` when the
line belongs to a connection.

## `pgbouncer_stats`

One row per `stats:` line, `src/stats.c:399`. Averages over `stats_period`,
computed by PgBouncer itself.

| Column | Class | Unit | Source field |
|---|---|---|---|
| `ts` | t | — | line timestamp |
| `xacts_per_sec` | g | per_second | `xacts/s` |
| `queries_per_sec` | g | per_second | `queries/s` |
| `client_parses_per_sec` | g | per_second | `client parses/s` |
| `server_parses_per_sec` | g | per_second | `server parses/s` |
| `binds_per_sec` | g | per_second | `binds/s` |
| `client_logins_per_sec` | g | per_second | `client logins/s` |
| `bytes_in_per_sec` | g | bytes_per_second | `in B/s` |
| `bytes_out_per_sec` | g | bytes_per_second | `out B/s` |
| `avg_xact_us` | g | microseconds | `xact us` |
| `avg_query_us` | g | microseconds | `query us` |
| `avg_wait_us` | g | microseconds | `wait us` |

`avg_wait_us` is how long a client waited for a server connection. It is the
number that says whether the pool is the bottleneck.

Known limit: the log line is process-wide. `SHOW STATS` breaks the same numbers
down per database; the log does not.

## `pgbouncer_pool_full`

One row per refusal to open or accept a connection. Sites in `src/objects.c`.

| Column | Class | Unit | Nullable |
|---|---|---|---|
| `ts` | t | — | no |
| `limit_kind` | l | — | no |
| `database` | l | — | yes |
| `username` | l | — | yes |
| `current` | g | count | yes |
| `limit_value` | g | count | yes |

`limit_kind` takes the value the source distinguishes:

| Value | Log text |
|---|---|
| `pool` | `launch_new_connection: pool full (%d >= %d)` |
| `database` | `launch_new_connection: database '%s' full (%d >= %d)` |
| `user` | `launch_new_connection: user '%s' full (%d >= %d)` |
| `peer_pool` | `launch_new_connection: peer pool full (%d >= %d)` |
| `max_client_conn` | `no more connections allowed (max_client_conn)` |
| `max_db_client_connections` | `client connections exceeded (max_db_client_connections)` |
| `max_user_client_connections` | `client connections exceeded (max_user_client_connections)` |
| `too_many_servers` | `too many servers in the pool` |

`current` and `limit_value` are null for the three that carry no numbers.

## `pgbouncer_disconnect`

One row per `closing because: %s (age=%llus)`, `src/objects.c:1363` for the
server side and `:1481` for the client side.

| Column | Class | Unit | Nullable |
|---|---|---|---|
| `ts` | t | — | no |
| `side` | l | — | no |
| `reason` | l | — | no |
| `database` | l | — | yes |
| `username` | l | — | yes |
| `peer_addr` | l | — | yes |
| `age_s` | g | seconds | no |

`reason` is the string PgBouncer printed, interned. The distinct values are the
categories; no classification is added on top. The ones an operator acts on:

- Timeouts: `query timeout`, `query_wait_timeout`, `idle transaction timeout`,
  `transaction timeout`, `client_idle_timeout`, `client_login_timeout`,
  `client_login_timeout (server down)`, `connect timeout`, `cancel_wait_timeout`,
  `suspend_timeout`
- Server recycling: `server lifetime over`, `server idle timeout`,
  `idle server got dirty`, `SV_IDLE server got dirty`, `SV_USED server got dirty`
- Server trouble: `server conn crashed?`, `server connection closed`,
  `server shutting down`, `server DNS lookup failed`, `connect failed`,
  `cannot connect`, `test query failed`, `reset query failed`,
  `exec_on_connect query failed`,
  `server login has been failing, cached error: %s (server_login_retry)`
- Client trouble: `client unexpected eof`, `client disconnected with query in
  progress`, `client disconnect before everything was sent to the server`,
  `client disconnect while server was not ready`
- Configuration: `connect string changed`, `database configuration changed`,
  `obsolete connection`, `evicted`, `pause mode`, `pooler is shutting down`

## `pgbouncer_events`

Everything the other three do not cover, one row per line.

| Column | Class | Unit | Nullable |
|---|---|---|---|
| `ts` | t | — | no |
| `level` | l | — | no |
| `database` | l | — | yes |
| `username` | l | — | yes |
| `peer_addr` | l | — | yes |
| `detail` | l | — | no |

`level` is `LOG`, `WARNING`, `ERROR` or `FATAL`. `detail` is the message text,
interned, so repeated messages cost one dictionary entry.

No `kind` column. The message text is the category, distinct texts already
deduplicate through the string dictionary, and a taxonomy maintained by hand
goes stale against a moving upstream.

What lands here:

- Authentication: `password authentication failed`, `SASL authentication
  failed`, `LDAP authentication failed`, `PAM authentication failed`,
  `certificate authentication failed`, `no such user`, `no such database: %s`,
  `login rejected`, `unix socket login rejected`, `empty password returned by
  client`, `no authentication method is found`, `broken auth file`
- Auth query: `error response from auth_query`, `unable to send auth_query`,
  `unexpected response from auth_query`, `auth_query response contained null
  user name`, `expected 2 columns from auth_query, not %hu`
- Resources: `out of memory`, `no memory for pool`, `no memory for
  authentication pool`, `bouncer resources exhaustion`,
  `unable to allocate new user for auth`
- Protocol: `bad packet`, `bad packet header`, `failed to parse packet`,
  `unknown pkt`, `unknown pkt from server`, `broken Bind/Parse/Describe/Close
  packet`, `old V2 protocol not supported`, `invalid startup packet layout`,
  `client re-sent startup pkt`, `PQexec disallowed`,
  `transaction blocks not allowed in statement pooling mode`
- Prepared statements: `prepared statement did not exist`,
  `prepared statement name is already in use`
- TLS: `TLS handshake error`, `TLS accept error`, `TLS connect error`,
  `TLS startup failed`, `server refused SSL`, `SSL required`,
  `SSL req inside SSL`, `received unencrypted data after SSL request`,
  `TLS configuration could not be reloaded, keeping old configuration`
- Cancel requests: `bad cancel key`, `bad cancel request`, `failed cancel
  request`, `failed to forward cancel request because its TTL was exhausted`,
  `could not find peer to forward request to`
- Admin: `PAUSE command issued`, `RELOAD command issued`, `KILL command issued`,
  `RELOAD failed, see logs for additional details`, `admin forced disconnect`
- Lifecycle: `pooler is shutting down`, `client connections dropped, exiting`,
  `server connections dropped, exiting`, `database removed`, `peer removed`,
  `cleaning up idle pool for user %s on db %s because: pool idle timeout`,
  `launching new connection to satisfy min_pool_size`

## New units

`per_second` and `bytes_per_second` do not exist in the registry yet. Both get
consumers here.

## Not collected

`SHOW STATS`, `SHOW POOLS`, `SHOW CLIENTS` over the admin console. The `stats:`
line carries the same aggregate numbers without a connection, a password, or a
grant on the admin database. If a specific number turns out to be missing from
the log, that is when the console earns its place.
