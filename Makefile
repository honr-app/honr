# honr — API (Rust) + UI (Vite/React)
#
#   make          build both (cargo binary + web/dist for :8080)
#   make run      build both, then serve on :8080
#   make dev-ui   Vite hot-reload on :5173 (proxies API to :8080)
#   make test     cargo + web unit tests

.PHONY: all build api release ui install-ui run dev-ui test test-api test-ui clippy sandbox clean help

all: build

help:
	@echo "Targets:"
	@echo "  make / make build   Build API (debug) and UI into web/dist"
	@echo "  make api            cargo build (debug — fast iterate)"
	@echo "  make release        cargo build --release"
	@echo "  make ui             npm build → web/dist (served by the API)"
	@echo "  make run            Build both, then cargo run (debug)"
	@echo "  make dev-ui         Vite dev server (:5173 → :8080)"
	@echo "  make sandbox        Rebuild honr-sandbox:latest (warm crates/npm caches)"
	@echo "  make test           cargo nextest/test + web tests"
	@echo "  make clippy         cargo clippy -D warnings"
	@echo "  make clean          cargo clean + remove web/dist"

build: api ui

api:
	cargo build

release:
	cargo build --release

install-ui:
	npm --prefix web install

ui: install-ui
	npm --prefix web run build

run: build
	cargo run

dev-ui: install-ui
	npm --prefix web run dev

test: test-api test-ui

test-api:
	@if command -v cargo-nextest >/dev/null 2>&1; then \
		cargo nextest run --offline; \
	else \
		cargo test --offline; \
	fi

test-ui: install-ui
	npm --prefix web test

clippy:
	cargo clippy --all-targets --offline -- -D warnings

# Rebuild when Cargo.lock / web/package-lock.json change. New sandboxes pick
# this up via --from; existing ones keep the create-time image.
sandbox:
	docker build -f sandbox/Containerfile -t honr-sandbox:latest .

clean:
	cargo clean
	rm -rf web/dist web/dist-test
