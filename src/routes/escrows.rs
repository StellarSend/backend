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

/// POST /api/escrows/:id/release/build
///
/// Builds an unsigned `release_escrow` invocation sourced from the calling
/// party's own account (`req.account`), for the client to sign with its own
/// wallet — the on-chain call requires the actual beneficiary/arbiter
/// signature (`caller.require_auth()`), so this cannot be keeper-executed.
pub async fn build_release_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<EscrowActionRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let stellar = StellarService::new(&state.config.horizon_url);
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);

    let xdr = svc
        .build_action_tx(&state.config, &stellar, &soroban, id, EscrowAction::Release, &req)
        .await?;

    Ok(Json(json!({ "success": true, "data": { "xdr": xdr } })))
}

/// POST /api/escrows/:id/release
///
/// Relays a client-signed `release_escrow` envelope (from the `/release/build`
/// step) and records the resulting on-chain state.
pub async fn release_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SignedActionRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);
    let tx_svc = TransactionService::new(state.pool.clone());

    let escrow = svc
        .submit_signed_action(&soroban, &tx_svc, id, EscrowAction::Release, &req.signed_xdr)
        .await?;

    Ok(Json(json!({ "success": true, "data": escrow })))
}

/// POST /api/escrows/:id/refund/build
///
/// Same as `/release/build`, for `refund_escrow`.
pub async fn build_refund_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<EscrowActionRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let stellar = StellarService::new(&state.config.horizon_url);
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);

    let xdr = svc
        .build_action_tx(&state.config, &stellar, &soroban, id, EscrowAction::Refund, &req)
        .await?;

    Ok(Json(json!({ "success": true, "data": { "xdr": xdr } })))
}

/// POST /api/escrows/:id/refund
///
/// Relays a client-signed `refund_escrow` envelope and records the result.
pub async fn refund_escrow(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Path(id): Path<Uuid>,
    Json(req): Json<SignedActionRequest>,
) -> AppResult<Json<Value>> {
    let svc = EscrowService::new(state.pool.clone());
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);
    let tx_svc = TransactionService::new(state.pool.clone());

    let escrow = svc
        .submit_signed_action(&soroban, &tx_svc, id, EscrowAction::Refund, &req.signed_xdr)
        .await?;

    Ok(Json(json!({ "success": true, "data": escrow })))
}

/// Request body for POST /api/escrows/:id/release and /refund.
#[derive(Debug, serde::Deserialize)]
pub struct SignedActionRequest {
    pub signed_xdr: String,
}
