use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::transaction::TransactionListParams,
    services::transaction::TransactionService,
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

// ─── List ─────────────────────────────────────────────────────────────────────

/// GET /api/transactions
///
/// Returns a paginated, optionally-filtered list of the authenticated user's
/// transaction history.
///
/// Query params:
/// - `status`      – filter by status (pending | submitted | completed | failed | expired)
/// - `from_asset`  – filter by source asset code
/// - `to_asset`    – filter by destination asset code
/// - `page`        – page number (default: 1)
/// - `per_page`    – results per page (default: 20, max: 100)
pub async fn list_transactions(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(params): Query<TransactionListParams>,
) -> AppResult<Json<Value>> {
    let tx_svc = TransactionService::new(state.pool.clone());
    let paginated = tx_svc.list_for_user(auth.user_id, &params).await?;

    Ok(Json(json!({
        "success": true,
        "data": paginated
    })))
}

// ─── Get by ID ────────────────────────────────────────────────────────────────

/// GET /api/transactions/:id
///
/// Returns the full detail of a single transaction.  Returns 403 if the
/// authenticated user does not own the record.
pub async fn get_transaction(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let tx_svc = TransactionService::new(state.pool.clone());
    let tx = tx_svc
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Transaction".into()))?;

    if tx.user_id != auth.user_id {
        return Err(AppError::Forbidden);
    }

    Ok(Json(json!({
        "success": true,
        "data": tx
    })))
}
