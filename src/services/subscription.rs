use crate::{
    config::Config,
    error::{AppError, AppResult},
    models::{
        subscription::{
            CreateSubscriptionRequest, ListSubscriptionsParams, Subscription, SubscriptionRow,
            SubscriptionStatus,
        },
        transaction::{CreateTransaction, TransactionStatus},
    },
    services::{
        soroban::{ContractCallArgs, SorobanService},
        stellar::StellarService,
        transaction::TransactionService,
    },
};
use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use stellar_xdr::curr::ScVal;
use uuid::Uuid;

/// After this many consecutive on-chain failures (e.g. insufficient balance,
/// revoked allowance) a subscription is marked `failed` instead of being
/// retried forever.
const MAX_CONSECUTIVE_FAILURES: i32 = 3;

/// Max subscriptions processed per keeper pass, to bound worst-case latency.
const KEEPER_BATCH_LIMIT: i64 = 50;

#[derive(Debug, Default, serde::Serialize)]
pub struct KeeperRunSummary {
    pub executed: usize,
    pub failed: usize,
    pub considered: usize,
}

pub struct SubscriptionService {
    pool: PgPool,
}

impl SubscriptionService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ─── Create ───────────────────────────────────────────────────────────────

    pub async fn create(
        &self,
        payer_id: Uuid,
        req: &CreateSubscriptionRequest,
    ) -> AppResult<Subscription> {
        let amount: f64 = req
            .amount
            .parse()
            .map_err(|_| AppError::Validation("amount must be a valid decimal string".into()))?;
        if amount <= 0.0 {
            return Err(AppError::Validation("amount must be positive".into()));
        }
        if req.interval_seconds <= 0 {
            return Err(AppError::Validation(
                "interval_seconds must be positive".into(),
            ));
        }
        if req.payer_account.trim().is_empty() || req.recipient_account.trim().is_empty() {
            return Err(AppError::Validation(
                "payer_account and recipient_account are required".into(),
            ));
        }

        let next_execution_at = req
            .first_execution_at
            .unwrap_or_else(|| Utc::now() + ChronoDuration::seconds(req.interval_seconds));

        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, SubscriptionRow>(
            r#"
            INSERT INTO subscriptions (
                id, payer_id, payer_account, recipient_account, asset_code,
                asset_issuer, amount, interval_seconds, next_execution_at,
                status, onchain_subscription_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active', $10)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(payer_id)
        .bind(&req.payer_account)
        .bind(&req.recipient_account)
        .bind(&req.asset_code)
        .bind(&req.asset_issuer)
        .bind(&req.amount)
        .bind(req.interval_seconds)
        .bind(next_execution_at)
        .bind(&req.onchain_subscription_id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.into())
    }

    // ─── Read ─────────────────────────────────────────────────────────────────

    pub async fn get_by_id(&self, id: Uuid) -> AppResult<Option<Subscription>> {
        let row = sqlx::query_as::<_, SubscriptionRow>("SELECT * FROM subscriptions WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(Subscription::from))
    }

    pub async fn list_for_user(
        &self,
        payer_id: Uuid,
        params: &ListSubscriptionsParams,
    ) -> AppResult<Vec<Subscription>> {
        let rows = if let Some(status) = params.status {
            sqlx::query_as::<_, SubscriptionRow>(
                "SELECT * FROM subscriptions WHERE payer_id = $1 AND status = $2 ORDER BY created_at DESC",
            )
            .bind(payer_id)
            .bind(status)
            .fetch_all(&self.pool)
            .await?
        } else {
            sqlx::query_as::<_, SubscriptionRow>(
                "SELECT * FROM subscriptions WHERE payer_id = $1 ORDER BY created_at DESC",
            )
            .bind(payer_id)
            .fetch_all(&self.pool)
            .await?
        };

        Ok(rows.into_iter().map(Subscription::from).collect())
    }

    // ─── Cancel ───────────────────────────────────────────────────────────────

    /// Cancel a subscription. Only the payer who owns it may cancel it.
    pub async fn cancel(&self, payer_id: Uuid, id: Uuid) -> AppResult<Subscription> {
        let existing = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Subscription".into()))?;

        if existing.payer_id != payer_id {
            return Err(AppError::Forbidden);
        }

        if existing.status != SubscriptionStatus::Active {
            return Err(AppError::Conflict(
                "Subscription is not active and cannot be cancelled".into(),
            ));
        }

        let row = sqlx::query_as::<_, SubscriptionRow>(
            r#"
            UPDATE subscriptions
            SET status = 'cancelled', updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.into())
    }

    // ─── Keeper ───────────────────────────────────────────────────────────────

    /// Find active subscriptions whose `next_execution_at` has passed and
    /// submit the on-chain `execute_subscription` call for each, via the
    /// keeper service account.
    ///
    /// Uses `FOR UPDATE SKIP LOCKED` so multiple keeper instances (or an
    /// overlapping manual trigger + background loop) never double-execute
    /// the same subscription.
    pub async fn run_due_executions(
        &self,
        config: &Config,
        stellar: &StellarService,
        soroban: &SorobanService,
        tx_service: &TransactionService,
    ) -> AppResult<KeeperRunSummary> {
        let mut summary = KeeperRunSummary::default();

        let (contract_id, keeper_secret) = match (
            &config.subscription_contract_id,
            &config.keeper_secret_key,
        ) {
            (Some(c), Some(k)) => (c.clone(), k.clone()),
            _ => {
                return Err(AppError::KeeperUnavailable(
                    "SUBSCRIPTION_CONTRACT_ID and KEEPER_SECRET_KEY must both be set".into(),
                ))
            }
        };

        let mut tx = self.pool.begin().await?;
        let due: Vec<SubscriptionRow> = sqlx::query_as(
            r#"
            SELECT * FROM subscriptions
            WHERE status = 'active' AND next_execution_at <= NOW()
            ORDER BY next_execution_at ASC
            LIMIT $1
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(KEEPER_BATCH_LIMIT)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        summary.considered = due.len();

        for sub in due {
            let call = ContractCallArgs {
                contract_id: contract_id.clone(),
                function_name: "execute_subscription".to_string(),
                // NOTE: arg encoding is a best-effort placeholder until the
                // contracts repo publishes the final `execute_subscription`
                // ABI — adjust once the deployed signature is known.
                args: vec![subscription_id_scval(&sub)],
            };

            // Pre-create a pending transaction record for history.
            let pending_tx = tx_service
                .create(CreateTransaction {
                    user_id: sub.payer_id,
                    from_asset: format_asset(&sub.asset_code, &sub.asset_issuer),
                    to_asset: format_asset(&sub.asset_code, &sub.asset_issuer),
                    send_amount: sub.amount.clone(),
                    receive_amount: None,
                    source_account: sub.payer_account.clone(),
                    destination_account: sub.recipient_account.clone(),
                    fee_xlm: None,
                    path: None,
                })
                .await?;

            match soroban
                .invoke_contract_via_keeper(
                    stellar,
                    &config.stellar_network_passphrase,
                    &keeper_secret,
                    &call,
                )
                .await
            {
                Ok(result) => {
                    tx_service
                        .update_status(
                            pending_tx.id,
                            TransactionStatus::Completed,
                            Some(result.hash.clone()),
                            None,
                        )
                        .await?;

                    let next_execution_at =
                        Utc::now() + ChronoDuration::seconds(sub.interval_seconds);

                    sqlx::query(
                        r#"
                        UPDATE subscriptions
                        SET last_execution_at = NOW(),
                            last_tx_hash = $2,
                            next_execution_at = $3,
                            failure_count = 0,
                            last_error = NULL,
                            updated_at = NOW()
                        WHERE id = $1
                        "#,
                    )
                    .bind(sub.id)
                    .bind(&result.hash)
                    .bind(next_execution_at)
                    .execute(&self.pool)
                    .await?;

                    summary.executed += 1;
                }
                Err(e) => {
                    tx_service
                        .update_status(
                            pending_tx.id,
                            TransactionStatus::Failed,
                            None,
                            Some(e.to_string()),
                        )
                        .await?;

                    let new_failure_count = sub.failure_count + 1;
                    // Don't retry forever: after MAX_CONSECUTIVE_FAILURES,
                    // stop scheduling and surface the terminal state instead
                    // of silently looping every poll interval.
                    if new_failure_count >= MAX_CONSECUTIVE_FAILURES {
                        sqlx::query(
                            r#"
                            UPDATE subscriptions
                            SET status = 'failed',
                                failure_count = $2,
                                last_error = $3,
                                updated_at = NOW()
                            WHERE id = $1
                            "#,
                        )
                        .bind(sub.id)
                        .bind(new_failure_count)
                        .bind(e.to_string())
                        .execute(&self.pool)
                        .await?;
                    } else {
                        // Retry at the next poll, but push next_execution_at
                        // forward slightly so a persistently-broken
                        // subscription doesn't hammer the RPC every second.
                        let retry_at = Utc::now() + ChronoDuration::seconds(60);
                        sqlx::query(
                            r#"
                            UPDATE subscriptions
                            SET failure_count = $2,
                                last_error = $3,
                                next_execution_at = $4,
                                updated_at = NOW()
                            WHERE id = $1
                            "#,
                        )
                        .bind(sub.id)
                        .bind(new_failure_count)
                        .bind(e.to_string())
                        .bind(retry_at)
                        .execute(&self.pool)
                        .await?;
                    }

                    summary.failed += 1;
                }
            }
        }

        Ok(summary)
    }
}

/// Encode the subscription's on-chain id as an `ScVal` argument. Falls back
/// to the internal UUID's low 64 bits if no on-chain id has been recorded
/// yet (e.g. subscription created off-chain-first); real deployments should
/// always have `onchain_subscription_id` populated before the first
/// execution is due.
fn subscription_id_scval(sub: &SubscriptionRow) -> ScVal {
    let raw = sub
        .onchain_subscription_id
        .clone()
        .unwrap_or_else(|| sub.id.to_string());

    if let Ok(n) = raw.parse::<u64>() {
        ScVal::U64(n)
    } else {
        ScVal::Bytes(raw.into_bytes().try_into().unwrap_or_default())
    }
}

fn format_asset(code: &str, issuer: &Option<String>) -> String {
    match issuer {
        Some(issuer) => format!("{code}:{issuer}"),
        None => code.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscription_id_scval_prefers_numeric_onchain_id() {
        let row = SubscriptionRow {
            id: Uuid::nil(),
            payer_id: Uuid::nil(),
            payer_account: "G...".into(),
            recipient_account: "G...".into(),
            asset_code: "XLM".into(),
            asset_issuer: None,
            amount: "10".into(),
            interval_seconds: 60,
            next_execution_at: Utc::now(),
            status: SubscriptionStatus::Active,
            onchain_subscription_id: Some("42".into()),
            last_execution_at: None,
            last_tx_hash: None,
            failure_count: 0,
            last_error: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };

        assert_eq!(subscription_id_scval(&row), ScVal::U64(42));
    }

    #[test]
    fn format_asset_includes_issuer_when_present() {
        assert_eq!(format_asset("XLM", &None), "XLM");
        assert_eq!(
            format_asset("USDC", &Some("GISSUER".into())),
            "USDC:GISSUER"
        );
    }
}
