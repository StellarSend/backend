use crate::{
    error::AppResult,
    middleware::auth::AuthUser,
    services::stellar::StellarService,
    validation::validate_stellar_address,
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
