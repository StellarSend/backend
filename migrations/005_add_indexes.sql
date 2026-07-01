-- Performance indexes
-- Migration: 005_add_indexes
-- Author: Adaora Nwosu

CREATE INDEX IF NOT EXISTS idx_payments_from_address ON payments(from_address);
CREATE INDEX IF NOT EXISTS idx_payments_to_address ON payments(to_address);
CREATE INDEX IF NOT EXISTS idx_payments_created_at ON payments(created_at DESC);
CREATE INDEX IF NOT EXISTS idx_users_stellar_address ON users(stellar_address);
