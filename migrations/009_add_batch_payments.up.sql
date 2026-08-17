-- Migration 009: Split / batch payments
--
-- A batch payment is one client-signed Stellar transaction containing many
-- payment operations (one sender, many recipients). We record one
-- `transactions` row per leg so existing history/reporting keeps working,
-- linked together by `batch_id`.

ALTER TABLE transactions
    ADD COLUMN IF NOT EXISTS batch_id    UUID,
    ADD COLUMN IF NOT EXISTS batch_index INT;

CREATE INDEX IF NOT EXISTS idx_transactions_batch_id
    ON transactions (batch_id)
    WHERE batch_id IS NOT NULL;
