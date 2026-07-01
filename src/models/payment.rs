use serde::{Deserialize, Serialize};

/// An asset on the Stellar network — either native XLM or a code:issuer pair.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Asset {
    /// e.g. "XLM", "USDC", "BTC"
    pub code: String,
    /// None for XLM (native), Some(issuer_address) otherwise
    pub issuer: Option<String>,
}

impl Asset {
    pub fn native() -> Self {
        Self {
            code: "XLM".to_string(),
            issuer: None,
        }
    }

    pub fn is_native(&self) -> bool {
        self.code.to_uppercase() == "XLM" && self.issuer.is_none()
    }

    /// Format as "native" or "CODE:ISSUER" for Horizon query params.
    pub fn to_horizon_param(&self) -> String {
        if self.is_native() {
            "native".to_string()
        } else {
            format!(
                "{}:{}",
                self.code,
                self.issuer.as_deref().unwrap_or_default()
            )
        }
    }
}

/// One hop in a payment path.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PathHop {
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub asset_type: String,
}

/// Request body for POST /api/payments/quote.
#[derive(Debug, Deserialize)]
pub struct QuoteRequest {
    pub from_asset: Asset,
    pub to_asset: Asset,
    /// Amount to send (in the from_asset denomination).
    pub amount: String,
    pub destination: String,
}

/// Response body for GET /api/payments/quote.
#[derive(Debug, Serialize)]
pub struct QuoteResponse {
    /// Amount the destination will receive.
    pub estimated_receive: String,
    /// Implied exchange rate (to_amount / from_amount).
    pub rate: String,
    /// Estimated Stellar network fee in XLM.
    pub fee_xlm: String,
    /// Asset hops between source and destination.
    pub path: Vec<PathHop>,
    /// The raw send amount echoed back.
    pub send_amount: String,
    pub from_asset: Asset,
    pub to_asset: Asset,
}

/// Request body for POST /api/payments/send.
///
/// The client builds and signs the Stellar transaction client-side, then
/// submits the signed XDR envelope here for relay to Horizon.
#[derive(Debug, Deserialize)]
pub struct SendPaymentRequest {
    /// The Stellar public key of the sending account.
    pub source_account: String,
    /// Base64-encoded signed TransactionEnvelope XDR.
    pub signed_xdr: String,
    /// Internal transaction record id (returned by the quote or a prior POST).
    pub transaction_id: Option<uuid::Uuid>,
    // Metadata echoed into the DB record when transaction_id is None.
    pub from_asset: Option<super::payment::Asset>,
    pub to_asset: Option<super::payment::Asset>,
    pub send_amount: Option<String>,
    pub destination_account: Option<String>,
}

/// Response body for POST /api/payments/send.
#[derive(Debug, Serialize)]
pub struct PaymentResult {
    /// Stellar transaction hash.
    pub tx_hash: String,
    /// Whether the Horizon submission was successful.
    pub success: bool,
    /// Ledger number in which the transaction was included.
    pub ledger: Option<u64>,
    /// Fee charged in stroops.
    pub fee_charged: Option<String>,
    /// Internal transaction id.
    pub transaction_id: uuid::Uuid,
}
