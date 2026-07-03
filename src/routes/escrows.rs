use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::escrow::{CreateEscrowRequest, EscrowActionRequest},
    services::{
        escrow::{EscrowAction, EscrowService},
        soroban::SorobanService,
        stellar::StellarService,
        transaction::TransactionService,
    },
    AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// POST /api/escrows
///
/// Records an escrow that the client has already funded (or is about to
/// fund) via its own signed `create_escrow` transaction.
pub async fn create_escrow(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(req): Json<CreateEscrowRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let escrow = svc.create(auth.user_id, &req).await?;

    Ok(Json(json!({ "success": true, "data": escrow })))
}

/// GET /api/escrows
pub async fn list_escrows(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let escrows = svc.list_for_user(auth.user_id).await?;

    Ok(Json(json!({ "success": true, "data": escrows })))
}

/// GET /api/escrows/:id
///
/// Looked up by unguessable id, not restricted to the depositor: the
/// beneficiary and arbiter (who are not necessarily StellarSend users with
/// an account in `users`) also need to be able to check escrow status.
pub async fn get_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let escrow = svc
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Escrow".into()))?;

    Ok(Json(json!({ "success": true, "data": escrow })))
}

/// POST /api/escrows/:id/release
///
/// Releases escrowed funds to the beneficiary. Allowed once `unlock_time`
/// has passed (beneficiary) or at any time (arbiter) — validated against
/// the roles recorded on the escrow, then relayed on-chain via the keeper.
pub async fn release_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<EscrowActionRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let stellar = StellarService::new(&state.config.horizon_url);
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);
    let tx_svc = TransactionService::new(state.pool.clone());

    let escrow = svc
        .execute_action(
            &state.config,
            &stellar,
            &soroban,
            &tx_svc,
            id,
            EscrowAction::Release,
            &req,
        )
        .await?;

    Ok(Json(json!({ "success": true, "data": escrow })))
}

/// POST /api/escrows/:id/refund
///
/// Returns escrowed funds to the depositor. Allowed once `unlock_time` has
/// passed (depositor) or at any time (arbiter).
pub async fn refund_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<EscrowActionRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let stellar = StellarService::new(&state.config.horizon_url);
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);
    let tx_svc = TransactionService::new(state.pool.clone());

    let escrow = svc
        .execute_action(
            &state.config,
            &stellar,
            &soroban,
            &tx_svc,
            id,
            EscrowAction::Refund,
            &req,
        )
        .await?;

    Ok(Json(json!({ "success": true, "data": escrow })))
}
