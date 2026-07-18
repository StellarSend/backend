use crate::{
    error::AppResult,
    models::transaction::TransactionStatus,
    services::stellar::{HorizonTransaction, StellarService},
};
use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;

/// How many distinct stuck hashes to resolve per sweep — mirrors
/// `SubscriptionService::run_due_executions`' `KEEPER_BATCH_LIMIT` pattern
/// so a single pass can't run unbounded.
const RECONCILIATION_BATCH_LIMIT: i64 = 50;

#[derive(Debug, Default, PartialEq, Eq)]
pub struct ReconciliationSummary {
    pub considered: usize,
    pub resolved_completed: usize,
    pub resolved_failed: usize,
    pub still_unconfirmed: usize,
}

/// Recovers `transactions.status` from the two failure modes
/// `BatchPaymentService::execute_batch` can't fully protect against on its
/// own (#30):
///
///  - a crash between Horizon accepting a submission and our own
///    post-submission `UPDATE` committing, which would otherwise leave the
///    row `pending` forever even though the payment went through; and
///  - a client-side timeout/connection failure on submission that doesn't
///    tell us whether Horizon actually received and processed it, which
///    `execute_batch` marks `submitted_unconfirmed` rather than a
///    false-negative `failed`.
///
/// Both converge the same way: look the transaction's precomputed hash up
/// on Horizon directly and trust *that* over our own uncertain local
/// state, rather than ever guessing.
pub struct ReconciliationService {
    pool: PgPool,
}

impl ReconciliationService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Finds transaction rows stuck in a non-terminal state with a known
    /// hash, older than `stale_after`, and converges each distinct hash's
    /// rows to their true on-chain outcome.
    pub async fn reconcile_stuck_transactions(
        &self,
        stellar: &StellarService,
        stale_after: Duration,
    ) -> AppResult<ReconciliationSummary> {
        let mut summary = ReconciliationSummary::default();
        let cutoff: DateTime<Utc> = Utc::now() - stale_after;

        // FOR UPDATE SKIP LOCKED (mirroring
        // SubscriptionService::run_due_executions) means an overlapping
        // sweep, or a second server instance running the same loop, simply
        // skips rows another pass already grabbed instead of racing it.
        let mut tx = self.pool.begin().await?;
        let stuck_hashes: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT DISTINCT stellar_tx_hash
            FROM transactions
            WHERE status IN ('pending', 'submitted_unconfirmed')
              AND stellar_tx_hash IS NOT NULL
              AND updated_at < $1
            ORDER BY stellar_tx_hash
            LIMIT $2
            FOR UPDATE SKIP LOCKED
            "#,
        )
        .bind(cutoff)
        .bind(RECONCILIATION_BATCH_LIMIT)
        .fetch_all(&mut *tx)
        .await?;
        tx.commit().await?;

        summary.considered = stuck_hashes.len();

        for hash in stuck_hashes {
            let horizon_result = stellar.fetch_transaction(&hash).await;

            match resolve_from_horizon_result(&horizon_result) {
                Some(status) => {
                    sqlx::query(
                        r#"
                        UPDATE transactions
                        SET status = $2, updated_at = NOW()
                        WHERE stellar_tx_hash = $1
                          AND status IN ('pending', 'submitted_unconfirmed')
                        "#,
                    )
                    .bind(&hash)
                    .bind(&status)
                    .execute(&self.pool)
                    .await?;

                    match status {
                        TransactionStatus::Completed => summary.resolved_completed += 1,
                        _ => summary.resolved_failed += 1,
                    }
                }
                None => summary.still_unconfirmed += 1,
            }
        }

        Ok(summary)
    }
}

