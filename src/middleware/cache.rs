// Response caching middleware for exchange rate and account endpoints
// Uses ETag + If-None-Match for HTTP-level caching
// Added by Zara Mensah (#148)
pub const CACHE_MIDDLEWARE_VERSION: &str = "1.0";
