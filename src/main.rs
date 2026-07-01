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
