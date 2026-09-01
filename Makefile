RUST_TOOLCHAIN ?= 1.96.0
TARGET ?= $(shell rustc +$(RUST_TOOLCHAIN) -vV | sed -n 's/^host: //p')
CARGO_BUILD = cargo +$(RUST_TOOLCHAIN) build --locked --target $(TARGET)
UI_DIR = bins/kronika-web/ui

.PHONY: build collector demo web ui-install ui-build ui-check fmt fmt-check lint test bdd-check check test-bdd demo-run demo-image demo-image-run demo-up demo-stop demo-clean demo-status demo-logs diagrams

build: ## Build every binary for the selected target.
	@$(CARGO_BUILD) -p kronika-collector -p kronika-dump -p kronika-demo -p kronika-web

collector: ## Build kronika-collector.
	@$(CARGO_BUILD) -p kronika-collector

demo: ## Build kronika-demo.
	@$(CARGO_BUILD) -p kronika-demo

web: ## Build kronika-web from the committed interface artifact.
	@$(CARGO_BUILD) -p kronika-web

ui-install: ## Install the locked interface build dependencies.
	@npm --prefix $(UI_DIR) ci

ui-build: ## Rebuild the committed self-contained interface artifact.
	@RUST_TOOLCHAIN=$(RUST_TOOLCHAIN) npm --prefix $(UI_DIR) run build

ui-check: ## Type-check and reproduce the committed interface artifact.
	@npm --prefix $(UI_DIR) run fixture:check
	@npm --prefix $(UI_DIR) test
	@npm --prefix $(UI_DIR) run typecheck
	@RUST_TOOLCHAIN=$(RUST_TOOLCHAIN) npm --prefix $(UI_DIR) run check

fmt: ## Format the workspace.
	@cargo +$(RUST_TOOLCHAIN) fmt --all

fmt-check: ## Verify workspace formatting without changing files.
	@cargo +$(RUST_TOOLCHAIN) fmt --all --check

lint: ## Run clippy over the workspace with warnings denied.
	@cargo +$(RUST_TOOLCHAIN) clippy --locked --workspace --all-targets -- -D warnings

test: ## Run every non-BDD unit and integration test.
	@cargo +$(RUST_TOOLCHAIN) test --locked --workspace --exclude kronika-bdd --all-targets

bdd-check: ## Compile the BDD runner and the binaries it exercises, without running scenarios.
	@cargo +$(RUST_TOOLCHAIN) build --locked -p kronika-collector -p kronika-dump -p kronika-bdd

check: ui-check fmt-check lint test bdd-check ## The full local pre-commit gate.

test-bdd: ## Run BDD inside the cached Docker image.
	@scripts/bdd-image.sh run

demo-run: ## Run the collector for a bounded window and report its cost.
	@$(CARGO_BUILD) -p kronika-collector -p kronika-demo
	@KRONIKA_COLLECTOR_BIN=target/$(TARGET)/debug/kronika-collector \
		target/$(TARGET)/debug/kronika-demo

demo-image: ## Build the demo Docker image (PostgreSQL, PgBouncer, collector, web).
	@scripts/demo-image.sh build

demo-image-run: ## Build and run the demo image, publishing web on :8080.
	@scripts/demo-image.sh up

demo-up: ## Build and start the interactive demo, then wait until it is usable.
	@scripts/demo-image.sh up

demo-stop: ## Stop the demo while preserving collected demo data.
	@scripts/demo-image.sh stop

demo-clean: ## Stop the demo and remove its containers, network, and data volume.
	@scripts/demo-image.sh clean

demo-status: ## Show the demo container and health state.
	@scripts/demo-image.sh status

demo-logs: ## Follow all demo service logs.
	@scripts/demo-image.sh logs

diagrams: ## Regenerate the committed documentation SVGs from docs/diagrams (requires d2).
	@for f in docs/diagrams/*.d2; do \
		d2 --theme=0 --pad 24 $$f docs/images/$$(basename $$f .d2).svg; \
	done
