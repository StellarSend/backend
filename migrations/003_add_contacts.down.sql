-- Migration 003 (down): Contacts
-- Reverses 003_add_contacts.up.sql — drops the index and the table.

DROP INDEX IF EXISTS idx_contacts_user_id;
DROP TABLE IF EXISTS contacts;