-- Migration 012: Validate payment_requests.memo length against Stellar 28-byte limit (#49)
--
-- Stellar's on-chain MEMO_TEXT field is hard-capped at 28 bytes by the protocol
-- (stellar_xdr::curr::Memo::Text backed by StringM<28>). A memo longer than that
-- cannot be embedded into an on-chain transaction fulfilling the request.
--
-- Multi-byte UTF-8 characters mean length in characters != length in bytes;
-- octet_length ensures strict byte-level enforcement in PostgreSQL.
-- The column is nullable, so NULL values remain permitted.

ALTER TABLE payment_requests
ADD CONSTRAINT check_payment_requests_memo_length
CHECK (memo IS NULL OR octet_length(memo) <= 28);
