#!/usr/bin/env bash
# Build and run the demo image.
#
#   ./scripts/demo-image.sh deps-key   # cache key of the dependency layer
#   ./scripts/demo-image.sh build      # build the image
#   ./scripts/demo-image.sh run        # build, then run it
#
# The dependency layer is keyed on the manifests and the lockfile, so a
# source-only change reuses it.
set -euo pipefail

cd "$(dirname "$0")/.."

IMAGE="${DEMO_IMAGE:-kronika-demo:local}"
PORT="${DEMO_PORT:-8080}"

deps_key() {
	# Same inputs the dependency stage copies.
	{
		cat Cargo.toml Cargo.lock rust-toolchain.toml
		find crates bins -name Cargo.toml -print0 | sort -z | xargs -0 cat
	} | sha256sum | cut -d' ' -f1
}

case "${1:-run}" in
deps-key)
	deps_key
	;;
build)
	docker build --file Dockerfile.demo --tag "$IMAGE" .
	;;
run)
	docker build --file Dockerfile.demo --tag "$IMAGE" .
	args=(--rm -p "${PORT}:8080")
	if [ -n "${DEMO_DATA_DIR:-}" ]; then
		args+=(-v "${DEMO_DATA_DIR}:/var/lib/kronika/data")
	fi
	docker run "${args[@]}" "$IMAGE"
	;;
*)
	echo "usage: $0 {deps-key|build|run}" >&2
	exit 2
	;;
esac
