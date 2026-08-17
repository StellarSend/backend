-- Migration 008 (down): Escrow / conditional transfers
-- Reverses 008_add_escrows.up.sql — drops the indexes, the table (its
-- trigger goes with it), then the `escrow_status` enum.

DROP INDEX IF EXISTS idx_escrows_depositor_id;
DROP INDEX IF EXISTS idx_escrows_beneficiary_account;
DROP INDEX IF EXISTS idx_escrows_unlock_due;
DROP TABLE IF EXISTS escrows;
DROP TYPE IF EXISTS escrow_status;