use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "payment_request_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum PaymentRequestStatus {
    Pending,
    Fulfilled,
    Cancelled,
    Expired,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct PaymentRequestRow {
    pub id: Uuid,
    pub requester_id: Uuid,
    pub requester_account: String,
    pub payer_account: Option<String>,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub memo: Option<String>,
    pub status: PaymentRequestStatus,
    pub onchain_request_id: Option<String>,
    pub fulfilled_tx_hash: Option<String>,
    pub fulfilled_by: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaymentRequest {
    pub id: Uuid,
    pub requester_id: Uuid,
    pub requester_account: String,
    pub payer_account: Option<String>,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub memo: Option<String>,
    pub status: PaymentRequestStatus,
    pub onchain_request_id: Option<String>,
    pub fulfilled_tx_hash: Option<String>,
    pub fulfilled_by: Option<String>,
    pub expires_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<PaymentRequestRow> for PaymentRequest {
    fn from(row: PaymentRequestRow) -> Self {
        Self {
            id: row.id,
            requester_id: row.requester_id,
            requester_account: row.requester_account,
            payer_account: row.payer_account,
            asset_code: row.asset_code,
            asset_issuer: row.asset_issuer,
            amount: row.amount,
            memo: row.memo,
            status: row.status,
            onchain_request_id: row.onchain_request_id,
            fulfilled_tx_hash: row.fulfilled_tx_hash,
            fulfilled_by: row.fulfilled_by,
            expires_at: row.expires_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request body for POST /api/payment-requests.
#[derive(Debug, Deserialize)]
pub struct CreatePaymentRequestRequest {
    pub requester_account: String,
    pub payer_account: Option<String>,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub memo: Option<String>,
    /// Seconds from now until this request expires. `None` = never expires.
    pub expires_in_secs: Option<i64>,
}

/// Response for a newly created payment request: includes a
/// QR-encodable/shareable payload for the payer's client to prefill a send.
#[derive(Debug, Serialize)]
pub struct PaymentRequestWithShareLink {
    #[serde(flatten)]
    pub request: PaymentRequest,
    /// Shareable identifier the payer's UI can resolve via
    /// GET /api/payment-requests/:id.
    pub share_url: String,
    /// A minimal payload a client can encode into a QR code without an
    /// extra round-trip.
    pub qr_payload: String,
}

/// Request body for POST /api/payment-requests/:id/fulfill.
#[derive(Debug, Deserialize)]
pub struct FulfillPaymentRequestRequest {
    /// The account actually paying (must match `payer_account` if the
    /// request restricted who may pay).
    pub payer_account: String,
    /// Stellar transaction hash of the client-signed payment that fulfilled
    /// this request (already submitted via /api/payments/send), or the
    /// on-chain `fulfill_payment_request` contract call result.
    pub tx_hash: String,
}
