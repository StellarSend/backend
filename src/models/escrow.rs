use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, sqlx::Type)]
#[sqlx(type_name = "escrow_status", rename_all = "lowercase")]
#[serde(rename_all = "lowercase")]
pub enum EscrowStatus {
    Active,
    Released,
    Refunded,
    Cancelled,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct EscrowRow {
    pub id: Uuid,
    pub depositor_id: Uuid,
    pub depositor_account: String,
    pub beneficiary_account: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub unlock_time: DateTime<Utc>,
    pub arbiter_account: Option<String>,
    pub status: EscrowStatus,
    pub onchain_escrow_id: Option<String>,
    pub funding_tx_hash: Option<String>,
    pub release_tx_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Escrow {
    pub id: Uuid,
    pub depositor_id: Uuid,
    pub depositor_account: String,
    pub beneficiary_account: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub unlock_time: DateTime<Utc>,
    pub arbiter_account: Option<String>,
    pub status: EscrowStatus,
    pub onchain_escrow_id: Option<String>,
    pub funding_tx_hash: Option<String>,
    pub release_tx_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<EscrowRow> for Escrow {
    fn from(row: EscrowRow) -> Self {
        Self {
            id: row.id,
            depositor_id: row.depositor_id,
            depositor_account: row.depositor_account,
            beneficiary_account: row.beneficiary_account,
            asset_code: row.asset_code,
            asset_issuer: row.asset_issuer,
            amount: row.amount,
            unlock_time: row.unlock_time,
            arbiter_account: row.arbiter_account,
            status: row.status,
            onchain_escrow_id: row.onchain_escrow_id,
            funding_tx_hash: row.funding_tx_hash,
            release_tx_hash: row.release_tx_hash,
            created_at: row.created_at,
            updated_at: row.updated_at,
        }
    }
}

/// Request body for POST /api/escrows. The client funds the escrow with its
/// own signed transaction (`create_escrow` on-chain); this records it.
#[derive(Debug, Deserialize)]
pub struct CreateEscrowRequest {
    pub depositor_account: String,
    pub beneficiary_account: String,
    pub asset_code: String,
    pub asset_issuer: Option<String>,
    pub amount: String,
    pub unlock_time: DateTime<Utc>,
    pub arbiter_account: Option<String>,
    pub onchain_escrow_id: Option<String>,
    pub funding_tx_hash: Option<String>,
}

/// Who is attempting to trigger the escrow action — determines which rules
/// apply (mirrors the contract's own authorization rules).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EscrowActor {
    Depositor,
    Beneficiary,
    Arbiter,
}

/// Request body for POST /api/escrows/:id/release and /refund.
#[derive(Debug, Deserialize)]
pub struct EscrowActionRequest {
    /// Which party is invoking this action (validated against the escrow's
    /// recorded accounts).
    pub actor: EscrowActor,
    /// The Stellar account of the caller (must match the role claimed by
    /// `actor`).
    pub account: String,
}
