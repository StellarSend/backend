-- Migration 011 (down): Unique on-chain ids
-- Reverses 011_unique_onchain_ids.up.sql — drops the two partial UNIQUE
-- indexes on subscriptions.onchain_subscription_id and
-- escrows.onchain_escrow_id. Pure index drops are cleanly reversible.

DROP INDEX IF EXISTS idx_subscriptions_onchain_id_unique;
DROP INDEX IF EXISTS idx_escrows_onchain_id_unique;