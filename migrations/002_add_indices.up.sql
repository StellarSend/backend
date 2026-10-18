-- Migration 002: Performance indices
-- Added after profiling to cover the most common query patterns.

-- ─── Users ────────────────────────────────────────────────────────────────────

-- Login lookup (email is already UNIQUE, so an index exists, but an explicit
-- partial index on active users speeds up the common auth path).
CREATE INDEX IF NOT EXISTS idx_users_email_active
    ON users (email)
    WHERE is_active = TRUE;

CREATE INDEX IF NOT EXISTS idx_users_stellar_address
    ON users (stellar_address)
    WHERE stellar_address IS NOT NULL;

-- ─── Transactions ─────────────────────────────────────────────────────────────

-- Most common query: all transactions for a user ordered by recency.
CREATE INDEX IF NOT EXISTS idx_transactions_user_id_created_at
    ON transactions (user_id, created_at DESC);

-- Filter by status within a user's transaction list.
CREATE INDEX IF NOT EXISTS idx_transactions_user_status
    ON transactions (user_id, status);

-- Look up a transaction by its Stellar hash (e.g. after webhook).
CREATE INDEX IF NOT EXISTS idx_transactions_stellar_tx_hash
    ON transactions (stellar_tx_hash)
    WHERE stellar_tx_hash IS NOT NULL;

-- Filter by asset pairs (useful for reporting / analytics).
CREATE INDEX IF NOT EXISTS idx_transactions_assets
    ON transactions (from_asset, to_asset);

-- Partial index for in-flight transactions (status dashboard, cleanup jobs).
CREATE INDEX IF NOT EXISTS idx_transactions_in_flight
    ON transactions (user_id, created_at)
    WHERE status IN ('pending', 'submitted');
