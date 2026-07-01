use crate::{
    error::{AppError, AppResult},
    middleware::auth::issue_jwt,
    models::user::{AuthResponse, CreateUserRequest, LoginRequest, User, UserRow},
    AppState,
};
use axum::{extract::State, Json};
use bcrypt::{hash, verify, DEFAULT_COST};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

// ─── Register ─────────────────────────────────────────────────────────────────

/// POST /api/auth/register
pub async fn register(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateUserRequest>,
) -> AppResult<Json<Value>> {
    // Validate inputs.
    let email = req.email.trim().to_lowercase();
    if email.is_empty() || !email.contains('@') {
        return Err(AppError::Validation("Invalid email address".into()));
    }
    if req.password.len() < 8 {
        return Err(AppError::Validation(
            "Password must be at least 8 characters".into(),
        ));
    }
    if req.full_name.trim().is_empty() {
        return Err(AppError::Validation("full_name is required".into()));
    }

    // Check uniqueness.
    let exists: bool = sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM users WHERE email = $1)")
        .bind(&email)
        .fetch_one(&state.pool)
        .await?;

    if exists {
        return Err(AppError::Conflict("Email".into()));
    }

    // Hash password.
    let password_hash =
        hash(req.password.as_bytes(), DEFAULT_COST).map_err(|e| {
            AppError::Internal(anyhow::anyhow!("bcrypt error: {e}"))
        })?;

    // Insert.
    let id = Uuid::new_v4();
    let row = sqlx::query_as::<_, UserRow>(
        r#"
        INSERT INTO users (id, email, password_hash, full_name, stellar_address)
        VALUES ($1, $2, $3, $4, $5)
        RETURNING *
        "#,
    )
    .bind(id)
    .bind(&email)
    .bind(&password_hash)
    .bind(req.full_name.trim())
    .bind(&req.stellar_address)
    .fetch_one(&state.pool)
    .await?;

    let user = User::from(row);

    // Issue JWT.
    let token = issue_jwt(
        user.id,
        &user.email,
        &state.config.jwt_secret,
        state.config.jwt_expiry_hours,
    )?;

    Ok(Json(json!({
        "success": true,
        "data": AuthResponse { token, user }
    })))
}

// ─── Login ────────────────────────────────────────────────────────────────────

/// POST /api/auth/login
pub async fn login(
    State(state): State<Arc<AppState>>,
    Json(req): Json<LoginRequest>,
) -> AppResult<Json<Value>> {
    let email = req.email.trim().to_lowercase();

    // Look up user.
    let row: Option<UserRow> =
        sqlx::query_as("SELECT * FROM users WHERE email = $1 AND is_active = TRUE")
            .bind(&email)
            .fetch_optional(&state.pool)
            .await?;

    let row = row.ok_or(AppError::InvalidCredentials)?;

    // Verify password.
    let valid = verify(req.password.as_bytes(), &row.password_hash).map_err(|e| {
        AppError::Internal(anyhow::anyhow!("bcrypt verify error: {e}"))
    })?;

    if !valid {
        return Err(AppError::InvalidCredentials);
    }

    let user = User::from(row);

    // Issue JWT.
    let token = issue_jwt(
        user.id,
        &user.email,
        &state.config.jwt_secret,
        state.config.jwt_expiry_hours,
    )?;

    Ok(Json(json!({
        "success": true,
        "data": AuthResponse { token, user }
    })))
}
