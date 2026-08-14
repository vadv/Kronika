#!/usr/bin/env bash
# Supervises PostgreSQL, PgBouncer, kronika-web, and kronika-demo (which
# itself owns the collector and the optional workload) inside the demo
# container.
#
# Plain and flat on purpose: a died process is a died process, logged and
# the container exits, not retried. There is exactly one place shutdown is
# decided.
#
# Known limitation, documented rather than engineered around: this only
# tracks kronika-web and kronika-demo as job-controlled children. If
# PostgreSQL or PgBouncer itself dies, the container keeps running; the
# collector and the workload surface that as repeated logged connection
# errors instead of a container exit. A health-check loop for a demo aid
# would be more machinery than the problem is worth.
set -euo pipefail

PG_DATA=/var/lib/kronika/pgdata
PG_USER=postgres
PG_PORT=5432
PGB_DIR=/var/lib/kronika/pgbouncer
PGB_PORT=6432
OUT_DIR=${KRONIKA_OUT_DIR:-/var/lib/kronika/data}

pg_bin() {
	local version
	version=$(find /usr/lib/postgresql -mindepth 1 -maxdepth 1 -printf '%f\n' | sort -n | tail -n1)
	echo "/usr/lib/postgresql/${version}/bin"
}

BIN=$(pg_bin)

start_postgres() {
	rm -rf "$PG_DATA"
	mkdir -p "$PG_DATA"
	chown "$PG_USER" "$PG_DATA"
	su "$PG_USER" -c "$BIN/initdb --pgdata=$PG_DATA --auth=trust --username=$PG_USER --encoding=UTF8 --no-sync"
	cat >>"$PG_DATA/postgresql.conf" <<-EOF
		listen_addresses = '127.0.0.1'
		port = $PG_PORT
		max_connections = 300
		logging_collector = on
		log_destination = 'stderr'
		log_directory = 'log'
		log_filename = 'postgresql.log'
		log_checkpoints = on
		log_lock_waits = on
		log_temp_files = 0
		log_min_duration_statement = 0
		fsync = off
	EOF
	su "$PG_USER" -c "$BIN/pg_ctl --pgdata=$PG_DATA --wait --timeout=60 --log=$PG_DATA/startup.log start"
}

start_pgbouncer() {
	rm -rf "$PGB_DIR"
	mkdir -p "$PGB_DIR"
	chown "$PG_USER" "$PGB_DIR"
	cat >"$PGB_DIR/pgbouncer.ini" <<-EOF
		[databases]
		postgres = host=127.0.0.1 port=$PG_PORT dbname=postgres
		[pgbouncer]
		listen_addr = 0.0.0.0
		listen_port = $PGB_PORT
		auth_type = trust
		stats_users = $PG_USER
		logfile = $PGB_DIR/pgbouncer.log
		pidfile = $PGB_DIR/pgbouncer.pid
		pool_mode = transaction
		max_client_conn = 300
		default_pool_size = 80
	EOF
	su "$PG_USER" -c "pgbouncer -d $PGB_DIR/pgbouncer.ini"
	local attempt
	# shellcheck disable=SC2034 # the loop only needs a bounded attempt count
	for attempt in $(seq 1 100); do
		if su "$PG_USER" -c "$BIN/psql --host=127.0.0.1 --port=$PGB_PORT --username=$PG_USER --dbname=pgbouncer --command 'show version'" >/dev/null 2>&1; then
			return 0
		fi
		sleep 0.2
	done
	echo "demo-entrypoint: pgbouncer never answered on its admin console" >&2
	exit 1
}

mkdir -p "$OUT_DIR"

start_postgres
start_pgbouncer

export KRONIKA_OUT_DIR="$OUT_DIR"
export KRONIKA_PG_DSNS="host=127.0.0.1 port=$PG_PORT user=$PG_USER dbname=postgres"
export KRONIKA_PGBOUNCER_DSNS="host=127.0.0.1 port=$PGB_PORT user=$PG_USER dbname=pgbouncer"
export KRONIKA_POSTGRES_EFFECTIVE_CPUS="${KRONIKA_POSTGRES_EFFECTIVE_CPUS:-$(nproc)}"
export KRONIKA_WEB_LISTEN="${KRONIKA_WEB_LISTEN:-0.0.0.0:8080}"
export KRONIKA_WEB_USER="${KRONIKA_WEB_USER:-demo}"
export KRONIKA_WEB_PASSWORD="${KRONIKA_WEB_PASSWORD:-demo}"
export KRONIKA_WEB_SOURCES="${KRONIKA_WEB_SOURCES:-3}"
export KRONIKA_COLLECTOR_BIN=/usr/local/bin/kronika-collector
export KRONIKA_DEMO_DURATION_S="${KRONIKA_DEMO_DURATION_S:-0}"
export KRONIKA_DEMO_WORKLOAD_DSN="${KRONIKA_DEMO_WORKLOAD_DSN:-host=127.0.0.1 port=$PGB_PORT user=$PG_USER dbname=postgres}"

kronika-web &
WEB_PID=$!
echo "demo-entrypoint: kronika-web pid $WEB_PID"

kronika-demo &
DEMO_PID=$!
echo "demo-entrypoint: kronika-demo pid $DEMO_PID"

shutdown() {
	echo "demo-entrypoint: shutting down"
	kill -TERM "$DEMO_PID" 2>/dev/null || true
	wait "$DEMO_PID" 2>/dev/null || true
	kill -TERM "$WEB_PID" 2>/dev/null || true
	su "$PG_USER" -c "$BIN/pg_ctl --pgdata=$PG_DATA --mode=fast --wait --timeout=30 stop" || true
	exit 0
}
trap shutdown TERM INT

set +e
wait -n "$WEB_PID" "$DEMO_PID"
set -e
if ! kill -0 "$WEB_PID" 2>/dev/null; then
	echo "demo-entrypoint: kronika-web died" >&2
elif ! kill -0 "$DEMO_PID" 2>/dev/null; then
	echo "demo-entrypoint: kronika-demo died" >&2
fi
shutdown
