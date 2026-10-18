-- Migration 007: Payment requests / invoicing
--
-- Mirrors the on-chain `create_payment_request` / `fulfill_payment_request`
-- / `cancel_payment_request` Soroban contract functions.

CREATE TYPE payment_request_status AS ENUM (
    'pending',
    'fulfilled',
    'cancelled',
    'expired'
);

CREATE TABLE IF NOT EXISTS payment_requests (
    id                  UUID                    PRIMARY KEY DEFAULT gen_random_uuid(),
    requester_id        UUID                    NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    requester_account   TEXT                    NOT NULL,
    -- If set, only this account may fulfill the request.
    payer_account       TEXT,
    asset_code          TEXT                    NOT NULL,
    asset_issuer        TEXT,
    amount              TEXT                    NOT NULL,
    memo                TEXT,
    status              payment_request_status  NOT NULL DEFAULT 'pending',
    onchain_request_id  TEXT,
    fulfilled_tx_hash   TEXT,
    fulfilled_by        TEXT,
    expires_at          TIMESTAMPTZ,
    created_at          TIMESTAMPTZ             NOT NULL DEFAULT NOW(),
    updated_at          TIMESTAMPTZ             NOT NULL DEFAULT NOW()
);

CREATE TRIGGER payment_requests_set_updated_at
    BEFORE UPDATE ON payment_requests
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE INDEX IF NOT EXISTS idx_payment_requests_requester_id
    ON payment_requests (requester_id, created_at DESC);

CREATE INDEX IF NOT EXISTS idx_payment_requests_status
    ON payment_requests (status);
