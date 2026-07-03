#![allow(dead_code)] // Public API surface — fields and methods are used by callers
use anyhow::Result;
use sqlx::PgPool;
use std::{net::SocketAddr, sync::Arc};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

mod config;
mod db;
mod error;
mod middleware;
mod models;
mod routes;
mod services;

pub use config::Config;

/// Shared application state — one instance, behind an `Arc`, referenced by
/// every route handler.
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env if present (silently ignored if missing).
    dotenv::dotenv().ok();

    // Initialise structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stellarsend_backend=debug,tower_http=debug,axum=info".into()),
        )
        .json()
        .init();

    tracing::info!("StellarSend backend starting…");

    // Load config.
    let config = Config::from_env()?;
    tracing::info!(
        host = %config.host,
        port = %config.port,
        horizon_url = %config.horizon_url,
        env = ?config.app_env,
        "Configuration loaded"
    );

    // Establish database pool and run migrations.
    let pool = db::create_pool(&config).await?;
    db::run_migrations(&pool).await?;

    let state = Arc::new(AppState { pool, config: config.clone() });

    // Spawn the keeper background loop: periodically scans for subscriptions
    // that are due for execution and submits the on-chain
    // `execute_subscription` call via the keeper service account. This is a
    // plain tokio interval task — no external scheduler/queue — kept simple
    // on purpose. It only runs if a keeper account and contract id are
    // configured; the manual `/api/keeper/run-subscriptions` endpoint always
    // works as a fallback trigger.
    if config.keeper_enabled {
        let keeper_state = state.clone();
        tokio::spawn(async move {
            run_keeper_loop(keeper_state).await;
        });
    } else {
        tracing::info!("Keeper background loop disabled (KEEPER_ENABLED=false)");
    }

    // Build CORS layer from config.
    let cors = middleware::cors::build_cors_layer(&config.allowed_origins);

    // Build the full router.
    let app = routes::build_router(state)
        // Request-id propagation (X-Request-Id).
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        // Structured request/response logging via Tower Trace.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    DefaultMakeSpan::new()
                        .level(Level::INFO)
                        .include_headers(false),
                )
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .include_headers(false),
                ),
        )
        // Reject requests that take longer than 30 s.
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(30)))
        // CORS.
        .layer(cors);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("Invalid bind address");

    tracing::info!(address = %addr, "Listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Background keeper loop: every `keeper_poll_interval_secs`, look for
/// subscriptions due for execution and submit the on-chain call for each.
/// Errors (including "keeper not configured") are logged and the loop keeps
/// running — a transient RPC/Horizon outage should not crash the server.
async fn run_keeper_loop(state: Arc<AppState>) {
    use services::{soroban::SorobanService, stellar::StellarService, subscription::SubscriptionService, transaction::TransactionService};

    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
        state.config.keeper_poll_interval_secs.max(5),
    ));

    loop {
        interval.tick().await;

        let sub_svc = SubscriptionService::new(state.pool.clone());
        let stellar = StellarService::new(&state.config.horizon_url);
        let soroban = SorobanService::new(&state.config.soroban_rpc_url);
        let tx_svc = TransactionService::new(state.pool.clone());

        match sub_svc
            .run_due_executions(&state.config, &stellar, &soroban, &tx_svc)
            .await
        {
            Ok(summary) => {
                if summary.considered > 0 {
                    tracing::info!(
                        executed = summary.executed,
                        failed = summary.failed,
                        considered = summary.considered,
                        "Keeper pass complete"
                    );
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "Keeper pass failed");
            }
        }
    }
}
