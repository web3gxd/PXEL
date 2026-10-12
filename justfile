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

# Separate fresh-instance acceptance; requires native PostgreSQL 18 binaries in PXEL_PG_BIN.
selfhost-check:
    cargo build --locked -p pxel-server
    pnpm build
    node --test --test-concurrency=1 tests/recovery/local-auth.test.mjs tests/recovery/selfhost-browser.test.mjs

bench-formula-rebase:
    cargo test --locked --release -p spreadsheet-collab --lib axis_history::tests::historical_formula_binding_benchmark -- --ignored --exact --nocapture

formula-parity:
    node scripts/wasm.mjs
    node tests/parity/formula.mjs
    node tests/parity/workbook.mjs
    node tests/parity/move.mjs

xlsx-wasi-check:
    cargo build --locked -p spreadsheet-xlsx --features worker-protocol --bin pxel-xlsx-worker --target wasm32-wasip1 --release
    cargo build --locked -p pxel-server --features dev-auth
    node --test tests/security/wasi-xlsx.test.mjs tests/security/xlsx-runtime.test.mjs

convergence:
    cargo test --locked -p spreadsheet-domain --test convergence -- --ignored --nocapture

bench-collab:
    node --test tests/performance/collaboration.test.mjs

bench-render:
    node tests/performance/render.mjs

bench-render-sparse:
    node tests/performance/render-sparse.mjs

bench-optimistic:
    node tests/performance/optimistic.mjs

bench-move-preview:
    node tests/performance/move-preview.mjs

bench-recovery-storage:
    cargo test --release --locked -p spreadsheet-storage --lib recovery_benchmark::persisted_snapshot_plus_ten_thousand_tail_events -- --ignored --exact --nocapture --test-threads=1

stress-slow-reader:
    node --test tests/performance/slow-reader.test.mjs

sqlx-check:
    cargo sqlx prepare --check --workspace -- --all-targets --all-features

s3-contract:
    cargo test --locked -p spreadsheet-storage --features s3 --test s3_contract -- --ignored --nocapture

s3-encrypted-snapshot:
    cargo build --locked -p pxel-server --features s3,dev-auth
    node --test tests/recovery/styles-restart.test.mjs

share-log-redaction:
    cargo build --locked -p pxel-server --features dev-auth
    node scripts/verify-share-log-redaction.mjs

distributed-share-revocation:
    cargo build --locked -p pxel-server --features dev-auth
    node --test tests/recovery/distributed-share-revocation.test.mjs

crash-recovery-postgres:
    cargo build --locked -p pxel-server --features dev-auth
    node --test tests/recovery/postgres-immediate-stop.test.mjs

backup-restore-postgres:
    cargo build --locked -p pxel-server --features dev-auth
    node --test tests/recovery/backup-restore.test.mjs

pitr-restore-postgres:
    cargo build --locked -p pxel-server --features dev-auth
    node --test tests/recovery/pitr-restore.test.mjs

audit:
    cargo deny check
    cargo audit
    pnpm audit
