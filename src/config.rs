use anyhow::{bail, Context, Result};
use std::env;

/// Central application configuration, loaded once at startup from environment variables.
#[derive(Debug, Clone)]
pub struct Config {
    // Server
    pub port: u16,
    pub host: String,

    // Database
    pub database_url: String,
    pub database_max_connections: u32,
    pub database_min_connections: u32,
    pub database_connect_timeout_secs: u64,

    // JWT
    pub jwt_secret: String,
    pub jwt_expiry_hours: i64,

    // Stellar
    pub horizon_url: String,
    pub stellar_network_passphrase: String,

    // Soroban / keeper — the keeper is a service account (not a user's
    // wallet) that pays network fees and invokes public contract functions
    // (execute_subscription, release_escrow, refund_escrow) once conditions
    // are met on-chain. It never touches user funds or user keys.
    pub soroban_rpc_url: String,
    pub keeper_secret_key: Option<String>,
    pub subscription_contract_id: Option<String>,
    pub escrow_contract_id: Option<String>,
    /// How often the background keeper loop scans for due work.
    pub keeper_poll_interval_secs: u64,
    /// Disable the background keeper loop (e.g. in tests, or when execution
    /// is instead triggered via the manual /api/keeper/run endpoint).
    pub keeper_enabled: bool,

    // Batch payment reconciliation (#30) — recovers `transactions` rows
    // stuck 'pending'/'submitted_unconfirmed' after a crash or an
    // ambiguous submission timeout by checking Horizon directly.
    /// How often the background reconciliation loop sweeps for stuck rows.
    pub reconciliation_poll_interval_secs: u64,
    /// How long a row must have been stuck before a sweep considers it
    /// worth checking against Horizon.
    pub reconciliation_stale_after_secs: i64,

    // Rates
    pub rate_cache_ttl_secs: u64,

    // CORS
    pub allowed_origins: Vec<String>,

    // App
    pub app_env: AppEnv,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppEnv {
    Development,
    Production,
}

impl Config {
    /// Load configuration from environment variables (dotenv must have been initialised first).
    pub fn from_env() -> Result<Self> {
        let port = env::var("PORT")
            .unwrap_or_else(|_| "8080".into())
            .parse::<u16>()
            .context("PORT must be a valid u16")?;

        let host = env::var("HOST").unwrap_or_else(|_| "0.0.0.0".into());

        let database_url = env::var("DATABASE_URL").context("DATABASE_URL must be set")?;

        let database_max_connections = env::var("DATABASE_MAX_CONNECTIONS")
            .unwrap_or_else(|_| "20".into())
            .parse::<u32>()
            .context("DATABASE_MAX_CONNECTIONS must be a valid u32")?;

        let database_min_connections = env::var("DATABASE_MIN_CONNECTIONS")
            .unwrap_or_else(|_| "2".into())
            .parse::<u32>()
            .context("DATABASE_MIN_CONNECTIONS must be a valid u32")?;

        let database_connect_timeout_secs = env::var("DATABASE_CONNECT_TIMEOUT_SECS")
            .unwrap_or_else(|_| "30".into())
            .parse::<u64>()
            .context("DATABASE_CONNECT_TIMEOUT_SECS must be a valid u64")?;

        let jwt_secret = env::var("JWT_SECRET").context("JWT_SECRET must be set")?;
        if jwt_secret.len() < 32 {
            anyhow::bail!("JWT_SECRET must be at least 32 characters");
        }

        let jwt_expiry_hours = env::var("JWT_EXPIRY_HOURS")
            .unwrap_or_else(|_| "24".into())
            .parse::<i64>()
            .context("JWT_EXPIRY_HOURS must be a valid i64")?;

        let horizon_url = env::var("HORIZON_URL")
            .unwrap_or_else(|_| "https://horizon-testnet.stellar.org".into());

        let stellar_network_passphrase = env::var("STELLAR_NETWORK_PASSPHRASE")
            .unwrap_or_else(|_| "Test SDF Network ; September 2015".into());

        let soroban_rpc_url = env::var("SOROBAN_RPC_URL")
            .unwrap_or_else(|_| "https://soroban-testnet.stellar.org".into());

        let keeper_secret_key = env::var("KEEPER_SECRET_KEY").ok();
        let subscription_contract_id = env::var("SUBSCRIPTION_CONTRACT_ID").ok();
        let escrow_contract_id = env::var("ESCROW_CONTRACT_ID").ok();

        let keeper_poll_interval_secs = env::var("KEEPER_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "60".into())
            .parse::<u64>()
            .context("KEEPER_POLL_INTERVAL_SECS must be a valid u64")?;

        let keeper_enabled = env::var("KEEPER_ENABLED")
            .map(|v| v != "false" && v != "0")
            .unwrap_or(true);

        let reconciliation_poll_interval_secs = env::var("RECONCILIATION_POLL_INTERVAL_SECS")
            .unwrap_or_else(|_| "30".into())
            .parse::<u64>()
            .context("RECONCILIATION_POLL_INTERVAL_SECS must be a valid u64")?;

        let reconciliation_stale_after_secs = env::var("RECONCILIATION_STALE_AFTER_SECS")
            .unwrap_or_else(|_| "60".into())
            .parse::<i64>()
            .context("RECONCILIATION_STALE_AFTER_SECS must be a valid i64")?;

        let rate_cache_ttl_secs = env::var("RATE_CACHE_TTL_SECS")
            .unwrap_or_else(|_| "60".into())
            .parse::<u64>()
            .context("RATE_CACHE_TTL_SECS must be a valid u64")?;

        let allowed_origins_raw =
            env::var("ALLOWED_ORIGINS").unwrap_or_else(|_| "*".into());
        let allowed_origins = allowed_origins_raw
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let app_env = match env::var("APP_ENV")
            .unwrap_or_else(|_| "development".into())
            .to_lowercase()
            .as_str()
        {
            "production" | "prod" => AppEnv::Production,
            _ => AppEnv::Development,
        };

        if app_env == AppEnv::Production
            && (allowed_origins.is_empty() || allowed_origins.iter().any(|o| o == "*"))
        {
            bail!("ALLOWED_ORIGINS must be set to an explicit, non-wildcard list of origins in production");
        }

        Ok(Self {
            port,
            host,
            database_url,
            database_max_connections,
            database_min_connections,
            database_connect_timeout_secs,
            jwt_secret,
            jwt_expiry_hours,
            horizon_url,
            stellar_network_passphrase,
            soroban_rpc_url,
            keeper_secret_key,
            subscription_contract_id,
            escrow_contract_id,
            keeper_poll_interval_secs,
            keeper_enabled,
            reconciliation_poll_interval_secs,
            reconciliation_stale_after_secs,
            rate_cache_ttl_secs,
            allowed_origins,
            app_env,
        })
    }

    pub fn is_production(&self) -> bool {
        self.app_env == AppEnv::Production
    }
}
