-- Migration 012: Add byte length constraint to payment_requests.memo (#49)
--
-- Stellar's on-chain MEMO_TEXT field is hard-capped at 28 bytes by the protocol
-- (stellar_xdr Memo::Text is backed by StringM<28>).
-- This migration adds a CHECK constraint ensuring payment_requests.memo never
-- exceeds 28 UTF-8 bytes (octet_length) in the database.

ALTER TABLE payment_requests
    ADD CONSTRAINT check_payment_requests_memo_max_bytes
    CHECK (memo IS NULL OR octet_length(memo) <= 28);
