use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    services::stellar::StellarService,
    AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;

// ─── Account info ─────────────────────────────────────────────────────────────

/// GET /api/accounts/:address
///
/// Returns the full Horizon account record for a given Stellar public key.
/// Authentication is required to prevent unintentional crawling.
pub async fn get_account(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(address): Path<String>,
) -> AppResult<Json<Value>> {
    validate_stellar_address(&address)?;

    let stellar_svc = StellarService::new(&state.config.horizon_url);
    let account = stellar_svc.get_account(&address).await?;

    Ok(Json(json!({
        "success": true,
        "data": account
    })))
}

// ─── Balances ─────────────────────────────────────────────────────────────────

/// GET /api/accounts/:address/balances
///
/// Returns only the balance array for a Stellar account — a common sub-set of
/// the full account info, useful for wallet display.
pub async fn get_balances(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(address): Path<String>,
) -> AppResult<Json<Value>> {
    validate_stellar_address(&address)?;

    let stellar_svc = StellarService::new(&state.config.horizon_url);
    let balances = stellar_svc.get_balances(&address).await?;

    Ok(Json(json!({
        "success": true,
        "data": {
            "address": address,
            "balances": balances
        }
    })))
}

// ─── Helpers ──────────────────────────────────────────────────────────────────

/// Rudimentary Stellar address validation — must be 56 chars and start with G.
fn validate_stellar_address(address: &str) -> AppResult<()> {
    if address.len() != 56 || !address.starts_with('G') {
        return Err(AppError::BadRequest(
            "Invalid Stellar address: must be a 56-character G... public key".into(),
        ));
    }
    // Ensure it's alphanumeric (base32).
    if !address.chars().all(|c| c.is_ascii_alphanumeric()) {
        return Err(AppError::BadRequest(
            "Invalid Stellar address: contains non-base32 characters".into(),
        ));
    }
    Ok(())
}
