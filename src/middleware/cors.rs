use axum::http::{header, Method};
use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

/// Build the CORS layer from the configured list of allowed origins.
///
/// When `allowed_origins` contains `"*"`, or is empty, every origin is
/// permitted (suitable for development / public APIs). In production, supply
/// an explicit list so that only known origins are whitelisted.
///
/// Methods and headers are restricted to the real surface required by the
/// StellarSend API (GET, POST, OPTIONS, and standard auth/content headers)
/// with a 1-hour preflight cache max age.
pub fn build_cors_layer(allowed_origins: &[String]) -> CorsLayer {
    let allow_origin: AllowOrigin = if allowed_origins.iter().any(|o| o == "*")
        || allowed_origins.is_empty()
    {
        AllowOrigin::any()
    } else {
        let headers: Vec<axum::http::HeaderValue> = allowed_origins
            .iter()
            .filter_map(|o| o.parse().ok())
            .collect();
        AllowOrigin::list(headers)
    };

    CorsLayer::new()
        .allow_origin(allow_origin)
        .allow_methods(AllowMethods::list([
            Method::GET,
            Method::POST,
            Method::OPTIONS,
        ]))
        .allow_headers(AllowHeaders::list([
            header::AUTHORIZATION,
            header::CONTENT_TYPE,
            header::ACCEPT,
            header::HeaderName::from_static("x-request-id"),
        ]))
        .max_age(std::time::Duration::from_secs(3600))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_cors_layer_with_wildcard_works() {
        let origins = vec!["*".to_string()];
        let _layer = build_cors_layer(&origins);
    }

    #[test]
    fn build_cors_layer_with_explicit_origins_works() {
        let origins = vec![
            "https://app.stellarsend.com".to_string(),
            "https://staging.stellarsend.com".to_string(),
        ];
        let _layer = build_cors_layer(&origins);
    }
}
