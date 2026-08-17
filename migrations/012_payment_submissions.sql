-- Migration 012: Duplicate-submission guard for single-payment sends (#35)
--
-- PaymentService::execute_send had no protection against the same race that
-- BatchPaymentService::execute_batch was hardened against in migration 010.
-- This migration adds the parallel primitives for single payments:
--
--   1. `payment_submissions`: one row per distinct signed transaction hash,
--      inserted atomically *before* submission, never deleted.  Its PRIMARY
--      KEY is the concurrency-safety primitive that stops the same signed XDR
--      from being submitted twice — a primary-key violation on INSERT means
--      "this exact transaction was already dispatched; reject outright."
--      Keyed separately from `batch_submissions` so the two flows remain
--      independently auditable; the underlying guarantee is identical.
--
-- No status-enum changes are needed: `submitted_unconfirmed` was already
-- added in migration 010 and is reused here so ReconciliationService can
-- recover ambiguous single-payment sends exactly as it recovers batches.

CREATE TABLE IF NOT EXISTS payment_submissions (
    stellar_tx_hash TEXT        PRIMARY KEY,
    transaction_id  UUID        NOT NULL,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
