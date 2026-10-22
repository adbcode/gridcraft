SHELL := /bin/sh
.DEFAULT_GOAL := help

CARGO ?= cargo
TRUNK ?= trunk
TRUNK_TOOLS_WASM_OPT ?= version_131
export TRUNK_TOOLS_WASM_OPT
CONTAINER ?= docker
IMAGE ?= gridcraft-service:local

.PHONY: help env build build-server build-web run run-native web-dev check-config check test test-db ci db-migrate db-seed-demo container-build

help: ## Show local development commands
	@awk 'BEGIN {FS = ":.*## "; printf "GridCraft service and editor tasks\n\n"} /^[a-zA-Z_-]+:.*## / {printf "  %-20s %s\n", $$1, $$2}' $(MAKEFILE_LIST)

env: ## Create a private .env from the example; preserve existing values
	@sh scripts/service/local-env.sh

build: build-server build-web ## Build the Rust server and release web assets

build-server: ## Build the release Rust server
	$(CARGO) build --locked --release -p gridcraft-server

build-web: ## Build WASM into dist/web using Trunk
	cd apps/gridcraft-web && $(TRUNK) build --locked --release

run: ## Run the HTTP server in the foreground; stop with Ctrl-C
	$(CARGO) run --locked -p gridcraft-server -- serve

run-native: ## Run the native spreadsheet with synthetic sales
	$(CARGO) run --locked -p gridcraft -- --sample sales

web-dev: ## Run the standalone editor preview on port 8771 (no backend proxy)
	cd apps/gridcraft-web && $(TRUNK) serve --locked

check-config: ## Validate configuration without printing secrets or making service calls
	$(CARGO) run --locked -p gridcraft-server -- check-config

check: ## Check formatting, server compilation and strict server clippy
	$(CARGO) fmt --all -- --check
	$(CARGO) check --locked -p gridcraft-server
	$(CARGO) clippy --locked -p gridcraft-server --all-targets -- -D warnings

test: ## Run server unit and local HTTP contract tests; no paid provider calls
	$(CARGO) test --locked -p gridcraft-server

test-db: ## Run opt-in PostgreSQL tests against GRIDCRAFT_TEST_DATABASE_URL
	@test -n "$${GRIDCRAFT_TEST_DATABASE_URL:-}" || { printf '%s\n' 'Set GRIDCRAFT_TEST_DATABASE_URL to a disposable PostgreSQL database.' >&2; exit 1; }
	$(CARGO) test --locked -p gridcraft-server --tests -- --ignored --test-threads=1

ci: ## Run the full repository commit gate, including WASM
	$(CARGO) xtask ci

db-migrate: ## Apply checked-in migrations to the configured DATABASE_URL
	$(CARGO) run --locked -p gridcraft-server -- migrate

db-seed-demo: ## Explicitly insert synthetic sales in the configured workspace
	$(CARGO) run --locked -p gridcraft-server -- seed-demo

container-build: ## Build the one-origin Aiven image (Docker or CONTAINER=podman)
	$(CONTAINER) build --tag "$(IMAGE)" .
