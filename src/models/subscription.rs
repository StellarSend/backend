use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Status of a recurring-payment subscription.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "subscription_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum SubscriptionStatus {
    Active,
    Cancelled,
    Failed,
    Completed,
}

/// Raw database row for a subscription.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct SubscriptionRow {
    pub id: Uuid,
    pub payer_id: Uuid,
    pub payer_account: String,
    pub recipient_account: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub interval_seconds: i64,
    pub next_execution_at: DateTime<Utc>,
    pub status: SubscriptionStatus,
    pub onchain_subscription_id: Option<String>,
    pub last_execution_at: Option<DateTime<Utc>>,
    pub last_tx_hash: Option<String>,
    pub failure_count: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Public-facing subscription representation.
#[derive(Debug, Clone, Serialize)]
pub struct Subscription {
    pub id: Uuid,
    pub payer_id: Uuid,
    pub payer_account: String,
    pub recipient_account: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub interval_seconds: i64,
    pub next_execution_at: DateTime<Utc>,
    pub status: SubscriptionStatus,
    pub onchain_subscription_id: Option<String>,
    pub last_execution_at: Option<DateTime<Utc>>,
    pub last_tx_hash: Option<String>,
    pub failure_count: i32,
    pub last_error: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<SubscriptionRow> for Subscription {
    fn from(row: SubscriptionRow) -> Self {
        Self {
            id: row.id,
            payer_id: row.payer_id,
            payer_account: row.payer_account,
            recipient_account: row.recipient_account,
            asset_code: row.asset_code,
            asset_issuer: row.asset_issuer,
            amount: row.amount,
            interval_seconds: row.interval_seconds,
            next_execution_at: row.next_execution_at,
            status: row.status,
            onchain_subscription_id: row.onchain_subscription_id,
            last_execution_at: row.last_execution_at,
            last_tx_hash: row.last_tx_hash,
            failure_count: row.failure_count,
            last_error: row.last_error,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request body for POST /api/subscriptions.
///
/// Mirrors an on-chain `create_subscription` call the client has already
/// made (or is about to make) with their own wallet; this just indexes it.
#[derive(Debug, Deserialize)]
pub struct CreateSubscriptionRequest {
    pub payer_account: String,
    pub recipient_account: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub interval_seconds: i64,
    /// When the first execution should occur. Defaults to `now + interval`.
    pub first_execution_at: Option<DateTime<Utc>>,
    /// The id the Soroban contract assigned to this subscription, if the
    /// client already submitted the on-chain `create_subscription` call.
    pub onchain_subscription_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ListSubscriptionsParams {
    pub status: Option<SubscriptionStatus>,
}
