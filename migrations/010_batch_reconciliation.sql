-- Migration 010: Durable crash/timeout recovery for batch payments (#30)
--
-- BatchPaymentService::execute_batch previously had no durability guarantee
-- across a crash between Horizon accepting a submission and our own status
-- UPDATE committing, and no way to distinguish a genuinely rejected
-- transaction from a submission whose HTTP response was merely lost. This
-- migration adds the schema pieces that close both gaps:
--
--   1. `stellar_tx_hash` was UNIQUE, which is wrong for batches — every leg
--      row of a single batch legitimately shares one hash (one transaction,
--      N payment operations). Drop the constraint, keep a plain index for
--      reconciliation lookups.
--   2. A new `submitted_unconfirmed` status: the ambiguous outcome of a
--      submission whose result we never got back (client-side timeout,
--      connection drop, or a crash before our post-submission UPDATE
--      committed), distinct from a definite Horizon-rejected `failed`.
--   3. `batch_submissions`: one row per distinct signed transaction hash,
--      inserted *before* submission and never deleted. Its PRIMARY KEY is
--      the actual concurrency-safety primitive that stops the same signed
--      XDR from ever being submitted twice through this service — an
--      atomic INSERT reacting to a primary-key violation, not a
--      check-then-insert, which would leave the same race open.

ALTER TABLE transactions
    DROP CONSTRAINT IF EXISTS transactions_stellar_tx_hash_key;

CREATE INDEX IF NOT EXISTS idx_transactions_stellar_tx_hash
    ON transactions (stellar_tx_hash)
    WHERE stellar_tx_hash IS NOT NULL;

ALTER TYPE transaction_status ADD VALUE IF NOT EXISTS 'submitted_unconfirmed';

CREATE TABLE IF NOT EXISTS batch_submissions (
    stellar_tx_hash TEXT        PRIMARY KEY,
    batch_id        UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
