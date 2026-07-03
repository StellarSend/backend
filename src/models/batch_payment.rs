use serde::Deserialize;
use uuid::Uuid;

/// One leg of a batch/split payment: pay `amount` of `asset` to `destination`.
#[derive(Debug, Clone, Deserialize)]
pub struct BatchPaymentLeg {
    pub destination: String,
    pub amount: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
}

/// Request body for POST /api/payments/batch.
///
/// Same non-custodial pattern as /api/payments/send: the client builds and
/// signs a *single* Stellar transaction containing one `Payment` (or
/// `send_batch_payment` Soroban) operation per recipient, then posts the
/// signed XDR here. We relay it once and record one `transactions` row per
/// leg so existing history/reporting keeps working per-recipient.
#[derive(Debug, Deserialize)]
pub struct SendBatchPaymentRequest {
    pub source_account: String,
    pub signed_xdr: String,
    pub legs: Vec<BatchPaymentLeg>,
}

#[derive(Debug, serde::Serialize)]
pub struct BatchPaymentResult {
    pub batch_id: Uuid,
    pub tx_hash: String,
    pub success: bool,
    pub ledger: Option<u64>,
    pub fee_charged: Option<String>,
    pub leg_transaction_ids: Vec<Uuid>,
}
