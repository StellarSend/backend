use crate::{
    error::AppResult,
    services::{
        soroban::SorobanService, stellar::StellarService, subscription::SubscriptionService,
        transaction::TransactionService,
    },
    AppState,
};
use axum::{extract::State, Json};
use serde_json::{json, Value};
use std::sync::Arc;

/// POST /api/keeper/run-subscriptions
///
/// Manually trigger one keeper pass over due subscriptions. The same logic
/// also runs on a background tokio interval (see `main.rs`); this endpoint
/// exists so a pass can be triggered on-demand (e.g. from an ops dashboard,
/// or an external scheduler like a cron-triggered HTTP call) without
/// standing up additional infrastructure. Requires no auth beyond running
/// on a trusted network — consider putting this behind a reverse-proxy
/// allowlist in production.
pub async fn run_subscriptions(State(state): State<Arc<AppState>>) -> AppResult<Json<Value>> {
    let sub_svc = SubscriptionService::new(state.pool.clone());
    let stellar = StellarService::new(&state.config.horizon_url);
    let soroban = SorobanService::new(&state.config.soroban_rpc_url);
    let tx_svc = TransactionService::new(state.pool.clone());

    let summary = sub_svc
        .run_due_executions(&state.config, &stellar, &soroban, &tx_svc)
        .await?;

    Ok(Json(json!({ "success": true, "data": summary })))
}
