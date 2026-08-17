-- Migration 002 (down): Performance indices
-- Reverses 002_add_indices.up.sql — drops every index it created.
-- Pure `DROP INDEX` statements are cleanly reversible by design.

DROP INDEX IF EXISTS idx_users_email_active;
DROP INDEX IF EXISTS idx_users_stellar_address;
DROP INDEX IF EXISTS idx_transactions_user_id_created_at;
DROP INDEX IF EXISTS idx_transactions_user_status;
DROP INDEX IF EXISTS idx_transactions_stellar_tx_hash;
DROP INDEX IF EXISTS idx_transactions_assets;
DROP INDEX IF EXISTS idx_transactions_in_flight;