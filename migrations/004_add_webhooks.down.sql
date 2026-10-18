-- Migration 004 (down): Webhooks
-- Reverses 004_add_webhooks.up.sql — drops the table.

DROP TABLE IF EXISTS webhooks;