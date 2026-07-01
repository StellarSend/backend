use crate::{
    error::{AppError, AppResult},
    services::{rate::RateService, stellar::StellarService},
    AppState,
};
use axum::{
    extract::{Query, State},
    Json,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

// ─── Query params ─────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct RateQuery {
    /// Source asset — bare code for XLM, or "CODE:ISSUER" for issued assets.
    pub from: String,
    /// Destination asset — same format as `from`.
    pub to: String,
}

// ─── Handler ──────────────────────────────────────────────────────────────────

/// GET /api/rates?from=USD&to=XLM
///
/// Returns the current exchange rate between two Stellar assets by probing the
/// Stellar DEX.  Results are cached server-side for `RATE_CACHE_TTL_SECS`.
/// No authentication required — rates are public information.
pub async fn get_rate(
    State(state): State<Arc<AppState>>,
    Query(params): Query<RateQuery>,
) -> AppResult<Json<Value>> {
    if params.from.trim().is_empty() || params.to.trim().is_empty() {
        return Err(AppError::BadRequest(
            "Both `from` and `to` query parameters are required".into(),
        ));
    }

    let stellar_svc = StellarService::new(&state.config.horizon_url);
    let rate_svc = RateService::new(stellar_svc, state.config.rate_cache_ttl_secs);

    let rate = rate_svc.fetch_rate(&params.from, &params.to).await?;

    Ok(Json(json!({
        "success": true,
        "data": rate
    })))
}
