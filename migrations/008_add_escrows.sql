-- Migration 008: Escrow / conditional transfers
--
-- Mirrors the on-chain `create_escrow` / `release_escrow` / `refund_escrow`
-- Soroban contract functions. Funds are locked in the contract itself; the
-- backend only indexes state and, once conditions are met, submits the
-- release/refund contract call.

CREATE TYPE escrow_status AS ENUM (
    'active',
    'released',
    'refunded',
    'cancelled'
);

CREATE TABLE IF NOT EXISTS escrows (
    id                    UUID           PRIMARY KEY DEFAULT gen_random_uuid(),
    depositor_id          UUID           NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    depositor_account     TEXT           NOT NULL,
    beneficiary_account   TEXT           NOT NULL,
    asset_code            TEXT           NOT NULL,
    asset_issuer          TEXT,
    amount                TEXT           NOT NULL,
    unlock_time           TIMESTAMPTZ    NOT NULL,
    arbiter_account       TEXT,
    status                escrow_status  NOT NULL DEFAULT 'active',
    onchain_escrow_id     TEXT,
    funding_tx_hash       TEXT,
    release_tx_hash       TEXT,
    created_at            TIMESTAMPTZ    NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ    NOT NULL DEFAULT NOW()
);

CREATE TRIGGER escrows_set_updated_at
    BEFORE UPDATE ON escrows
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX IF NOT EXISTS idx_escrows_depositor_id
    ON escrows (depositor_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_escrows_beneficiary_account
    ON escrows (beneficiary_account);

-- Keeper query: escrows past their unlock time still awaiting release.
CREATE INDEX IF NOT EXISTS idx_escrows_unlock_due
    ON escrows (unlock_time)
    WHERE status = 'active';
