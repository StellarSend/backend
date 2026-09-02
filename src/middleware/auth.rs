use crate::{
    error::AppError,
    models::user::JwtClaims,
};
use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts},
};
use jsonwebtoken::{decode, Algorithm, DecodingKey, Validation};
use std::sync::Arc;
use uuid::Uuid;

/// Authenticated user extracted from the Bearer JWT on every protected route.
#[derive(Debug, Clone)]
pub struct AuthUser {
    pub user_id: Uuid,
    pub email: String,
    pub claims: JwtClaims,
}

/// App state type alias (mirrors what main.rs exposes).
pub type AppState = Arc<crate::AppState>;

#[axum::async_trait]
impl FromRequestParts<AppState> for AuthUser {
    type Rejection = AppError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &AppState,
    ) -> Result<Self, Self::Rejection> {
        // 1. Extract the Authorization header.
        let auth_header = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .ok_or(AppError::Unauthorized)?;

        // 2. Expect "Bearer <token>".
        let token = auth_header
            .strip_prefix("Bearer ")
            .ok_or(AppError::Unauthorized)?;

        // 3. Decode and validate the JWT.
        let claims = decode_jwt(token, &state.config.jwt_secret)?;

        let user_id = Uuid::parse_str(&claims.sub).map_err(|_| AppError::InvalidToken)?;

        Ok(AuthUser {
            user_id,
            email: claims.email.clone(),
            claims,
        })
    }
}

/// Issue a signed JWT for the given user.
pub fn issue_jwt(
    user_id: Uuid,
    email: &str,
    secret: &str,
    expiry_hours: i64,
) -> Result<String, AppError> {
    use jsonwebtoken::{encode, EncodingKey, Header};

    let now = chrono::Utc::now();
    let exp = now + chrono::Duration::hours(expiry_hours);

    let claims = JwtClaims {
        sub: user_id.to_string(),
        email: email.to_string(),
        iat: now.timestamp(),
        exp: exp.timestamp(),
    };

    let token = encode(
        &Header::default(),
        &claims,
        &EncodingKey::from_secret(secret.as_bytes()),
    )?;

    Ok(token)
}

