use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

/// Unified application error type.  Every variant maps to an HTTP status code
/// and a machine-readable `code` string so clients can act on errors
/// programmatically.
#[derive(Debug, Error)]
#[allow(dead_code)]
pub enum AppError {
    // ── Authentication ──────────────────────────────────────────────────────
    #[error("Authentication required")]
    Unauthorized,

    #[error("Invalid credentials")]
    InvalidCredentials,

    #[error("Token has expired")]
    TokenExpired,

    #[error("Invalid token")]
    InvalidToken,

    #[error("Insufficient permissions")]
    Forbidden,

    // ── Validation ──────────────────────────────────────────────────────────
    #[error("Validation error: {0}")]
    Validation(String),

    #[error("Invalid request: {0}")]
    BadRequest(String),

    // ── Not Found ───────────────────────────────────────────────────────────
    #[error("{0} not found")]
    NotFound(String),

    // ── Conflicts ───────────────────────────────────────────────────────────
    #[error("{0} already exists")]
    Conflict(String),

    // ── External services ───────────────────────────────────────────────────
    #[error("Stellar Horizon error: {0}")]
    HorizonError(String),

    #[error("Stellar transaction failed: {0}")]
    TransactionFailed(String),

    #[error("No payment path found between the requested assets")]
    NoPathFound,

    #[error("{0} has expired")]
    Expired(String),

    #[error("Soroban RPC error: {0}")]
    SorobanError(String),

    #[error("Keeper is not configured: {0}")]
    KeeperUnavailable(String),

    // ── Internal ────────────────────────────────────────────────────────────
    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("HTTP client error: {0}")]
    HttpClient(#[from] reqwest::Error),

    #[error("Internal server error")]
    Internal(#[from] anyhow::Error),

    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),
}

/// Wire format sent back to the client for every error.
#[allow(dead_code)]
#[derive(serde::Serialize)]
struct ErrorBody {
    success: bool,
    error: ErrorDetail,
}

#[allow(dead_code)]
#[derive(serde::Serialize)]
struct ErrorDetail {
    code: &'static str,
    message: String,
}

impl AppError {
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match self {
            Self::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            Self::InvalidCredentials => (StatusCode::UNAUTHORIZED, "INVALID_CREDENTIALS"),
            Self::TokenExpired => (StatusCode::UNAUTHORIZED, "TOKEN_EXPIRED"),
            Self::InvalidToken => (StatusCode::UNAUTHORIZED, "INVALID_TOKEN"),
            Self::Forbidden => (StatusCode::FORBIDDEN, "FORBIDDEN"),
            Self::Validation(_) => (StatusCode::UNPROCESSABLE_ENTITY, "VALIDATION_ERROR"),
            Self::BadRequest(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            Self::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            Self::Conflict(_) => (StatusCode::CONFLICT, "CONFLICT"),
            Self::HorizonError(_) => (StatusCode::BAD_GATEWAY, "HORIZON_ERROR"),
            Self::TransactionFailed(_) => (StatusCode::UNPROCESSABLE_ENTITY, "TRANSACTION_FAILED"),
            Self::NoPathFound => (StatusCode::UNPROCESSABLE_ENTITY, "NO_PATH_FOUND"),
            Self::Expired(_) => (StatusCode::GONE, "EXPIRED"),
            Self::SorobanError(_) => (StatusCode::BAD_GATEWAY, "SOROBAN_ERROR"),
            Self::KeeperUnavailable(_) => (StatusCode::SERVICE_UNAVAILABLE, "KEEPER_UNAVAILABLE"),
            Self::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "DATABASE_ERROR"),
            Self::HttpClient(_) => (StatusCode::BAD_GATEWAY, "HTTP_CLIENT_ERROR"),
            Self::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            Self::Jwt(_) => (StatusCode::UNAUTHORIZED, "JWT_ERROR"),
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();

        // For internal errors log them but don't leak internals to clients.
        let message = match &self {
            Self::Database(e) => {
                tracing::error!(error = %e, "Database error");
                "A database error occurred".to_string()
            }
            Self::Internal(e) => {
                tracing::error!(error = %e, "Internal error");
                "An internal error occurred".to_string()
            }
            other => other.to_string(),
        };

        let body = json!({
            "success": false,
            "error": {
                "code": code,
                "message": message,
            }
        });

        (status, Json(body)).into_response()
    }
}

/// Convenience result alias used throughout the codebase.
pub type AppResult<T> = Result<T, AppError>;
