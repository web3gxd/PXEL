set windows-shell := ["pwsh", "-NoLogo", "-NoProfile", "-Command"]

default:
    @just --list

infra-up:
    node scripts/infra.mjs

infra-native:
    node scripts/infra.mjs --native

dev:
    node scripts/dev.mjs

generate:
    pnpm proto:generate

check:
    cargo fmt --all --check
    cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
    cargo clippy --locked --workspace --all-targets -- -D warnings
    cargo test --locked --workspace --all-features
    cargo build --locked --workspace
    cargo check --locked -p spreadsheet-domain -p spreadsheet-wire-protocol -p spreadsheet-formula -p spreadsheet-wasm --target wasm32-unknown-unknown
    pnpm proto:check
    node scripts/wasm.mjs
    pnpm format:check
    pnpm lint
    pnpm typecheck
    pnpm test
    pnpm build

test:
    cargo nextest run --locked --workspace --all-features
    pnpm test

e2e:
    pnpm test:e2e

smoke:
    node scripts/smoke.mjs

formula-parity:
    node scripts/wasm.mjs
    node tests/parity/formula.mjs
    node tests/parity/workbook.mjs

convergence:
    cargo test --locked -p spreadsheet-domain --test convergence -- --ignored --nocapture

bench-collab:
    node --test tests/performance/collaboration.test.mjs

bench-render:
    node tests/performance/render.mjs

sqlx-check:
    cargo sqlx prepare --check --workspace -- --all-targets --all-features

audit:
    cargo deny check
    cargo audit
    pnpm audit
