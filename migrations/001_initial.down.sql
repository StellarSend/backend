-- Migration 001 (down): Initial schema
-- Reverses 001_initial.up.sql — removes everything it created:
-- extensions, the `transaction_status` enum, the `set_updated_at`
-- trigger function, and the `users`/`transactions` tables.
--
-- Order matters (reverse of creation):
--   1. Triggers before the tables/function they depend on.
--   2. Tables before the function and enum they reference.
--   3. Enum before the extension that has no other dependents.
--
-- All statements carry IF EXISTS so this is safe to re-run.

DROP TRIGGER IF EXISTS transactions_set_updated_at ON transactions;
DROP TRIGGER IF EXISTS users_set_updated_at ON users;
DROP TABLE IF EXISTS transactions;
DROP TABLE IF EXISTS users;
DROP FUNCTION IF EXISTS set_updated_at();
DROP TYPE IF EXISTS transaction_status;
DROP EXTENSION IF EXISTS pgcrypto;