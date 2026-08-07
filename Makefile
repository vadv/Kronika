RUST_TOOLCHAIN ?= 1.96.0
TARGET ?= $(shell rustc +$(RUST_TOOLCHAIN) -vV | sed -n 's/^host: //p')
CARGO_BUILD = cargo +$(RUST_TOOLCHAIN) build --locked --target $(TARGET)

.PHONY: build collector demo fmt lint test check test-bdd

build: ## Build every binary for the selected target.
	@$(CARGO_BUILD) -p kronika-collector -p kronika-demo

collector: ## Build kronika-collector.
	@$(CARGO_BUILD) -p kronika-collector

demo: ## Build kronika-demo.
	@$(CARGO_BUILD) -p kronika-demo

fmt: ## Format the workspace.
	@cargo +$(RUST_TOOLCHAIN) fmt --all

lint: ## Run clippy over the workspace with warnings denied.
	@cargo +$(RUST_TOOLCHAIN) clippy --workspace --all-targets -- -D warnings

test: ## Run the unit and integration test suite.
	@cargo +$(RUST_TOOLCHAIN) test --workspace

check: fmt lint test ## The full pre-commit gate.

test-bdd: ## Run BDD inside the cached Docker image.
	@scripts/bdd-image.sh run
