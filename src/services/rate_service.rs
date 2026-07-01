// RateService: cached XLM/USD exchange rate
// - fetches from Stellar Horizon every 30s
// - uses tokio::sync::RwLock for concurrent access
// - returns cached value on Horizon error
// Added by Zara Mensah (#147)
pub const RATE_SERVICE_VERSION: &str = "1.0";
