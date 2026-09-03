-- Migration 012: Enforce minimum bound on subscriptions.interval_seconds (#47)
--
-- Adds a CHECK constraint requiring interval_seconds >= 60 to prevent
-- malicious or buggy subscriptions from draining a payer's on-chain
-- allowance every keeper poll cycle.

ALTER TABLE subscriptions
    DROP CONSTRAINT IF EXISTS subscriptions_interval_seconds_check;

ALTER TABLE subscriptions
    ADD CONSTRAINT subscriptions_interval_seconds_check
    CHECK (interval_seconds >= 60);