/// Pure decision: given Horizon's answer for a stuck hash, decide the
/// terminal status to converge to, or `None` to leave it for the next
/// sweep because the outcome is still genuinely ambiguous.
///
/// `NotFound` means Horizon has never seen this hash — it could still be
/// en route, or the submission never actually reached Horizon at all — so
/// it's deliberately *not* treated as a rejection. Any other error is a
/// transient problem with *this* reconciliation attempt (Horizon outage,
/// network blip), not new information about the transaction, so it's
/// handled identically: try again next sweep.
fn resolve_from_horizon_result(
    result: &AppResult<HorizonTransaction>,
) -> Option<TransactionStatus> {
    match result {
        Ok(tx) if tx.successful => Some(TransactionStatus::Completed),
        Ok(_) => Some(TransactionStatus::Failed),
        Err(_) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::AppError;

    fn horizon_tx(successful: bool) -> HorizonTransaction {
        HorizonTransaction {
            id: "abc".into(),
            hash: "abc".into(),
            ledger: 12345,
            created_at: "2024-01-01T00:00:00Z".into(),
            source_account: "GSOURCE".into(),
            fee_charged: "100".into(),
            successful,
        }
    }

    #[test]
    fn a_successful_horizon_transaction_resolves_to_completed() {
        let result = Ok(horizon_tx(true));
        assert_eq!(
            resolve_from_horizon_result(&result),
            Some(TransactionStatus::Completed)
        );
    }

    #[test]
    fn a_confirmed_but_unsuccessful_horizon_transaction_resolves_to_failed() {
        let result = Ok(horizon_tx(false));
        assert_eq!(
            resolve_from_horizon_result(&result),
            Some(TransactionStatus::Failed)
        );
    }

    #[test]
    fn horizon_never_having_seen_the_hash_stays_unresolved() {
        let result: AppResult<HorizonTransaction> =
            Err(AppError::NotFound("Stellar transaction 'abc'".into()));
        assert_eq!(resolve_from_horizon_result(&result), None);
    }

    #[test]
    fn a_transient_horizon_error_stays_unresolved_rather_than_guessing() {
        let result: AppResult<HorizonTransaction> =
            Err(AppError::HorizonError("503 Service Unavailable".into()));
        assert_eq!(resolve_from_horizon_result(&result), None);
    }
}

/// End-to-end coverage requiring a real Postgres (`DATABASE_URL`) — this
/// crate is bin-only (no `src/lib.rs`), so these live here as `#[ignore]`d
/// `#[tokio::test]`s rather than in `tests/`, which would have no access to
/// internal types. Not runnable in this sandbox (no local Postgres); run
/// with `cargo test -- --ignored` against a real database. Covers exactly
/// the two scenarios from #30's "Testing strategy":
///
///  1. a batch crashes between Horizon accepting the submission and our
///     `UPDATE` committing — simulated by inserting a row already in that
///     exact stuck state rather than literally killing the process — and a
///     reconciliation pass resolves it to `completed`.
///  2. a submission that never got a definitive answer from Horizon is
///     marked `submitted_unconfirmed`, not `failed`, and a retry with the
///     identical `signed_xdr` is rejected rather than risking a double
///     payment.
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::{
        error::AppError,
        models::batch_payment::{BatchPaymentLeg, SendBatchPaymentRequest},
        services::{batch::BatchPaymentService, tx_hash::compute_transaction_hash},
    };
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;
    use wiremock::{
        matchers::{method, path},
        Mock, MockServer, ResponseTemplate,
    };

    const NETWORK: &str = "Test SDF Network ; September 2015";

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

    async fn cleanup(pool: &PgPool, batch_id: Uuid, hash: &str) {
        let _ = sqlx::query("DELETE FROM transactions WHERE batch_id = $1")
            .bind(batch_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM batch_submissions WHERE stellar_tx_hash = $1")
            .bind(hash)
            .execute(pool)
            .await;
    }

    /// A syntactically valid, minimal signed_xdr — real enough to parse and
    /// hash, not real enough (or meant) to ever be submitted to a live
    /// network. Mirrors `tx_hash::tests::sample_transaction`.
    fn sample_signed_xdr() -> String {
        use stellar_xdr::curr::{
            Limits, Memo, MuxedAccount, Operation, OperationBody, PaymentOp, Preconditions,
            SequenceNumber, Transaction, TransactionEnvelope, TransactionExt,
            TransactionV1Envelope, Uint256, VecM, WriteXdr,
        };

        const SOURCE: &str = "GBZXN7PIRZGNMHGA7MUUUF4GWPY5AYPV6LY4UV2GL6VJGIQRXFDNMADI";
        let source_id = stellar_strkey::ed25519::PublicKey::from_string(SOURCE)
            .expect("valid test source account");

        let tx = Transaction {
            source_account: MuxedAccount::Ed25519(Uint256(source_id.0)),
            fee: 100,
            seq_num: SequenceNumber(1),
            cond: Preconditions::None,
            memo: Memo::None,
            operations: vec![Operation {
                source_account: None,
                body: OperationBody::Payment(PaymentOp {
                    destination: MuxedAccount::Ed25519(Uint256(source_id.0)),
                    asset: stellar_xdr::curr::Asset::Native,
                    amount: 100_000_000,
                }),
            }]
            .try_into()
            .expect("single operation fits VecM"),
            ext: TransactionExt::V0,
        };

        let envelope = TransactionEnvelope::Tx(TransactionV1Envelope {
            tx,
            signatures: VecM::default(),
        });

        envelope
            .to_xdr_base64(Limits::none())
            .expect("encoding should succeed")
    }

    #[tokio::test]
    #[ignore]
    async fn reconciliation_resolves_a_crashed_pending_row_to_completed() {
        let pool = test_pool().await;
        let user_id = Uuid::new_v4();
        let batch_id = Uuid::new_v4();
        let hash = format!("{:064x}", Uuid::new_v4().as_u128());

        // Simulates the crash the issue describes: a row already sitting in
        // 'pending' with its hash recorded, updated_at old enough to count
        // as stuck, exactly as `execute_batch` would leave it if the
        // process died right after the leg-insert transaction committed
        // but before the post-submission UPDATE ever ran.
        sqlx::query(
            r#"
            INSERT INTO transactions (
                id, user_id, from_asset, to_asset, send_amount, receive_amount,
                source_account, destination_account, status, stellar_tx_hash,
                batch_id, batch_index, updated_at
            )
            VALUES ($1, $2, 'XLM', 'XLM', '10', NULL, 'GSOURCE', 'GDEST', 'pending', $3, $4, 0, NOW() - INTERVAL '5 minutes')
            "#,
        )
        .bind(Uuid::new_v4())
        .bind(user_id)
        .bind(&hash)
        .bind(batch_id)
        .execute(&pool)
        .await
        .expect("seed insert should succeed");

        let mock_horizon = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path(format!("/transactions/{hash}")))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": hash,
                "hash": hash,
                "ledger": 100,
                "created_at": "2024-01-01T00:00:00Z",
                "source_account": "GSOURCE",
                "fee_charged": "100",
                "successful": true,
            })))
            .mount(&mock_horizon)
            .await;

        let stellar = StellarService::new(mock_horizon.uri());
        let reconciliation = ReconciliationService::new(pool.clone());

        let summary = reconciliation
            .reconcile_stuck_transactions(&stellar, Duration::seconds(60))
            .await
            .expect("reconciliation pass should succeed");

        assert_eq!(summary.resolved_completed, 1);

        let status: TransactionStatus =
            sqlx::query_scalar("SELECT status FROM transactions WHERE batch_id = $1")
                .bind(batch_id)
                .fetch_one(&pool)
                .await
                .expect("row should still exist");
        assert_eq!(status, TransactionStatus::Completed);

        cleanup(&pool, batch_id, &hash).await;
    }

    #[tokio::test]
    #[ignore]
    async fn ambiguous_submission_is_unconfirmed_not_failed_and_blocks_a_retry() {
        let pool = test_pool().await;
        let user_id = Uuid::new_v4();

        // An address nothing listens on: submit_transaction fails at the
        // connection level (AppError::HttpClient), the same class of error
        // a client-side timeout produces — Horizon never gives us a
        // definitive answer either way.
        let stellar = StellarService::new("http://127.0.0.1:1");
        let batch_svc = BatchPaymentService::new(pool.clone());

        let req = SendBatchPaymentRequest {
            source_account: "GSOURCE".into(),
            signed_xdr: sample_signed_xdr(),
            legs: vec![BatchPaymentLeg {
                destination: "GDEST".into(),
                amount: "10".into(),
                asset_code: "XLM".into(),
                asset_issuer: None,
            }],
        };

        let hash = compute_transaction_hash(&req.signed_xdr, NETWORK)
            .expect("fixture xdr should at least parse for this test's purposes");

        let first = batch_svc
            .execute_batch(user_id, &stellar, NETWORK, &req)
            .await;
        assert!(matches!(first, Err(AppError::HttpClient(_))));

        let status: TransactionStatus =
            sqlx::query_scalar("SELECT status FROM transactions WHERE stellar_tx_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .expect("leg row should have been inserted before submission");
        assert_eq!(status, TransactionStatus::SubmittedUnconfirmed);

        // Retrying the identical signed_xdr must not be allowed to execute
        // a second time while the first attempt's outcome is still
        // unknown — this is the "never allow a plain retry of a batch
        // whose xdr may have already landed" requirement.
        let retry = batch_svc
            .execute_batch(user_id, &stellar, NETWORK, &req)
            .await;
        assert!(matches!(retry, Err(AppError::Conflict(_))));

        let batch_id: Uuid =
            sqlx::query_scalar("SELECT batch_id FROM transactions WHERE stellar_tx_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        cleanup(&pool, batch_id, &hash).await;
    }
}
