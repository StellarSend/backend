-- Performance indexes
-- Migration: 005_add_indexes
-- Author: Adaora Nwosu

-- Original text referenced a "payments" table with from_address/to_address
-- columns that never existed under those names — the actual table (created
-- by 001_initial.sql) is "transactions", with source_account/
-- destination_account. Fixed to match the real schema; this migration had
-- never applied cleanly to a fresh database before this fix.
CREATE INDEX IF NOT EXISTS idx_payments_from_address ON transactions(source_account);
CREATE INDEX IF NOT EXISTS idx_payments_to_address ON transactions(destination_account);
CREATE INDEX IF NOT EXISTS idx_payments_created_at ON transactions(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_users_stellar_address ON users(stellar_address);
