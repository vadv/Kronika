#!/usr/bin/env bash
set -euo pipefail

repo_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
crate_root="$repo_root/crates/kronika-query"
source_root="$crate_root/src"
cargo_bin=${CARGO_BIN:-cargo}
rust_toolchain=${RUST_TOOLCHAIN:-1.96.0}
failed=0

if rg -n -i '\bproduct\b' "$crate_root"; then
    echo "kronika-query must not use product naming" >&2
    failed=1
fi

if rg -n \
    'std::(fs|path)(::|\b)|\b(Path|PathBuf|File|LocalDir|Instant)\b|\b(hyper|tokio|rmcp|http)(::|_)' \
    "$source_root"; then
    echo "kronika-query source crosses its native storage/transport boundary" >&2
    failed=1
fi

dependency_names=$(
    "$cargo_bin" "+$rust_toolchain" tree --locked -p kronika-query \
        --edges normal --prefix none |
        sed -E 's/ v[0-9].*$//' |
        sort -u
)
if printf '%s\n' "$dependency_names" |
    rg '^(http|http-body.*|hyper.*|tokio.*|rmcp.*)$'; then
    echo "kronika-query dependency graph contains a native transport/runtime crate" >&2
    failed=1
fi

exit "$failed"
