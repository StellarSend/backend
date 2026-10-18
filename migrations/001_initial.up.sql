-- Migration 001: Initial schema
-- Creates the `users` and `transactions` tables.

-- ─── Extensions ───────────────────────────────────────────────────────────────
CREATE EXTENSION IF NOT EXISTS "pgcrypto";

-- ─── Enum types ───────────────────────────────────────────────────────────────
CREATE TYPE transaction_status AS ENUM (
    'pending',
    'submitted',
    'completed',
    'failed',
    'expired'
);

-- ─── Users ────────────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS users (
    id               UUID        PRIMARY KEY DEFAULT gen_random_uuid(),
    email            TEXT        NOT NULL UNIQUE,
    password_hash    TEXT        NOT NULL,
    full_name        TEXT        NOT NULL,
    stellar_address  TEXT,
    is_active        BOOLEAN     NOT NULL DEFAULT TRUE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT NOW()
);

-- Auto-update the updated_at column on every row modification.
CREATE OR REPLACE FUNCTION set_updated_at()
RETURNS TRIGGER LANGUAGE plpgsql AS $$
BEGIN
    NEW.updated_at = NOW();
    RETURN NEW;
END;
$$;

CREATE TRIGGER users_set_updated_at
    BEFORE UPDATE ON users
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- ─── Transactions ─────────────────────────────────────────────────────────────
CREATE TABLE IF NOT EXISTS transactions (
    id                   UUID               PRIMARY KEY DEFAULT gen_random_uuid(),
    user_id              UUID               NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    stellar_tx_hash      TEXT               UNIQUE,
    from_asset           TEXT               NOT NULL,
    to_asset             TEXT               NOT NULL,
    send_amount          TEXT               NOT NULL,
    receive_amount       TEXT,
    source_account       TEXT               NOT NULL,
    destination_account  TEXT               NOT NULL,
    status               transaction_status NOT NULL DEFAULT 'pending',
    error_message        TEXT,
    fee_xlm              TEXT,
    path                 JSONB,
    created_at           TIMESTAMPTZ        NOT NULL DEFAULT NOW(),
    updated_at           TIMESTAMPTZ        NOT NULL DEFAULT NOW()
);

CREATE TRIGGER transactions_set_updated_at
    BEFORE UPDATE ON transactions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
