use crate::{
    config::Config,
    db::is_unique_violation,
    error::{AppError, AppResult},
    models::{
        escrow::{CreateEscrowRequest, Escrow, EscrowActionRequest, EscrowActor, EscrowRow, EscrowStatus},
        transaction::{CreateTransaction, TransactionStatus},
    },
    services::{
        soroban::{account_address_scval as soroban_account_scval, ContractCallArgs, SorobanService},
        stellar::StellarService,
        transaction::TransactionService,
    },
};
use chrono::Utc;
use sqlx::PgPool;
use stellar_xdr::curr::ScVal;
use uuid::Uuid;

/// Which contract function a validated escrow action maps to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EscrowAction {
    Release,
    Refund,
}

impl EscrowAction {
    fn function_name(self) -> &'static str {
        match self {
            EscrowAction::Release => "release_escrow",
            EscrowAction::Refund => "refund_escrow",
        }
    }
}

pub struct EscrowService {
    pool: PgPool,
}


impl EscrowService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ─── Create ───────────────────────────────────────────────────────────────

    pub async fn create(&self, depositor_id: Uuid, req: &CreateEscrowRequest) -> AppResult<Escrow> {
        let amount: f64 = req
            .amount
            .parse()
            .map_err(|_| AppError::Validation("amount must be a valid decimal string".into()))?;
        if amount <= 0.0 {
            return Err(AppError::Validation("amount must be positive".into()));
        }
        if req.depositor_account.trim().is_empty() || req.beneficiary_account.trim().is_empty() {
            return Err(AppError::Validation(
                "depositor_account and beneficiary_account are required".into(),
            ));
        }
        if req.unlock_time <= Utc::now() {
            return Err(AppError::Validation(
                "unlock_time must be in the future".into(),
            ));
        }

        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, EscrowRow>(
            r#"
            INSERT INTO escrows (
                id, depositor_id, depositor_account, beneficiary_account, asset_code,
                asset_issuer, amount, unlock_time, arbiter_account, status,
                onchain_escrow_id, funding_tx_hash
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'active', $10, $11)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(depositor_id)
        .bind(&req.depositor_account)
        .bind(&req.beneficiary_account)
        .bind(&req.asset_code)
        .bind(&req.asset_issuer)
        .bind(&req.amount)
        .bind(req.unlock_time)
        .bind(&req.arbiter_account)
        .bind(&req.onchain_escrow_id)
        .bind(&req.funding_tx_hash)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| {
            if is_unique_violation(&e) {
                AppError::Conflict("An escrow with this onchain_escrow_id already exists".into())
            } else {
                e.into()
            }
        })?;

        Ok(row.into())
    }

    // ─── Read ─────────────────────────────────────────────────────────────────

    pub async fn get_by_id(&self, id: Uuid) -> AppResult<Option<Escrow>> {
        let row = sqlx::query_as::<_, EscrowRow>("SELECT * FROM escrows WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        Ok(row.map(Escrow::from))
    }

    pub async fn list_for_user(&self, depositor_id: Uuid) -> AppResult<Vec<Escrow>> {
        let rows = sqlx::query_as::<_, EscrowRow>(
            "SELECT * FROM escrows WHERE depositor_id = $1 ORDER BY created_at DESC",
        )
        .bind(depositor_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(Escrow::from).collect())
    }

    // ─── Validation ───────────────────────────────────────────────────────────

    /// Validate that `req.actor`/`req.account` is actually allowed to take
    /// `action` on `escrow` right now, mirroring the contract's own rules:
    ///
    /// - `release`: the beneficiary may release once `unlock_time` has
    ///   passed; the arbiter (if any) may release at any time.
    /// - `refund`: the depositor may reclaim funds once `unlock_time` has
    ///   passed without the beneficiary claiming; the arbiter (if any) may
    ///   refund at any time.
    ///
    /// Returns the validated `Escrow` on success.
    pub fn authorize_action(
        &self,
        escrow: &Escrow,
        action: EscrowAction,
        req: &EscrowActionRequest,
    ) -> AppResult<()> {
        if escrow.status != EscrowStatus::Active {
            return Err(AppError::Conflict(format!(
                "Escrow is already {:?}",
                escrow.status
            )));
        }

        let claimed_account_matches = match req.actor {
            EscrowActor::Depositor => escrow.depositor_account == req.account,
            EscrowActor::Beneficiary => escrow.beneficiary_account == req.account,
            EscrowActor::Arbiter => escrow.arbiter_account.as_deref() == Some(req.account.as_str()),
        };
        if !claimed_account_matches {
            return Err(AppError::Forbidden);
        }

        let role_permitted = match (action, req.actor) {
            (EscrowAction::Release, EscrowActor::Beneficiary) => true,
            (EscrowAction::Release, EscrowActor::Arbiter) => true,
            (EscrowAction::Refund, EscrowActor::Depositor) => true,
            (EscrowAction::Refund, EscrowActor::Arbiter) => true,
            _ => false,
        };
        if !role_permitted {
            return Err(AppError::Forbidden);
        }

        // The arbiter can act at any time; everyone else must wait for
        // unlock_time to pass.
        if req.actor != EscrowActor::Arbiter && Utc::now() < escrow.unlock_time {
            return Err(AppError::BadRequest(
                "Escrow cannot be actioned before unlock_time (only the arbiter may act early)"
                    .into(),
            ));
        }

        Ok(())
    }

    /// Validate the action and build an *unsigned* `release_escrow`/
    /// `refund_escrow` invocation sourced from the caller's own account,
    /// ready for the caller to sign client-side (Freighter) and post back
    /// to [`Self::submit_signed_action`].
    ///
    /// This is client-signed rather than keeper-executed because the
    /// contract's `release_escrow`/`refund_escrow` call
    /// `caller.require_auth()` — the on-chain call only succeeds if the
    /// *actual* beneficiary/depositor/arbiter signs it. A keeper (a service
    /// account with its own key) cannot sign on behalf of any of those
    /// parties, so unlike subscription execution (which uses a pre-granted
    /// token allowance) there is no sound way to automate this action for
    /// a party the keeper isn't itself.
    pub async fn build_action_tx(
        &self,
        config: &Config,
        stellar: &StellarService,
        soroban: &SorobanService,
        id: Uuid,
        action: EscrowAction,
        req: &EscrowActionRequest,
    ) -> AppResult<String> {
        let escrow = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Escrow".into()))?;

        self.authorize_action(&escrow, action, req)?;

        let contract_id = config.escrow_contract_id.clone().ok_or_else(|| {
            AppError::KeeperUnavailable("ESCROW_CONTRACT_ID must be set".into())
        })?;

        let call = ContractCallArgs {
            contract_id,
            function_name: action.function_name().to_string(),
            args: vec![
                escrow_id_scval(&escrow),
                soroban_account_scval(&req.account)?,
            ],
        };

        soroban
            .build_unsigned_invoke_tx(stellar, &req.account, &call)
            .await
    }

    /// Relay a client-signed `release_escrow`/`refund_escrow` envelope
    /// (produced from [`Self::build_action_tx`]) and update the escrow's
    /// recorded state once it lands.
    pub async fn submit_signed_action(
        &self,
        soroban: &SorobanService,
        tx_service: &TransactionService,
        id: Uuid,
        action: EscrowAction,
        signed_xdr: &str,
    ) -> AppResult<Escrow> {
        let escrow = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Escrow".into()))?;

        let (from_account, to_account) = match action {
            EscrowAction::Release => (escrow.depositor_account.clone(), escrow.beneficiary_account.clone()),
            EscrowAction::Refund => (escrow.beneficiary_account.clone(), escrow.depositor_account.clone()),
        };

        let pending_tx = tx_service
            .create(CreateTransaction {
                user_id: escrow.depositor_id,
                from_asset: format_asset(&escrow.asset_code, &escrow.asset_issuer),
                to_asset: format_asset(&escrow.asset_code, &escrow.asset_issuer),
                send_amount: escrow.amount.clone(),
                receive_amount: None,
                source_account: from_account,
                destination_account: to_account,
                fee_xlm: None,
                path: None,
            })
            .await?;

        match soroban.send_transaction(signed_xdr).await {
            Ok(result) => {
                tx_service
                    .update_status(
                        pending_tx.id,
                        TransactionStatus::Completed,
                        Some(result.hash.clone()),
                        None,
                    )
                    .await?;

                // Both release and refund settle into the same
                // `release_tx_hash` column — it just records "the tx hash
                // of whichever terminal on-chain call resolved this escrow".
                let new_status = match action {
                    EscrowAction::Release => EscrowStatus::Released,
                    EscrowAction::Refund => EscrowStatus::Refunded,
                };

                let row = sqlx::query_as::<_, EscrowRow>(
                    r#"
                    UPDATE escrows
                    SET status = $2, release_tx_hash = $3, updated_at = NOW()
                    WHERE id = $1
                    RETURNING *
                    "#,
                )
                .bind(id)
                .bind(new_status)
                .bind(&result.hash)
                .fetch_one(&self.pool)
                .await?;

                Ok(row.into())
            }
            Err(e) => {
                tx_service
                    .update_status(pending_tx.id, TransactionStatus::Failed, None, Some(e.to_string()))
                    .await?;
                Err(e)
            }
        }
    }
}

