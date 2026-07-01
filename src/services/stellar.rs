use crate::{
    error::{AppError, AppResult},
    models::payment::{Asset, PathHop},
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Thin async wrapper around the Stellar Horizon REST API.
#[derive(Debug, Clone)]
pub struct StellarService {
    client: Client,
    horizon_url: String,
}

// ─── Horizon response types ───────────────────────────────────────────────────

#[derive(Debug, Deserialize, Serialize)]
pub struct HorizonAccount {
    pub id: String,
    pub account_id: String,
    pub sequence: String,
    pub subentry_count: u32,
    pub balances: Vec<HorizonBalance>,
    pub thresholds: HorizonThresholds,
    pub flags: HorizonFlags,
    pub last_modified_ledger: u64,
    pub last_modified_time: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HorizonBalance {
    pub balance: String,
    pub asset_type: String,
    #[serde(default)]
    pub asset_code: String,
    #[serde(default)]
    pub asset_issuer: String,
    pub limit: Option<String>,
    pub buying_liabilities: Option<String>,
    pub selling_liabilities: Option<String>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HorizonThresholds {
    pub low_threshold: u32,
    pub med_threshold: u32,
    pub high_threshold: u32,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HorizonFlags {
    pub auth_required: bool,
    pub auth_revocable: bool,
    pub auth_immutable: bool,
}

#[derive(Debug, Deserialize)]
struct PathPaymentResponse {
    #[serde(rename = "_embedded")]
    embedded: PathPaymentEmbedded,
}

#[derive(Debug, Deserialize)]
struct PathPaymentEmbedded {
    records: Vec<PathRecord>,
}

#[derive(Debug, Deserialize)]
pub struct PathRecord {
    pub source_asset_type: String,
    #[serde(default)]
    pub source_asset_code: String,
    #[serde(default)]
    pub source_asset_issuer: String,
    pub source_amount: String,
    pub destination_asset_type: String,
    #[serde(default)]
    pub destination_asset_code: String,
    #[serde(default)]
    pub destination_asset_issuer: String,
    pub destination_amount: String,
    pub path: Vec<HorizonPathHop>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct HorizonPathHop {
    pub asset_type: String,
    #[serde(default)]
    pub asset_code: String,
    #[serde(default)]
    pub asset_issuer: String,
}

#[derive(Debug, Deserialize)]
pub struct SubmitTransactionResponse {
    pub hash: String,
    pub ledger: Option<u64>,
    pub fee_charged: Option<String>,
    pub successful: bool,
}

#[derive(Debug, Deserialize)]
struct HorizonErrorResponse {
    title: String,
    detail: Option<String>,
    extras: Option<Value>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct HorizonTransaction {
    pub id: String,
    pub hash: String,
    pub ledger: u64,
    pub created_at: String,
    pub source_account: String,
    pub fee_charged: String,
    pub successful: bool,
}

#[derive(Debug, Deserialize)]
struct FeeStatsResponse {
    last_ledger_base_fee: String,
    fee_charged: FeeCharged,
}

#[derive(Debug, Deserialize)]
struct FeeCharged {
    p50: String,
    p90: String,
}

impl StellarService {
    pub fn new(horizon_url: impl Into<String>) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("Failed to build reqwest client");

        Self {
            client,
            horizon_url: horizon_url.into().trim_end_matches('/').to_string(),
        }
    }

    // ── Account ──────────────────────────────────────────────────────────────

    /// Fetch a Stellar account from Horizon.
    pub async fn get_account(&self, address: &str) -> AppResult<HorizonAccount> {
        let url = format!("{}/accounts/{}", self.horizon_url, address);
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::NotFound(format!(
                "Stellar account '{address}'"
            )));
        }

        if !resp.status().is_success() {
            return Err(self.parse_horizon_error(resp).await);
        }

        resp.json::<HorizonAccount>()
            .await
            .map_err(AppError::HttpClient)
    }

    /// Return the balance list for a Stellar account.
    pub async fn get_balances(&self, address: &str) -> AppResult<Vec<HorizonBalance>> {
        let account = self.get_account(address).await?;
        Ok(account.balances)
    }

    // ── Path payments ────────────────────────────────────────────────────────

    /// Call Horizon `/paths/strict-send` to find all available payment paths.
    pub async fn get_path_payment_paths(
        &self,
        from_asset: &Asset,
        to_asset: &Asset,
        send_amount: &str,
        destination: &str,
    ) -> AppResult<Vec<PathRecord>> {
        let url = format!("{}/paths/strict-send", self.horizon_url);

        // Build query string manually to handle native vs issued assets.
        let mut params: Vec<(&str, String)> = vec![
            ("source_amount", send_amount.to_string()),
            ("destination_account", destination.to_string()),
        ];

        if from_asset.is_native() {
            params.push(("source_asset_type", "native".to_string()));
        } else {
            params.push(("source_asset_type", "credit_alphanum4".to_string()));
            params.push(("source_asset_code", from_asset.code.clone()));
            if let Some(issuer) = &from_asset.issuer {
                params.push(("source_asset_issuer", issuer.clone()));
            }
        }

        if to_asset.is_native() {
            params.push(("destination_asset_type", "native".to_string()));
        } else {
            params.push(("destination_asset_type", "credit_alphanum4".to_string()));
            params.push(("destination_asset_code", to_asset.code.clone()));
            if let Some(issuer) = &to_asset.issuer {
                params.push(("destination_asset_issuer", issuer.clone()));
            }
        }

        let resp = self
            .client
            .get(&url)
            .query(&params)
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        if !resp.status().is_success() {
            return Err(self.parse_horizon_error(resp).await);
        }

        let path_resp = resp
            .json::<PathPaymentResponse>()
            .await
            .map_err(AppError::HttpClient)?;

        Ok(path_resp.embedded.records)
    }

    // ── Transaction submission ────────────────────────────────────────────────

    /// Submit a signed TransactionEnvelope XDR to Horizon.
    pub async fn submit_transaction(
        &self,
        signed_xdr: &str,
    ) -> AppResult<SubmitTransactionResponse> {
        let url = format!("{}/transactions", self.horizon_url);

        let resp = self
            .client
            .post(&url)
            .form(&[("tx", signed_xdr)])
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        if !resp.status().is_success() {
            return Err(self.parse_horizon_error(resp).await);
        }

        resp.json::<SubmitTransactionResponse>()
            .await
            .map_err(AppError::HttpClient)
    }

    // ── Fee estimation ────────────────────────────────────────────────────────

    /// Estimate the current network fee in XLM (using p90 of recent fee stats).
    pub async fn estimate_fee(&self) -> AppResult<String> {
        let url = format!("{}/fee_stats", self.horizon_url);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        if !resp.status().is_success() {
            // Fall back to base fee if fee_stats is unavailable.
            return Ok("0.00001".to_string());
        }

        let stats: FeeStatsResponse = resp.json().await.map_err(AppError::HttpClient)?;

        // Convert stroops to XLM (1 XLM = 10_000_000 stroops).
        let fee_stroops: f64 = stats.fee_charged.p90.parse().unwrap_or(100.0);
        let fee_xlm = fee_stroops / 10_000_000.0;
        Ok(format!("{:.7}", fee_xlm))
    }

    // ── Transaction fetching ──────────────────────────────────────────────────

    /// Fetch a single transaction by hash from Horizon.
    pub async fn fetch_transaction(&self, hash: &str) -> AppResult<HorizonTransaction> {
        let url = format!("{}/transactions/{}", self.horizon_url, hash);

        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(AppError::HttpClient)?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(AppError::NotFound(format!(
                "Stellar transaction '{hash}'"
            )));
        }

        if !resp.status().is_success() {
            return Err(self.parse_horizon_error(resp).await);
        }

        resp.json::<HorizonTransaction>()
            .await
            .map_err(AppError::HttpClient)
    }

    // ── Helpers ───────────────────────────────────────────────────────────────

    async fn parse_horizon_error(&self, resp: reqwest::Response) -> AppError {
        let status = resp.status();
        match resp.json::<HorizonErrorResponse>().await {
            Ok(e) => {
                let detail = e.detail.unwrap_or_else(|| e.title.clone());
                // If extras contain result_codes, surface them.
                let msg = if let Some(extras) = e.extras {
                    if let Some(codes) = extras.get("result_codes") {
                        format!("{detail} — result_codes: {codes}")
                    } else {
                        detail
                    }
                } else {
                    detail
                };
                AppError::HorizonError(msg)
            }
            Err(_) => AppError::HorizonError(format!("HTTP {status}")),
        }
    }

    /// Convert a `HorizonPathHop` to our model `PathHop`.
    pub fn convert_path_hop(hop: &HorizonPathHop) -> PathHop {
        PathHop {
            asset_code: if hop.asset_type == "native" {
                "XLM".to_string()
            } else {
                hop.asset_code.clone()
            },
            asset_issuer: if hop.asset_issuer.is_empty() {
                None
            } else {
                Some(hop.asset_issuer.clone())
            },
            asset_type: hop.asset_type.clone(),
        }
    }
}
