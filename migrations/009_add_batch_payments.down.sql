-- Migration 009 (down): Split / batch payments
-- Reverses 009_add_batch_payments.up.sql — drops the partial index and
-- the two nullable columns that were added to `transactions`.
--
-- The columns are nullable with no dependent objects, so this is a clean,
-- unconditionally safe reversal (the issue calls this out as a low-risk
-- starting point). Columns are dropped in reverse of their creation order.

DROP INDEX IF EXISTS idx_transactions_batch_id;

ALTER TABLE transactions
    DROP COLUMN IF EXISTS batch_index,
    DROP COLUMN IF EXISTS batch_id;