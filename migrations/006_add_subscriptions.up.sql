-- Migration 006: Recurring / scheduled payments (subscriptions)
--
-- Mirrors the on-chain `create_subscription` / `cancel_subscription` /
-- `execute_subscription` Soroban contract functions. The payer authorizes
-- the subscription on-chain once; this table is the backend's index of
-- that authorization plus the scheduling metadata a keeper job needs to
-- know when the next execution is due.

CREATE TYPE subscription_status AS ENUM (
    'active',
    'cancelled',
    'failed',
    'completed'
);

CREATE TABLE IF NOT EXISTS subscriptions (
    id                    UUID                 PRIMARY KEY DEFAULT gen_random_uuid(),
    payer_id              UUID                 NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    payer_account         TEXT                 NOT NULL,
    recipient_account     TEXT                 NOT NULL,
    asset_code            TEXT                 NOT NULL,
    asset_issuer          TEXT,
    amount                TEXT                 NOT NULL,
    interval_seconds      BIGINT               NOT NULL CHECK (interval_seconds > 0),
    next_execution_at     TIMESTAMPTZ          NOT NULL,
    status                subscription_status  NOT NULL DEFAULT 'active',
    -- Populated once the on-chain `create_subscription` call has been
    -- confirmed (the id the Soroban contract assigned).
    onchain_subscription_id TEXT,
    last_execution_at     TIMESTAMPTZ,
    last_tx_hash          TEXT,
    failure_count         INT                  NOT NULL DEFAULT 0,
    last_error            TEXT,
    created_at            TIMESTAMPTZ          NOT NULL DEFAULT NOW(),
    updated_at            TIMESTAMPTZ          NOT NULL DEFAULT NOW()
);

CREATE TRIGGER subscriptions_set_updated_at
    BEFORE UPDATE ON subscriptions
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

-- Keeper query: find active subscriptions due for execution.
CREATE INDEX IF NOT EXISTS idx_subscriptions_due
    ON subscriptions (next_execution_at)
    WHERE status = 'active';

CREATE INDEX IF NOT EXISTS idx_subscriptions_payer_id
    ON subscriptions (payer_id, created_at DESC);