fn escrow_id_scval(escrow: &Escrow) -> ScVal {
    let raw = escrow
        .onchain_escrow_id
        .clone()
        .unwrap_or_else(|| escrow.id.to_string());

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
    use chrono::Duration;

    fn sample_escrow(unlock_time: chrono::DateTime<Utc>, arbiter: Option<String>) -> Escrow {
        Escrow {
            id: Uuid::new_v4(),
            depositor_id: Uuid::new_v4(),
            depositor_account: "GDEPOSITOR".into(),
            beneficiary_account: "GBENEFICIARY".into(),
            asset_code: "XLM".into(),
            asset_issuer: None,
            amount: "100".into(),
            unlock_time,
            arbiter_account: arbiter,
            status: EscrowStatus::Active,
            onchain_escrow_id: None,
            funding_tx_hash: None,
            release_tx_hash: None,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn svc() -> EscrowService {
        EscrowService {
            pool: sqlx::PgPool::connect_lazy("postgres://user:pass@localhost/db").unwrap(),
        }
    }

    #[tokio::test]
    async fn beneficiary_cannot_release_before_unlock_time() {
        let escrow = sample_escrow(Utc::now() + Duration::hours(1), None);
        let req = EscrowActionRequest {
            actor: EscrowActor::Beneficiary,
            account: "GBENEFICIARY".into(),
        };
        let err = svc().authorize_action(&escrow, EscrowAction::Release, &req).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn beneficiary_can_release_after_unlock_time() {
        let escrow = sample_escrow(Utc::now() - Duration::hours(1), None);
        let req = EscrowActionRequest {
            actor: EscrowActor::Beneficiary,
            account: "GBENEFICIARY".into(),
        };
        assert!(svc().authorize_action(&escrow, EscrowAction::Release, &req).is_ok());
    }

    #[tokio::test]
    async fn arbiter_can_release_before_unlock_time() {
        let escrow = sample_escrow(Utc::now() + Duration::hours(1), Some("GARBITER".into()));
        let req = EscrowActionRequest {
            actor: EscrowActor::Arbiter,
            account: "GARBITER".into(),
        };
        assert!(svc().authorize_action(&escrow, EscrowAction::Release, &req).is_ok());
    }

    #[tokio::test]
    async fn depositor_cannot_release() {
        let escrow = sample_escrow(Utc::now() - Duration::hours(1), None);
        let req = EscrowActionRequest {
            actor: EscrowActor::Depositor,
            account: "GDEPOSITOR".into(),
        };
        let err = svc().authorize_action(&escrow, EscrowAction::Release, &req).unwrap_err();
        assert!(matches!(err, AppError::Forbidden));
    }

    #[tokio::test]
    async fn mismatched_account_for_claimed_role_is_forbidden() {
        let escrow = sample_escrow(Utc::now() - Duration::hours(1), None);
        let req = EscrowActionRequest {
            actor: EscrowActor::Beneficiary,
            account: "GSOMEONE_ELSE".into(),
        };
        let err = svc().authorize_action(&escrow, EscrowAction::Release, &req).unwrap_err();
        assert!(matches!(err, AppError::Forbidden));
    }
}

/// End-to-end coverage requiring a real Postgres (`DATABASE_URL`) — see
/// `reconciliation::db_tests` for why these live here as `#[ignore]`d
/// `#[tokio::test]`s rather than in `tests/`. Run with
/// `cargo test -- --ignored` against a real database. Covers #56's
/// acceptance criteria for `escrows.onchain_escrow_id`.
#[cfg(test)]
mod db_tests {
    use super::*;
    use chrono::Duration;
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

    async fn seed_user(pool: &PgPool) -> Uuid {
        let user_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO users (id, email, password_hash, full_name) VALUES ($1, $2, 'x', 'Test User')",
        )
        .bind(user_id)
        .bind(format!("{user_id}@example.test"))
        .execute(pool)
        .await
        .expect("seed user insert should succeed");
        user_id
    }

    async fn cleanup(pool: &PgPool, user_id: Uuid) {
        let _ = sqlx::query("DELETE FROM escrows WHERE depositor_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }

    fn sample_request(onchain_escrow_id: Option<String>) -> CreateEscrowRequest {
        CreateEscrowRequest {
            depositor_account: "GDEPOSITOR".into(),
            beneficiary_account: "GBENEFICIARY".into(),
            asset_code: "XLM".into(),
            asset_issuer: None,
            amount: "10".into(),
            unlock_time: Utc::now() + Duration::hours(1),
            arbiter_account: None,
            onchain_escrow_id,
            funding_tx_hash: None,
        }
    }

    #[tokio::test]
    #[ignore]
    async fn create_rejects_a_duplicate_onchain_escrow_id_with_conflict() {
        let pool = test_pool().await;
        let user_id = seed_user(&pool).await;
        let service = EscrowService::new(pool.clone());
        let onchain_id = format!("onchain-{}", Uuid::new_v4());

        let first = service
            .create(user_id, &sample_request(Some(onchain_id.clone())))
            .await
            .expect("first create with a fresh onchain_escrow_id should succeed");
        assert_eq!(
            first.onchain_escrow_id.as_deref(),
            Some(onchain_id.as_str())
        );

        let second = service
            .create(user_id, &sample_request(Some(onchain_id)))
            .await;
        assert!(
            matches!(second, Err(AppError::Conflict(_))),
            "expected Conflict, got {second:?}"
        );

        cleanup(&pool, user_id).await;
    }

    #[tokio::test]
    #[ignore]
    async fn create_allows_multiple_rows_with_no_onchain_escrow_id() {
        let pool = test_pool().await;
        let user_id = seed_user(&pool).await;
        let service = EscrowService::new(pool.clone());

        let first = service.create(user_id, &sample_request(None)).await;
        let second = service.create(user_id, &sample_request(None)).await;

        assert!(first.is_ok(), "expected first NULL-id create to succeed");
        assert!(second.is_ok(), "expected second NULL-id create to succeed");

        cleanup(&pool, user_id).await;
    }
}
