pub mod accounts;
pub mod auth;
pub mod payments;
pub mod rates;
pub mod transactions;

use crate::AppState;
use axum::{
    routing::{get, post},
    Router,
};
use std::sync::Arc;

/// Assemble the complete application router with all sub-routers mounted.
pub fn build_router(state: Arc<AppState>) -> Router {
    Router::new()
        // ── Health ──────────────────────────────────────────────────────────
        .route("/health", get(health_check))
        // ── Auth ────────────────────────────────────────────────────────────
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        // ── Payments ────────────────────────────────────────────────────────
        .route("/api/payments/quote", post(payments::get_quote))
        .route("/api/payments/send", post(payments::send_payment))
        .route("/api/payments/:id", get(payments::get_payment))
        // ── Transactions ────────────────────────────────────────────────────
        .route("/api/transactions", get(transactions::list_transactions))
        .route("/api/transactions/:id", get(transactions::get_transaction))
        // ── Accounts ────────────────────────────────────────────────────────
        .route(
            "/api/accounts/:address",
            get(accounts::get_account),
        )
        .route(
            "/api/accounts/:address/balances",
            get(accounts::get_balances),
        )
        // ── Rates ───────────────────────────────────────────────────────────
        .route("/api/rates", get(rates::get_rate))
        .with_state(state)
}

/// GET /health — lightweight liveness probe.
async fn health_check() -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "success": true,
        "data": {
            "status": "ok",
            "service": "stellarsend-backend"
        }
    }))
}
