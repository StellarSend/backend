-- Migration 006 (down): Recurring / scheduled payments (subscriptions)
-- Reverses 006_add_subscriptions.up.sql — drops the indexes, the table
-- (its trigger goes with it), then the `subscription_status` enum.

DROP INDEX IF EXISTS idx_subscriptions_due;
DROP INDEX IF EXISTS idx_subscriptions_payer_id;
DROP TABLE IF EXISTS subscriptions;
DROP TYPE IF EXISTS subscription_status;