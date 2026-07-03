use crate::{
    error::{AppError, AppResult},
    middleware::auth::AuthUser,
    models::subscription::{CreateSubscriptionRequest, ListSubscriptionsParams},
    services::subscription::SubscriptionService,
    AppState,
};
use axum::{
    extract::{Path, Query, State},
    Json,
};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

/// POST /api/subscriptions
///
/// Records a recurring-payment subscription. If the client has already
/// submitted the on-chain `create_subscription` call, pass
/// `onchain_subscription_id` so the keeper knows which on-chain
/// subscription to execute later.
pub async fn create_subscription(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Json(req): Json<CreateSubscriptionRequest>,
) -> AppResult<Json<Value>> {
    let svc = SubscriptionService::new(state.pool.clone());
    let subscription = svc.create(auth.user_id, &req).await?;

    Ok(Json(json!({ "success": true, "data": subscription })))
}

/// GET /api/subscriptions
pub async fn list_subscriptions(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Query(params): Query<ListSubscriptionsParams>,
) -> AppResult<Json<Value>> {
    let svc = SubscriptionService::new(state.pool.clone());
    let subscriptions = svc.list_for_user(auth.user_id, &params).await?;

    Ok(Json(json!({ "success": true, "data": subscriptions })))
}

/// GET /api/subscriptions/:id
pub async fn get_subscription(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let svc = SubscriptionService::new(state.pool.clone());
    let subscription = svc
        .get_by_id(id)
        .await?
        .ok_or_else(|| AppError::NotFound("Subscription".into()))?;

    if subscription.payer_id != auth.user_id {
        return Err(AppError::Forbidden);
    }

    Ok(Json(json!({ "success": true, "data": subscription })))
}

/// POST /api/subscriptions/:id/cancel
///
/// Cancels the internal record. In production the client should also submit
/// the on-chain `cancel_subscription` call; the keeper will not execute a
/// subscription this endpoint has marked cancelled regardless.
pub async fn cancel_subscription(
    State(state): State<Arc<AppState>>,
    auth: AuthUser,
    Path(id): Path<Uuid>,
) -> AppResult<Json<Value>> {
    let svc = SubscriptionService::new(state.pool.clone());
    let subscription = svc.cancel(auth.user_id, id).await?;

    Ok(Json(json!({ "success": true, "data": subscription })))
}