/// Decode and validate a JWT string, returning its claims.
pub fn decode_jwt(token: &str, secret: &str) -> Result<JwtClaims, AppError> {
    let mut validation = Validation::new(Algorithm::HS256);
    validation.validate_exp = true;

    let data = decode::<JwtClaims>(
        token,
        &DecodingKey::from_secret(secret.as_bytes()),
        &validation,
    )
    .map_err(|e| match e.kind() {
        jsonwebtoken::errors::ErrorKind::ExpiredSignature => AppError::TokenExpired,
        _ => AppError::InvalidToken,
    })?;

    Ok(data.claims)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::Request;
    use jsonwebtoken::{encode, EncodingKey, Header};
    use sqlx::PgPool;

    const TEST_SECRET: &str = "super-secret-jwt-key-must-be-at-least-32-chars-long";
    const OTHER_SECRET: &str = "another-secret-jwt-key-must-be-32-chars-or-more!!";

    fn test_app_state(secret: &str) -> AppState {
        let config = crate::Config {
            port: 8080,
            host: "0.0.0.0".into(),
            database_url: "postgres://user:pass@localhost/db".into(),
            database_max_connections: 5,
            database_min_connections: 1,
            database_connect_timeout_secs: 10,
            jwt_secret: secret.into(),
            jwt_expiry_hours: 24,
            horizon_url: "https://horizon-testnet.stellar.org".into(),
            stellar_network_passphrase: "Test SDF Network ; September 2015".into(),
            soroban_rpc_url: "https://soroban-testnet.stellar.org".into(),
            keeper_secret_key: None,
            subscription_contract_id: None,
            escrow_contract_id: None,
            keeper_poll_interval_secs: 60,
            keeper_enabled: false,
            reconciliation_poll_interval_secs: 30,
            reconciliation_stale_after_secs: 60,
            rate_cache_ttl_secs: 60,
            allowed_origins: vec!["*".into()],
            app_env: crate::config::AppEnv::Development,
        };
        Arc::new(crate::AppState {
            pool: PgPool::connect_lazy("postgres://user:pass@localhost/db").unwrap(),
            config,
            loop_health: crate::BackgroundLoopHealth::default(),
        })
    }

    #[test]
    fn round_trip_issue_and_decode_recovers_claims() {
        let user_id = Uuid::new_v4();
        let email = "alice@example.com";
        let token = issue_jwt(user_id, email, TEST_SECRET, 24).expect("issue_jwt should succeed");

        let claims = decode_jwt(&token, TEST_SECRET).expect("decode_jwt should succeed");
        assert_eq!(claims.sub, user_id.to_string());
        assert_eq!(claims.email, email);
        assert!(claims.exp > claims.iat);
        assert_eq!(claims.exp - claims.iat, 24 * 3600);
    }

    #[test]
    fn decode_fails_with_invalid_token_when_secret_mismatches() {
        let user_id = Uuid::new_v4();
        let email = "bob@example.com";
        let token = issue_jwt(user_id, email, TEST_SECRET, 24).expect("issue_jwt should succeed");

        let err = decode_jwt(&token, OTHER_SECRET).unwrap_err();
        assert!(
            matches!(err, AppError::InvalidToken),
            "expected InvalidToken on secret mismatch, got {err:?}"
        );
    }

    #[test]
    fn decode_specifically_fails_with_token_expired_when_past_exp() {
        let user_id = Uuid::new_v4();
        let email = "charlie@example.com";
        // Issue token with negative expiry hours to produce an expired token
        let token = issue_jwt(user_id, email, TEST_SECRET, -1).expect("issue_jwt should succeed");

        let err = decode_jwt(&token, TEST_SECRET).unwrap_err();
        assert!(
            matches!(err, AppError::TokenExpired),
            "expected TokenExpired on expired token, got {err:?}"
        );
    }

    #[test]
    fn decode_fails_when_signature_is_tampered() {
        let user_id = Uuid::new_v4();
        let email = "dave@example.com";
        let token = issue_jwt(user_id, email, TEST_SECRET, 24).expect("issue_jwt should succeed");

        let mut parts: Vec<&str> = token.split('.').collect();
        assert_eq!(parts.len(), 3);
        // Tamper with the signature portion
        let mut tampered_sig = parts[2].to_string();
        if tampered_sig.ends_with('A') {
            tampered_sig.replace_range(tampered_sig.len() - 1.., "B");
        } else {
            tampered_sig.replace_range(tampered_sig.len() - 1.., "A");
        }
        parts[2] = &tampered_sig;
        let tampered_token = parts.join(".");

        let err = decode_jwt(&tampered_token, TEST_SECRET).unwrap_err();
        assert!(
            matches!(err, AppError::InvalidToken),
            "expected InvalidToken on tampered signature, got {err:?}"
        );
    }

    #[test]
    fn decode_fails_when_algorithm_is_different() {
        let user_id = Uuid::new_v4();
        let email = "eve@example.com";
        let now = chrono::Utc::now();
        let exp = now + chrono::Duration::hours(1);

        let claims = JwtClaims {
            sub: user_id.to_string(),
            email: email.to_string(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
        };

        // Create token explicitly signed with HS384 instead of HS256
        let header = Header::new(Algorithm::HS384);
        let token = encode(
            &header,
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .expect("encode with HS384 should succeed");

        let err = decode_jwt(&token, TEST_SECRET).unwrap_err();
        assert!(
            matches!(err, AppError::InvalidToken),
            "expected InvalidToken when token algorithm is HS384 instead of HS256, got {err:?}"
        );
    }

    #[tokio::test]
    async fn extractor_rejects_missing_authorization_header() {
        let state = test_app_state(TEST_SECRET);
        let req = Request::builder()
            .uri("/api/protected")
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::Unauthorized),
            "expected Unauthorized for missing Authorization header, got {err:?}"
        );
    }

    #[tokio::test]
    async fn extractor_rejects_malformed_auth_scheme() {
        let state = test_app_state(TEST_SECRET);
        let req = Request::builder()
            .uri("/api/protected")
            .header(header::AUTHORIZATION, "Basic dXNlcjpwYXNz")
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::Unauthorized),
            "expected Unauthorized for non-Bearer Authorization scheme, got {err:?}"
        );
    }

    #[tokio::test]
    async fn extractor_rejects_non_uuid_sub() {
        let state = test_app_state(TEST_SECRET);
        let now = chrono::Utc::now();
        let exp = now + chrono::Duration::hours(1);

        let claims = JwtClaims {
            sub: "not-a-valid-uuid".into(),
            email: "frank@example.com".into(),
            iat: now.timestamp(),
            exp: exp.timestamp(),
        };

        let token = encode(
            &Header::default(),
            &claims,
            &EncodingKey::from_secret(TEST_SECRET.as_bytes()),
        )
        .expect("encode should succeed");

        let req = Request::builder()
            .uri("/api/protected")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();

        let err = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .unwrap_err();
        assert!(
            matches!(err, AppError::InvalidToken),
            "expected InvalidToken for non-UUID sub claim, got {err:?}"
        );
    }

    #[tokio::test]
    async fn extractor_succeeds_with_valid_bearer_token() {
        let state = test_app_state(TEST_SECRET);
        let user_id = Uuid::new_v4();
        let email = "grace@example.com";
        let token = issue_jwt(user_id, email, TEST_SECRET, 24).expect("issue_jwt should succeed");

        let req = Request::builder()
            .uri("/api/protected")
            .header(header::AUTHORIZATION, format!("Bearer {token}"))
            .body(())
            .unwrap();
        let (mut parts, _) = req.into_parts();

        let auth_user = AuthUser::from_request_parts(&mut parts, &state)
            .await
            .expect("AuthUser extraction should succeed");
        assert_eq!(auth_user.user_id, user_id);
        assert_eq!(auth_user.email, email);
        assert_eq!(auth_user.claims.sub, user_id.to_string());
    }
}
