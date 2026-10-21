#!/usr/bin/env bash
# Runs every CI check locally, in CI order. Exits non-zero on the first failure.
set -euo pipefail

cd "$(dirname "$0")/.."

step() {
  printf '\n==> %s\n' "$*"
}

step "web: install"
(cd web && npm ci)
step "web: lint"
(cd web && npm run lint)
step "web: format check"
(cd web && npm run format:check)
step "web: svelte-check"
(cd web && npm run check)
step "web: test"
(cd web && npm test)
step "web: build"
(cd web && npm run build)

step "rust: fmt"
cargo fmt --all --check
step "rust: clippy"
cargo clippy --workspace --all-targets -- -D warnings
step "rust: test"
cargo test --workspace
step "rust: wasm32 core build"
cargo build -p notebook-core --target wasm32-unknown-unknown

printf '\nAll checks passed.\n'
