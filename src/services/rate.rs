use crate::{
    error::AppResult,
    services::stellar::StellarService,
};
use serde::Serialize;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

/// A single exchange rate entry.
#[derive(Debug, Clone, Serialize)]
pub struct ExchangeRate {
    pub from: String,
    pub to: String,
    pub rate: f64,
    pub timestamp: chrono::DateTime<chrono::Utc>,
}

/// In-memory cache entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    rate: ExchangeRate,
    expires_at: Instant,
}

/// RateService fetches exchange rates by querying the Stellar DEX via
/// Horizon's path-payment endpoint and caches results with a configurable TTL.
#[derive(Clone)]
pub struct RateService {
    stellar: StellarService,
    cache: Arc<Mutex<HashMap<String, CacheEntry>>>,
    ttl: Duration,
}

impl RateService {
    pub fn new(stellar: StellarService, cache_ttl_secs: u64) -> Self {
        Self {
            stellar,
            cache: Arc::new(Mutex::new(HashMap::new())),
            ttl: Duration::from_secs(cache_ttl_secs),
        }
    }

    /// Fetch (or return a cached) exchange rate between two assets.
    ///
    /// We probe the DEX by pricing a 1-unit strict-send path payment and
    /// reading the resulting destination amount as the implied rate.
    pub async fn fetch_rate(&self, from: &str, to: &str) -> AppResult<ExchangeRate> {
        let cache_key = format!("{from}:{to}");

        // Check cache.
        if let Some(entry) = self.cache.lock().unwrap().get(&cache_key) {
            if entry.expires_at > Instant::now() {
                return Ok(entry.rate.clone());
            }
        }

        // Build minimal Asset structs for the query.
        let from_asset = parse_asset_code(from);
        let to_asset = parse_asset_code(to);

        // Use a well-funded Horizon anchor account for path finding,
        // or fall back to a synthetic destination placeholder.
        // For rate queries the destination is not critical — we just need
        // the DEX to return paths.
        let dummy_destination = "GAAZI4TCR3TY5OJHCTJC2A4QSY6CJWJH5IAJTGKIN2ER7LBNVKOCCWN";

        let paths = self
            .stellar
            .get_path_payment_paths(&from_asset, &to_asset, "1", dummy_destination)
            .await;

        let rate_value: f64 = match paths {
            Ok(records) if !records.is_empty() => {
                let best = records
                    .iter()
                    .map(|r| r.destination_amount.parse::<f64>().unwrap_or(0.0))
                    .fold(0.0_f64, f64::max);
                best
            }
            _ => {
                // Fallback: if the DEX query fails for XLM pairs, use a synthetic rate.
                self.fallback_rate(from, to)
            }
        };

        let entry_rate = ExchangeRate {
            from: from.to_string(),
            to: to.to_string(),
            rate: rate_value,
            timestamp: chrono::Utc::now(),
        };

        // Cache the result.
        self.cache.lock().unwrap().insert(
            cache_key,
            CacheEntry {
                rate: entry_rate.clone(),
                expires_at: Instant::now() + self.ttl,
            },
        );

        Ok(entry_rate)
    }

    /// Fetch multiple rates at once.
    pub async fn fetch_rates(
        &self,
        pairs: &[(String, String)],
    ) -> AppResult<Vec<ExchangeRate>> {
        let mut results = Vec::with_capacity(pairs.len());
        for (from, to) in pairs {
            results.push(self.fetch_rate(from, to).await?);
        }
        Ok(results)
    }

    /// Hard-coded fallback rates for common pairs when the DEX returns no paths.
    /// In production these should come from a proper price oracle.
    fn fallback_rate(&self, from: &str, to: &str) -> f64 {
        match (from.to_uppercase().as_str(), to.to_uppercase().as_str()) {
            ("XLM", "USD") | ("XLM", "USDC") => 0.11,
            ("USD", "XLM") | ("USDC", "XLM") => 9.09,
            ("BTC", "XLM") => 700_000.0,
            ("XLM", "BTC") => 0.0000014,
            (a, b) if a == b => 1.0,
            _ => 0.0,
        }
    }

    /// Invalidate the entire cache (useful after known market events).
    pub fn invalidate_cache(&self) {
        self.cache.lock().unwrap().clear();
    }
}

/// Parse a simple asset code string into an `Asset`.
///
/// Supports:
/// - `"XLM"` → native
/// - `"USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN"` → issued
fn parse_asset_code(code: &str) -> crate::models::payment::Asset {
    if let Some((asset_code, issuer)) = code.split_once(':') {
        crate::models::payment::Asset {
            code: asset_code.to_uppercase(),
            issuer: Some(issuer.to_string()),
        }
    } else if code.to_uppercase() == "XLM" {
        crate::models::payment::Asset::native()
    } else {
        // Treat bare code as native-like (caller should pass full code:issuer).
        crate::models::payment::Asset {
            code: code.to_uppercase(),
            issuer: None,
        }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    #[tokio::test]
    async fn rate_service_caches_rates_within_ttl() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "_embedded": {
                "records": [
                    {
                        "source_asset_type": "native",
                        "source_amount": "1.0000000",
                        "destination_asset_type": "credit_alphanum4",
                        "destination_asset_code": "USDC",
                        "destination_asset_issuer": "GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN",
                        "destination_amount": "0.1250000",
                        "path": []
                    }
                ]
            }
        });

        // Mock Horizon /paths/strict-send endpoint expecting exactly 1 call
        Mock::given(method("GET"))
            .and(path("/paths/strict-send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let stellar_svc = StellarService::new(&mock_server.uri());
        let rate_svc = RateService::new(stellar_svc, 60);

        // First call: hits mock server
        let rate1 = rate_svc
            .fetch_rate("XLM", "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
            .await
            .expect("fetch_rate should succeed");
        assert_eq!(rate1.rate, 0.125);

        // Second call: should hit in-memory cache and NOT hit mock server again
        let rate2 = rate_svc
            .fetch_rate("XLM", "USDC:GA5ZSEJYB37JRC5AVCIA5MOP4RHTM335X2KGX3IHOJAPP5RE34K4KZVN")
            .await
            .expect("fetch_rate should succeed from cache");
        assert_eq!(rate2.rate, 0.125);
        assert_eq!(rate1.timestamp, rate2.timestamp);
    }

    #[tokio::test]
    async fn rate_service_falls_back_when_dex_empty() {
        let mock_server = MockServer::start().await;

        let response_body = serde_json::json!({
            "_embedded": {
                "records": []
            }
        });

        Mock::given(method("GET"))
            .and(path("/paths/strict-send"))
            .respond_with(ResponseTemplate::new(200).set_body_json(response_body))
            .expect(1)
            .mount(&mock_server)
            .await;

        let stellar_svc = StellarService::new(&mock_server.uri());
        let rate_svc = RateService::new(stellar_svc, 60);

        let rate = rate_svc
            .fetch_rate("XLM", "USD")
            .await
            .expect("fetch_rate fallback should succeed");
        assert_eq!(rate.rate, 0.11);
    }
}
