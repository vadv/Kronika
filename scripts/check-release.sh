#!/usr/bin/env bash
set -euo pipefail

if [[ $# -ne 1 || ! -f "$1" ]]; then
  echo "Usage: scripts/check-release.sh ARCHIVE.tar.gz" >&2
  exit 2
fi
repo=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
archive=$(realpath "$1")
(cd "$(dirname "$archive")" && sha256sum --check "$(basename "$archive").sha256")
release_tmp=$(mktemp -d "${TMPDIR:-/tmp}/kronika-release-check.XXXXXX")
trap 'rm -rf -- "$release_tmp"' EXIT
tar -xzf "$archive" -C "$release_tmp"
package="$release_tmp/$(basename "$archive" .tar.gz)"
(cd "$package" && sha256sum --check SHA256SUMS)
for required in kronika-collector kronika-web kronika-dump kronika-report LICENSE INSTALL.md INSTALL.ru.md BUILDINFO; do
  test -s "$package/$required"
done

python3 - "$repo" "$package" "$release_tmp" <<'PY'
import base64
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

repo, package, scratch = map(Path, sys.argv[1:])
env = {k: v for k, v in os.environ.items() if not k.startswith('KRONIKA_')}
collector = subprocess.run([package / 'kronika-collector'], env=env,
                           capture_output=True, text=True, timeout=10)
assert collector.returncode != 0 and 'KRONIKA_STORAGE_DIR' in collector.stderr
store = scratch / 'data'
segment_dir = store / '2024/02/29'
segment_dir.mkdir(parents=True)
fixture = repo / 'bins/kronika-report/tests/fixtures/standalone.zms'
shutil.copyfile(fixture, segment_dir / '1709164800000000.zms')
dump = subprocess.run([package / 'kronika-dump', store, '--json'], env=env,
                      capture_output=True, text=True, check=True, timeout=30)
assert dump.stdout.strip(), 'dump produced no section sizes'
slice_env = dict(env, KRONIKA_STORAGE_DIR=str(store))
subprocess.run([package / 'kronika-dump', 'slice',
                '--from', '2024-02-29T00:00:00Z', '--to', '2024-02-29T00:59:59Z',
                '--out', scratch / 'slice.zms'], env=slice_env, check=True, timeout=30)
for output in ('report.html', 'report-again.html'):
    subprocess.run([package / 'kronika-report',
                    '--from', '1709164800000000', '--to-exclusive', '1709168400000000',
                    scratch / 'slice.zms', scratch / output],
                   env=env, check=True, timeout=30)
assert (scratch / 'report.html').read_bytes() == (scratch / 'report-again.html').read_bytes()

with socket.socket() as probe:
    probe.bind(('127.0.0.1', 0))
    port = probe.getsockname()[1]
web_env = dict(env, KRONIKA_STORAGE_DIR=str(store),
               KRONIKA_WEB_LISTEN=f'127.0.0.1:{port}', KRONIKA_WEB_SOURCES='3',
               KRONIKA_WEB_USER='package-check', KRONIKA_WEB_PASSWORD='local-package-check')
base = f'http://127.0.0.1:{port}'
headers = {'Authorization': 'Basic ' + base64.b64encode(
    b'package-check:local-package-check').decode()}
with (scratch / 'web.log').open('w+') as log:
    web = subprocess.Popen([package / 'kronika-web'], env=web_env, stdout=log, stderr=log)
    try:
        for attempt in range(100):
            if web.poll() is not None:
                log.seek(0)
                raise RuntimeError(log.read())
            try:
                with urllib.request.urlopen(base, timeout=1) as response:
                    assert b'<html' in response.read().lower()
                break
            except (urllib.error.URLError, TimeoutError):
                time.sleep(0.1)
        else:
            raise RuntimeError('web did not become ready')
        try:
            urllib.request.urlopen(base + '/api/catalog', timeout=10)
        except urllib.error.HTTPError as error:
            assert error.code == 401
        else:
            raise AssertionError('catalog accepted missing credentials')
        request = urllib.request.Request(base + '/api/catalog', headers=headers)
        with urllib.request.urlopen(request, timeout=10) as response:
            assert b'os_process' in response.read()
        rpc_headers = dict(headers, **{'Content-Type': 'application/json',
                                      'Accept': 'application/json, text/event-stream'})
        request = urllib.request.Request(base + '/mcp', headers=rpc_headers,
            data=b'{"jsonrpc":"2.0","id":1,"method":"tools/list","params":{}}')
        with urllib.request.urlopen(request, timeout=10) as response:
            tools = json.load(response)['result']['tools']
            assert any(tool['name'] == 'kronika_rank_metrics' for tool in tools)
    finally:
        if web.poll() is None:
            web.terminate()
        try:
            web.wait(timeout=10)
        except subprocess.TimeoutExpired:
            web.kill()
            web.wait()
print('Package smoke passed: collector configuration, dump, slice, deterministic report, web auth, MCP')
PY
"$repo/scripts/report-browser-smoke.sh" "$release_tmp/report.html"
