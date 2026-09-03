#!/usr/bin/env bash
# Open a generated report directly from disk and exercise its production UI.
set -euo pipefail

cd "$(dirname "$0")/.."

if [[ $# -ne 1 || ! -f $1 ]]; then
	echo "usage: $0 REPORT.html" >&2
	exit 2
fi

NODE=${NODE_BIN:-node}
report=$(realpath "$1")
page_url=$("$NODE" --input-type=module --eval \
	'import { pathToFileURL } from "node:url"; process.stdout.write(pathToFileURL(process.argv[1]).href)' \
	"$report")

browser=${CHROME_BIN:-}
if [[ -z $browser ]]; then
	for candidate in chromium-browser chromium google-chrome-stable google-chrome; do
		if command -v "$candidate" >/dev/null 2>&1; then
			browser=$candidate
			break
		fi
	done
fi
if [[ -z $browser ]]; then
	echo "no Chromium executable found; set CHROME_BIN" >&2
	exit 1
fi

profile=$(mktemp -d /tmp/kronika-report-browser.XXXXXX)
diagnostics=$profile/chromium.log
browser_pid=
browser_pgid=
cleanup() {
	if [[ $browser_pgid =~ ^[1-9][0-9]*$ ]]; then
		kill -- "-$browser_pgid" 2>/dev/null || true
	elif [[ $browser_pid =~ ^[1-9][0-9]*$ ]]; then
		kill "$browser_pid" 2>/dev/null || true
	fi
	if [[ $browser_pid =~ ^[1-9][0-9]*$ ]]; then
		wait "$browser_pid" 2>/dev/null || true
	fi
	rm -rf -- "$profile"
}
trap cleanup EXIT

setsid "$browser" \
	--headless \
	--disable-background-networking \
	--disable-component-update \
	--disable-default-apps \
	--disable-dev-shm-usage \
	--disable-extensions \
	--disable-gpu \
	--metrics-recording-only \
	--no-first-run \
	--no-sandbox \
	--remote-debugging-address=127.0.0.1 \
	--remote-debugging-port=0 \
	--user-data-dir="$profile" \
	about:blank \
	2>"$diagnostics" &
browser_pid=$!
browser_pgid=$(ps -o pgid= -p "$browser_pid" | tr -d ' ')
if [[ $browser_pgid != "$browser_pid" ]]; then
	echo "Chromium did not start in its own process group" >&2
	exit 1
fi

debug_port=
for _attempt in $(seq 1 250); do
	if [[ -s $profile/DevToolsActivePort ]]; then
		IFS= read -r debug_port <"$profile/DevToolsActivePort"
		break
	fi
	if ! kill -0 "$browser_pid" 2>/dev/null; then
		tail -100 "$diagnostics" >&2
		exit 1
	fi
	sleep 0.04
done
if [[ ! $debug_port =~ ^[0-9]+$ ]]; then
	tail -100 "$diagnostics" >&2
	echo "timed out starting Chromium" >&2
	exit 1
fi

"$NODE" bins/kronika-web/ui/tests/browser-smoke.mjs "$debug_port" "$page_url"
