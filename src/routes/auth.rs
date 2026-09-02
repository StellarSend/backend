use crate::{
    db::is_unique_violation,
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

    // Fast-path uniqueness check to avoid unnecessary bcrypt hashing cost.
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

    // Insert with unique-violation protection against concurrent registration races (#55).
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
    .await
    .map_err(|e| {
        if is_unique_violation(&e) {
            AppError::Conflict("Email".into())
        } else {
            e.into()
        }
    })?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn is_unique_violation_helper_identifies_non_db_errors_as_false() {
        let err = sqlx::Error::RowNotFound;
        assert!(!is_unique_violation(&err));
    }
}

/// End-to-end database tests for registration unique-constraint race condition (#55).
/// Run with `cargo test -- --ignored` against a real database instance.
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::config::Config;
    use sqlx::postgres::PgPoolOptions;

    async fn test_pool() -> PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set to run this integration test");
        let pool = PgPoolOptions::new()
            .max_connections(5)
            .connect(&database_url)
            .await
            .expect("failed to connect to DATABASE_URL");
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .expect("failed to run migrations");
        pool
    }

    async fn cleanup_user_by_email(pool: &PgPool, email: &str) {
        let _ = sqlx::query("DELETE FROM users WHERE email = $1")
            .bind(email)
            .execute(pool)
            .await;
    }

    #[tokio::test]
    #[ignore]
    async fn concurrent_register_calls_for_same_email_returns_one_success_and_one_conflict() {
        let pool = test_pool().await;
        let email = format!("test-race-{}@example.test", Uuid::new_v4());

        let config = Config::from_env().unwrap_or_else(|_| Config {
            app_env: "test".into(),
            host: "127.0.0.1".into(),
            port: 8080,
            database_url: std::env::var("DATABASE_URL").unwrap(),
            database_max_connections: 5,
            database_min_connections: 1,
            database_connect_timeout_secs: 5,
            jwt_secret: "test_secret_for_registration_tests_12345".into(),
            jwt_expiry_hours: 24,
            horizon_url: "https://horizon-testnet.stellar.org".into(),
            stellar_network_passphrase: "Test SDF Network ; September 2015".into(),
            soroban_rpc_url: "https://soroban-testnet.stellar.org".into(),
            allowed_origins: vec!["*".into()],
            rate_cache_ttl_secs: 30,
            keeper_enabled: false,
            keeper_poll_interval_secs: 60,
            keeper_secret_key: None,
            escrow_contract_id: None,
            subscription_contract_id: None,
            reconciliation_poll_interval_secs: 60,
            reconciliation_stale_after_secs: 300,
        });

        let state = Arc::new(AppState {
            pool: pool.clone(),
            config,
            loop_health: crate::BackgroundLoopHealth::default(),
        });

        let req1 = CreateUserRequest {
            email: email.clone(),
            password: "password123".into(),
            full_name: "User One".into(),
            stellar_address: None,
        };
        let req2 = CreateUserRequest {
            email: email.clone(),
            password: "password123".into(),
            full_name: "User Two".into(),
            stellar_address: None,
        };

        let (res1, res2) = tokio::join!(
            register(State(state.clone()), Json(req1)),
            register(State(state.clone()), Json(req2))
        );

        let one_ok = res1.is_ok() || res2.is_ok();
        let one_conflict = matches!(&res1, Err(AppError::Conflict(_))) || matches!(&res2, Err(AppError::Conflict(_)));

        assert!(one_ok, "Exactly one concurrent registration should succeed");
        assert!(one_conflict, "The racing registration loser must receive Conflict (409), not a 500 Database error");

        cleanup_user_by_email(&pool, &email).await;
    }
}

