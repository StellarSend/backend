use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::{
        batch_payment::SendBatchPaymentRequest,
        payment::{QuoteRequest, SendPaymentRequest},
    },
    services::{
        batch::BatchPaymentService, payment::PaymentService, stellar::StellarService,
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

// ─── Quote ────────────────────────────────────────────────────────────────────

/// POST /api/payments/quote
///
/// Returns an estimated receive amount, implied rate, fee, and best path for
/// a potential payment without committing it.  Authentication is required so
/// we can pre-create a pending transaction record for idempotency.
pub async fn get_quote(
    State(state): State<Arc<AppState>>,
    _auth: AuthUser,
    Json(req): Json<QuoteRequest>,
) -> AppResult<Json<Value>> {
    let payment_svc = PaymentService::new(StellarService::new(&state.config.horizon_url));
    let quote = payment_svc.get_quote(&req).await?;

    Ok(Json(json!({
        "success": true,
        "data": quote
    })))
}

// ─── Send ─────────────────────────────────────────────────────────────────────

/// POST /api/payments/send
///
/// The client builds and signs the Stellar transaction using the quote data,
/// then posts the signed XDR here.  We relay it to Horizon and record the
/// outcome.
pub async fn send_payment(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(req): Json<SendPaymentRequest>,
) -> AppResult<Json<Value>> {
    let stellar_svc = StellarService::new(&state.config.horizon_url);
    let payment_svc = PaymentService::new(stellar_svc);
    let tx_svc = TransactionService::new(state.pool.clone());

    let result = payment_svc
        .execute_send(auth.user_id, &req, &tx_svc)
        .await?;

    Ok(Json(json!({
        "success": true,
        "data": result
    })))
}

// ─── Batch / split ─────────────────────────────────────────────────────────────

/// POST /api/payments/batch
///
/// Same non-custodial pattern as `/send`: the client builds and signs a
/// single Stellar transaction with one payment operation per recipient
/// (or a `send_batch_payment` Soroban call), then posts the signed XDR
/// here. We relay it once and record one transaction row per leg.
pub async fn send_batch_payment(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(req): Json<SendBatchPaymentRequest>,
) -> AppResult<Json<Value>> {
    let stellar_svc = StellarService::new(&state.config.horizon_url);
    let batch_svc = BatchPaymentService::new(state.pool.clone());

    let result = batch_svc.execute_batch(auth.user_id, &stellar_svc, &req).await?;

    Ok(Json(json!({
        "success": true,
        "data": result
    })))
}

// ─── Get by ID ────────────────────────────────────────────────────────────────

/// GET /api/payments/:id
///
/// Retrieves an internal payment (transaction) record plus the live Stellar
/// transaction from Horizon when a hash is available.
pub async fn get_payment(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let tx_svc = TransactionService::new(state.pool.clone());
    let tx = tx_svc
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Payment".into()))?;

    // Ensure the caller owns this record.
    if tx.user_id != auth.user_id {
        return Err(AppError::Forbidden);
    }

    // Optionally fetch live Stellar data if we have a hash.
    let stellar_data: Option<Value> = if let Some(ref hash) = tx.stellar_tx_hash {
        let stellar_svc = StellarService::new(&state.config.horizon_url);
        match stellar_svc.fetch_transaction(hash).await {
            Ok(stellar_tx) => Some(serde_json::to_value(stellar_tx).unwrap_or(Value::Null)),
            Err(_) => None,
        }
    } else {
        None
    };

    Ok(Json(json!({
        "success": true,
        "data": {
            "payment": tx,
            "stellar": stellar_data
        }
    })))
}
