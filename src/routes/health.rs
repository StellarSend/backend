// Health check endpoints
// GET /health       - liveness probe (always 200 if process alive)
// GET /health/deep  - readiness probe (checks DB + Horizon connectivity)
// Added by Chiamaka Eze (#159)
pub const HEALTH_ROUTER_VERSION: &str = "1.0";

use crate::AppState;
use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// GET /health/deep — readiness probe checking database and Horizon upstream connectivity.
pub async fn health_deep(
    State(state): State<Arc<AppState>>,
) -> Response {
    let mut is_healthy = true;
    let mut database_status = "ok";
    let mut horizon_status = "ok";

    // 1. Check Database connectivity with timeout
    let db_check = tokio::time::timeout(
        Duration::from_secs(2),
        sqlx::query("SELECT 1").execute(&state.pool),
    )
    .await;

    match db_check {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "health_deep: database check failed");
            database_status = "error";
            is_healthy = false;
        }
        Err(_) => {
            tracing::warn!("health_deep: database check timed out");
            database_status = "timeout";
            is_healthy = false;
        }
    }

    // 2. Check Horizon connectivity with timeout
    let horizon_url = format!("{}/", state.config.horizon_url.trim_end_matches('/'));
    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(2))
        .build();

    match client {
        Ok(c) => match c.get(&horizon_url).send().await {
            Ok(resp) => {
                if resp.status().is_success() {
                    // Horizon root responded successfully
                } else {
                    tracing::warn!(status = %resp.status(), "health_deep: horizon check returned non-200");
                    horizon_status = "error";
                    is_healthy = false;
                }
            }
            Err(e) => {
                tracing::warn!(error = %e, "health_deep: horizon check failed");
                horizon_status = "unreachable";
                is_healthy = false;
            }
        },
        Err(e) => {
            tracing::warn!(error = %e, "health_deep: failed to build HTTP client for horizon check");
            horizon_status = "error";
            is_healthy = false;
        }
    }

    let status_code = if is_healthy {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };

    let body = json!({
        "success": is_healthy,
        "data": {
            "status": if is_healthy { "ok" } else { "degraded" },
            "database": database_status,
            "horizon": horizon_status,
        }
    });

    (status_code, Json(body)).into_response()
}
