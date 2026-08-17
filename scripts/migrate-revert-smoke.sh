#!/usr/bin/env bash
#
# Migration-revert smoke test (#58).
#
# Proves sqlx's reversible-migration tooling is actually wired up:
#   1. every migration applies to a fresh database;
#   2. `sqlx migrate revert` steps the newest reversible migration back and
#      the schema matches the pre-migration state;
#   3. the intentionally non-revertible piece of migration 010 (the
#      `submitted_unconfirmed` enum value) survives a revert, as documented;
#   4. re-running migrations afterwards re-applies cleanly (idempotent).
#
# Usage:
#   BASE_URL=postgres://postgres:postgres@localhost:5432 bash scripts/migrate-revert-smoke.sh
#
# Requires: sqlx-cli (https://github.com/launchbadge/sqlx — install with the
# flags in README.md) and a reachable PostgreSQL. If `psql` is on PATH the
# schema-level assertions below also run; otherwise only the migration-state
# transitions are checked.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
MIGRATIONS_DIR="$ROOT/migrations"

BASE_URL="${BASE_URL:-postgres://postgres:postgres@localhost:5432}"
# Unique per run: `sqlx database create` is a no-op when the database already
# exists, so a name collision with a previous run would reuse stale state.
DB_NAME="${DB_NAME:-stellarsend_smoke_$(date +%s)_$$}"
TARGET="${BASE_URL%/}/${DB_NAME}"

fail() { echo "FAIL: $*" >&2; exit 1; }
ok()   { echo "ok:   $*"; }
skip() { echo "skip: $*"; }

# --- 0. Fresh database ------------------------------------------------
# `sqlx database create` is a no-op if the (unique) database already exists.
sqlx database create --database-url "$TARGET"

info_state() { # <version> <state> ; asserts a migration's recorded state
  sqlx migrate info --source "$MIGRATIONS_DIR" --database-url "$TARGET" \
    | grep -E "^${1}/${2}[[:space:]]" >/dev/null \
    || fail "expected version ${1} to be ${2}"
}

total="$(ls "$MIGRATIONS_DIR"/[0-9]*.up.sql | wc -l | tr -d ' ')"

installed_count() {
  sqlx migrate info --source "$MIGRATIONS_DIR" --database-url "$TARGET" \
    | grep -c '/installed' || true
}

psql_assert() { # <label> <sql> <expected-trimmed-output>
  if command -v psql >/dev/null 2>&1; then
    local actual
    actual="$(psql -X -A -t "$TARGET" -c "$2" | tr -d '[:space:]')"
    [ "$actual" = "$3" ] || fail "$1: expected '$3', got '$actual'"
    ok "$1"
  else
    skip "$1 (psql not available)"
  fi
}

# --- 1. Apply everything ---------------------------------------------
echo "== applying all migrations =="
sqlx migrate run --source "$MIGRATIONS_DIR" --database-url "$TARGET"
[ "$(installed_count)" -eq "$total" ] || fail "expected $total installed, got $(installed_count)"
ok "all $total migrations installed on a fresh database"

psql_assert "batch_submissions table exists after full apply" \
  "SELECT to_regclass('public.batch_submissions');" "batch_submissions"
psql_assert "010 dropped UNIQUE constraint on transactions.stellar_tx_hash" \
  "SELECT count(*) FROM pg_constraint WHERE conname='transactions_stellar_tx_hash_key' AND contype='u';" "0"
psql_assert "010 added enum value 'submitted_unconfirmed'" \
  "SELECT count(*) FROM pg_enum e JOIN pg_type t ON t.oid=e.enumtypid WHERE t.typname='transaction_status' AND e.enumlabel='submitted_unconfirmed';" "1"
psql_assert "011 unique index on subscriptions.onchain_subscription_id" \
  "SELECT to_regclass('public.idx_subscriptions_onchain_id_unique');" "idx_subscriptions_onchain_id_unique"
psql_assert "011 unique index on escrows.onchain_escrow_id" \
  "SELECT to_regclass('public.idx_escrows_onchain_id_unique');" "idx_escrows_onchain_id_unique"
psql_assert "009 partial index on transactions.batch_id" \
  "SELECT to_regclass('public.idx_transactions_batch_id');" "idx_transactions_batch_id"

# --- 2. Revert the newest (011) ---------------------------------------
echo "== reverting migration 011 (unique on-chain ids) =="
sqlx migrate revert --source "$MIGRATIONS_DIR" --database-url "$TARGET"
info_state "$total" "pending"
[ "$(installed_count)" -eq $((total - 1)) ] || fail "expected $((total - 1)) installed after revert, got $(installed_count)"
ok "migration $total reverted and recorded as pending"
psql_assert "011 unique index on subscriptions.onchain_subscription_id is gone" \
  "SELECT to_regclass('public.idx_subscriptions_onchain_id_unique');" ""
psql_assert "011 unique index on escrows.onchain_escrow_id is gone" \
  "SELECT to_regclass('public.idx_escrows_onchain_id_unique');" ""

# --- 3. Revert 010 — documents the non-revertible enum piece ----------
echo "== reverting migration 010 (batch reconciliation) =="
sqlx migrate revert --source "$MIGRATIONS_DIR" --database-url "$TARGET"
info_state "10" "pending"
psql_assert "010's batch_submissions table is dropped by revert" \
  "SELECT to_regclass('public.batch_submissions');" ""
psql_assert "010's UNIQUE constraint is restored by revert" \
  "SELECT count(*) FROM pg_constraint WHERE conname='transactions_stellar_tx_hash_key' AND contype='u';" "1"
psql_assert "010's enum value survives the revert (irreversible, as documented)" \
  "SELECT count(*) FROM pg_enum e JOIN pg_type t ON t.oid=e.enumtypid WHERE t.typname='transaction_status' AND e.enumlabel='submitted_unconfirmed';" "1"

# --- 4. Revert 009 — the simple, fully-reversible case -----------------
echo "== reverting migration 009 (split / batch payments) =="
sqlx migrate revert --source "$MIGRATIONS_DIR" --database-url "$TARGET"
info_state "9" "pending"
psql_assert "009 partial index on transactions.batch_id is gone" \
  "SELECT to_regclass('public.idx_transactions_batch_id');" ""
psql_assert "009 batch_id/batch_index columns are gone" \
  "SELECT count(*) FROM information_schema.columns WHERE table_name='transactions' AND column_name IN ('batch_id','batch_index');" "0"

# --- 5. Re-apply: 010 → 011 must apply cleanly again -------------------
echo "== re-applying the reverted migrations =="
sqlx migrate run --source "$MIGRATIONS_DIR" --database-url "$TARGET"
[ "$(installed_count)" -eq "$total" ] || fail "expected $total installed after re-apply, got $(installed_count)"
psql_assert "re-applied 010 recreates batch_submissions" \
  "SELECT to_regclass('public.batch_submissions');" "batch_submissions"
psql_assert "re-applied 011 recreates the escrows unique index" \
  "SELECT to_regclass('public.idx_escrows_onchain_id_unique');" "idx_escrows_onchain_id_unique"
ok "migrations re-apply cleanly after a revert (idempotent)"

# --- 6. Teardown -------------------------------------------------------
sqlx database drop --force --database-url "$TARGET"
ok "smoke-test database dropped"

echo
echo "PASS — reversible migrations are wired up and reverts work."