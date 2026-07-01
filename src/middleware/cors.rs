use tower_http::cors::{AllowHeaders, AllowMethods, AllowOrigin, CorsLayer};

/// Build the CORS layer from the configured list of allowed origins.
///
/// When `allowed_origins` contains `"*"`, or is empty, every origin is
/// permitted (suitable for development / public APIs).  In production, supply
/// an explicit list so that only known origins are whitelisted.
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
        .allow_methods(AllowMethods::any())
        .allow_headers(AllowHeaders::any())
}
