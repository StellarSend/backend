pub mod accounts;
pub mod auth;
pub mod escrows;
pub mod health;
pub mod keeper;
pub mod payment_requests;
pub mod payments;
pub mod rates;
pub mod subscriptions;
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
        .route("/health/deep", get(health::health_deep))
        // ── Auth ────────────────────────────────────────────────────────────
        .route("/api/auth/register", post(auth::register))
        .route("/api/auth/login", post(auth::login))
        // ── Payments ────────────────────────────────────────────────────────
        .route("/api/payments/quote", post(payments::get_quote))
        .route("/api/payments/send", post(payments::send_payment))
        .route("/api/payments/batch", post(payments::send_batch_payment))
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
        // ── Subscriptions (recurring payments) ─────────────────────────────
        .route(
            "/api/subscriptions",
            get(subscriptions::list_subscriptions).post(subscriptions::create_subscription),
        )
        .route("/api/subscriptions/:id", get(subscriptions::get_subscription))
        .route(
            "/api/subscriptions/:id/cancel",
            post(subscriptions::cancel_subscription),
        )
        // ── Payment requests / invoicing ────────────────────────────────────
        .route(
            "/api/payment-requests",
            get(payment_requests::list_payment_requests).post(payment_requests::create_payment_request),
        )
        .route(
            "/api/payment-requests/:id",
            get(payment_requests::get_payment_request),
        )
        .route(
            "/api/payment-requests/:id/fulfill",
            post(payment_requests::fulfill_payment_request),
        )
        .route(
            "/api/payment-requests/:id/cancel",
            post(payment_requests::cancel_payment_request),
        )
        // ── Escrow / conditional transfers ──────────────────────────────────
        .route(
            "/api/escrows",
            get(escrows::list_escrows).post(escrows::create_escrow),
        )
        .route("/api/escrows/:id", get(escrows::get_escrow))
        .route("/api/escrows/:id/release/build", post(escrows::build_release_escrow))
        .route("/api/escrows/:id/release", post(escrows::release_escrow))
        .route("/api/escrows/:id/refund/build", post(escrows::build_refund_escrow))
        .route("/api/escrows/:id/refund", post(escrows::refund_escrow))
        // ── Keeper (manual trigger for the recurring-payment background job) ─
        .route("/api/keeper/run-subscriptions", post(keeper::run_subscriptions))
        .with_state(state)
}

/// GET /health — lightweight liveness probe.
///
/// Includes each background loop's last-tick unix timestamp (`null` if it
/// has never ticked — e.g. the keeper loop when `KEEPER_ENABLED=false`) so
/// an operator can tell those loops are actually still alive, not just that
/// the HTTP server is (#50) — the process staying up says nothing about
/// whether the loops backing it are.
async fn health_check(
    axum::extract::State(state): axum::extract::State<Arc<AppState>>,
) -> axum::Json<serde_json::Value> {
    axum::Json(serde_json::json!({
        "success": true,
        "data": {
            "status": "ok",
            "service": "stellarsend-backend",
            "background_loops": {
                "keeper_last_tick_at": state.loop_health.keeper_last_tick_at(),
                "reconciliation_last_tick_at": state.loop_health.reconciliation_last_tick_at(),
            }
        }
    }))
}
