-- Migration 007 (down): Payment requests / invoicing
-- Reverses 007_add_payment_requests.up.sql — drops the indexes, the
-- table (its trigger goes with it), then the `payment_request_status`
-- enum.

DROP INDEX IF EXISTS idx_payment_requests_requester_id;
DROP INDEX IF EXISTS idx_payment_requests_status;
DROP TABLE IF EXISTS payment_requests;
DROP TYPE IF EXISTS payment_request_status;