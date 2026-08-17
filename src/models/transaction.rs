use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// All possible states a StellarSend transaction can be in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "transaction_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum TransactionStatus {
    Pending,
    Submitted,
    /// Submission's outcome is unknown — Horizon never gave us a definitive
    /// answer (client-side timeout, connection drop, or a crash before our
    /// post-submission UPDATE committed). Distinct from `Failed`: the
    /// transaction may still have landed on-chain, so it's never safe to
    /// treat this the same as a definite rejection or let a caller blindly
    /// retry. Resolved by `ReconciliationService` querying Horizon directly
    /// by the pre-submission-computed hash (#30).
    #[sqlx(rename = "submitted_unconfirmed")]
    #[serde(rename = "submitted_unconfirmed")]
    SubmittedUnconfirmed,
    Completed,
    Failed,
    Expired,
}

impl std::fmt::Display for TransactionStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pending => write!(f, "pending"),
            Self::Submitted => write!(f, "submitted"),
            Self::SubmittedUnconfirmed => write!(f, "submitted_unconfirmed"),
            Self::Completed => write!(f, "completed"),
            Self::Failed => write!(f, "failed"),
            Self::Expired => write!(f, "expired"),
        }
    }
}

/// Raw database row for a transaction.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TransactionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub stellar_tx_hash: Option<String>,
    pub from_asset: String,
    pub to_asset: String,
    pub send_amount: String,
    pub receive_amount: Option<String>,
    pub source_account: String,
    pub destination_account: String,
    pub status: TransactionStatus,
    pub error_message: Option<String>,
    pub fee_xlm: Option<String>,
    pub path: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Public-facing transaction.
#[derive(Debug, Clone, Serialize)]
pub struct Transaction {
    pub id: Uuid,
    pub user_id: Uuid,
    pub stellar_tx_hash: Option<String>,
    pub from_asset: String,
    pub to_asset: String,
    pub send_amount: String,
    pub receive_amount: Option<String>,
    pub source_account: String,
    pub destination_account: String,
    pub status: TransactionStatus,
    pub error_message: Option<String>,
    pub fee_xlm: Option<String>,
    pub path: Option<serde_json::Value>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<TransactionRow> for Transaction {
    fn from(row: TransactionRow) -> Self {
        Self {
            id: row.id,
            user_id: row.user_id,
            stellar_tx_hash: row.stellar_tx_hash,
            from_asset: row.from_asset,
            to_asset: row.to_asset,
            send_amount: row.send_amount,
            receive_amount: row.receive_amount,
            source_account: row.source_account,
            destination_account: row.destination_account,
            status: row.status,
            error_message: row.error_message,
            fee_xlm: row.fee_xlm,
            path: row.path,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Data needed to create a new transaction record.
#[derive(Debug, Clone)]
pub struct CreateTransaction {
    pub user_id: Uuid,
    pub from_asset: String,
    pub to_asset: String,
    pub send_amount: String,
    pub receive_amount: Option<String>,
    pub source_account: String,
    pub destination_account: String,
    pub fee_xlm: Option<String>,
    pub path: Option<serde_json::Value>,
}

/// Query parameters for listing transactions.
///
/// Two pagination modes:
/// - **Cursor (default, cheap).** Pass `cursor` (opaque, from a previous
///   response's `next_cursor`) or omit both `cursor` and `page` for the
///   first page. Never runs a `COUNT(*)`.
/// - **Legacy offset (`page`/`per_page`).** Pass `page` to keep the old
///   page-number behavior, including its `COUNT(*)`-per-request cost. Kept
///   for existing consumers during the transition — see README/API docs.
///
/// `include_total=true` opts either mode into also computing `total`/
/// `total_pages` (always computed in legacy mode regardless of this flag).
#[derive(Debug, Deserialize)]
pub struct TransactionListParams {
    pub status: Option<TransactionStatus>,
    pub from_asset: Option<String>,
    pub to_asset: Option<String>,
    pub page: Option<u32>,
    pub per_page: Option<u32>,
    pub cursor: Option<String>,
    pub include_total: Option<bool>,
}

/// Paginated response.
///
/// `total`/`page`/`total_pages` are `None` in cursor mode unless
/// `include_total=true` was requested — computing them costs a full
/// `COUNT(*)`, which cursor mode exists specifically to avoid by default.
/// `next_cursor` is `None` in legacy offset mode and in cursor mode once
/// the last page has been reached.
#[derive(Debug, Serialize)]
pub struct PaginatedTransactions {
    pub items: Vec<Transaction>,
    pub total: Option<i64>,
    pub page: Option<u32>,
    pub per_page: u32,
    pub total_pages: Option<u32>,
    pub next_cursor: Option<String>,
}
