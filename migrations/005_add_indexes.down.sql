-- Migration 005 (down): Performance indexes
-- Reverses 005_add_indexes.up.sql — drops the three indexes this
-- migration actually created on `transactions`.
--
-- Note: the fourth statement in the up migration
-- (`idx_users_stellar_address` on `users(stellar_address)`) is a
-- no-op at runtime — 002_add_indices already created an index with that
-- exact name, and `CREATE INDEX IF NOT EXISTS` collides on the name. It
-- is therefore deliberately NOT dropped here; 002's down script owns it.

DROP INDEX IF EXISTS idx_payments_from_address;
DROP INDEX IF EXISTS idx_payments_to_address;
DROP INDEX IF EXISTS idx_payments_created_at;