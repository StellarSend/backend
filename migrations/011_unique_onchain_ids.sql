-- Migration 011: Prevent two rows referencing the same on-chain entity (#56)
--
-- subscriptions.onchain_subscription_id and escrows.onchain_escrow_id are
-- used as the literal on-chain contract argument identifying which real
-- on-chain entity a keeper (subscriptions) or client-signed action (escrows)
-- is targeting, but neither had a uniqueness constraint. For subscriptions
-- this is a double-execution risk: if two DB rows ever share the same
-- onchain_subscription_id, SubscriptionService::run_due_executions's
-- FOR UPDATE SKIP LOCKED sweep selects both independently, and
-- execute_subscription gets invoked twice against the same real on-chain
-- subscription in a single keeper pass.
--
-- Partial (not plain UNIQUE) because both columns are nullable — a row can
-- legitimately be created before its on-chain id is known (created
-- off-chain-first), and multiple NULLs must remain permitted.
--
-- Defensive note: if either table already has pre-existing duplicate
-- non-null values, this migration will fail to apply rather than silently
-- installing a constraint that doesn't actually hold — surfacing the
-- conflict for manual review/cleanup instead of masking it, per the
-- issue's own guidance. No automatic de-duplication is attempted here.

CREATE UNIQUE INDEX IF NOT EXISTS idx_subscriptions_onchain_id_unique
    ON subscriptions (onchain_subscription_id)
    WHERE onchain_subscription_id IS NOT NULL;

CREATE UNIQUE INDEX IF NOT EXISTS idx_escrows_onchain_id_unique
    ON escrows (onchain_escrow_id)
    WHERE onchain_escrow_id IS NOT NULL;
