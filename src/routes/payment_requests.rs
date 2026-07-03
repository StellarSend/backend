use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::payment_request::{
        CreatePaymentRequestRequest, FulfillPaymentRequestRequest, PaymentRequestWithShareLink,
    },
    services::payment_request::PaymentRequestService,
    AppState,
};
use axum::{
    extract::{Path, State},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// POST /api/payment-requests
///
/// Creates a shareable payment request ("invoice"). Returns a share URL and
/// a minimal QR-encodable payload — this codebase has no existing
/// QR-image-generation service, so we return the raw payload for the client
/// to render as a QR code itself rather than fabricate one.
pub async fn create_payment_request(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(req): Json<CreatePaymentRequestRequest>,
) -> AppResult<Json<Value>> {
    let svc = PaymentRequestService::new(state.pool.clone());
    let request = svc.create(auth.user_id, &req).await?;

    let share_url = format!("/api/payment-requests/{}", request.id);
    let qr_payload = json!({
        "type": "stellarsend_payment_request",
        "id": request.id,
        "destination": request.requester_account,
        "asset_code": request.asset_code,
        "asset_issuer": request.asset_issuer,
        "amount": request.amount,
        "memo": request.memo,
    })
    .to_string();

    Ok(Json(json!({
        "success": true,
        "data": PaymentRequestWithShareLink {
            request,
            share_url,
            qr_payload,
        }
    })))
}

/// GET /api/payment-requests/:id
///
/// Public lookup (no auth) so a payer who received a share link/QR code can
/// resolve the request details to prefill their send — mirrors how invoices
/// are normally shared.
pub async fn get_payment_request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let svc = PaymentRequestService::new(state.pool.clone());
    let request = svc
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Payment request".into()))?;

    Ok(Json(json!({ "success": true, "data": request })))
}

/// GET /api/payment-requests
pub async fn list_payment_requests(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
) -> AppResult<Json<Value>> {
    let svc = PaymentRequestService::new(state.pool.clone());
    let requests = svc.list_for_user(auth.user_id).await?;

    Ok(Json(json!({ "success": true, "data": requests })))
}

/// POST /api/payment-requests/:id/fulfill
pub async fn fulfill_payment_request(
    State(state): State<Arc<AppState>>,
    Path(id): Path<Uuid>,
    Json(req): Json<FulfillPaymentRequestRequest>,
) -> AppResult<Json<Value>> {
    let svc = PaymentRequestService::new(state.pool.clone());
    let request = svc.fulfill(id, &req).await?;

    Ok(Json(json!({ "success": true, "data": request })))
}

/// POST /api/payment-requests/:id/cancel
pub async fn cancel_payment_request(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let svc = PaymentRequestService::new(state.pool.clone());
    let request = svc.cancel(auth.user_id, id).await?;

    Ok(Json(json!({ "success": true, "data": request })))
}
