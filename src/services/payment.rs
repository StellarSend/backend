use crate::{
    error::{AppError, AppResult},
    models::{
        payment::{PathHop, QuoteRequest, QuoteResponse, SendPaymentRequest, PaymentResult},
        transaction::{CreateTransaction, TransactionStatus},
    },
    services::{
        stellar::StellarService,
        transaction::TransactionService,
        tx_hash::compute_transaction_hash,
    },
};
use sqlx::PgPool;
use uuid::Uuid;

/// Business logic for quoting and executing payments.
pub struct PaymentService {
    stellar: StellarService,
    pool: PgPool,
    network_passphrase: String,
}

/// True if `error` is a Postgres unique/primary-key violation — used to
/// detect losing the race to claim a `payment_submissions` row.
fn is_unique_violation(error: &sqlx::Error) -> bool {
    matches!(error, sqlx::Error::Database(db_err) if db_err.is_unique_violation())
}

/// Decides what a failed submission attempt means for transaction status
/// (#35, mirrors batch.rs::classify_submission_error from #30).
///
///  - `HorizonError`: Horizon received and rejected the request — definitive,
///    safe to record as `Failed`.
///  - Anything else (connection failure, timeout — `AppError::HttpClient`):
///    Horizon may have processed the request anyway; we never got the answer.
///    Record as `SubmittedUnconfirmed` so `ReconciliationService` can resolve
///    it by looking the hash up on Horizon directly, rather than letting the
///    caller assume failure and retry into a potential double payment.
fn classify_submission_error(error: &AppError) -> TransactionStatus {
    match error {
        AppError::HorizonError(_) => TransactionStatus::Failed,
        _ => TransactionStatus::SubmittedUnconfirmed,
    }
}

impl PaymentService {
    pub fn new(stellar: StellarService, pool: PgPool, network_passphrase: impl Into<String>) -> Self {
        Self {
            stellar,
            pool,
            network_passphrase: network_passphrase.into(),
        }
    }

    // ─── Quote ────────────────────────────────────────────────────────────────

    /// Retrieve the best path-payment quote for a given send/receive pair.
    ///
    /// Internally calls `GET /paths/strict-send` and picks the record with the
    /// highest `destination_amount` (best deal for the sender).
    pub async fn get_quote(&self, req: &QuoteRequest) -> AppResult<QuoteResponse> {
        // Validate send amount.
        let send_f: f64 = req
            .amount
            .parse()
            .map_err(|_| AppError::BadRequest("Invalid send amount".into()))?;

        if send_f <= 0.0 {
            return Err(AppError::BadRequest(
                "Send amount must be positive".into(),
            ));
        }

        // Fetch paths from Horizon.
        let paths = self
            .stellar
            .get_path_payment_paths(
                &req.from_asset,
                &req.to_asset,
                &req.amount,
                &req.destination,
            )
            .await?;

        if paths.is_empty() {
            return Err(AppError::NoPathFound);
        }

        // Select the path with the highest destination amount.
        let best = paths
            .iter()
            .max_by(|a, b| {
                let fa: f64 = a.destination_amount.parse().unwrap_or(0.0);
                let fb: f64 = b.destination_amount.parse().unwrap_or(0.0);
                fa.partial_cmp(&fb).unwrap_or(std::cmp::Ordering::Equal)
            })
            .ok_or(AppError::NoPathFound)?;

        // Compute implied rate.
        let dest_f: f64 = best.destination_amount.parse().unwrap_or(0.0);
        let rate = if send_f > 0.0 {
            format!("{:.7}", dest_f / send_f)
        } else {
            "0".to_string()
        };

        // Estimate network fee.
        let fee_xlm = self.stellar.estimate_fee().await.unwrap_or_else(|_| "0.00001".to_string());

        // Convert path hops.
        let path: Vec<PathHop> = best
            .path
            .iter()
            .map(StellarService::convert_path_hop)
            .collect();

        Ok(QuoteResponse {
            estimated_receive: best.destination_amount.clone(),
            rate,
            fee_xlm,
            path,
            send_amount: req.amount.clone(),
            from_asset: req.from_asset.clone(),
            to_asset: req.to_asset.clone(),
        })
    }

    // ─── Send ─────────────────────────────────────────────────────────────────

    /// Relay a signed Stellar transaction XDR to Horizon and record the result.
    ///
    /// The client is responsible for building and signing the transaction
    /// client-side (using any Stellar SDK) before calling this endpoint.
    ///
    /// Three guards that were missing before #35:
    ///
    /// 1. **Duplicate-submission guard**: we compute the transaction hash
    ///    locally (before any DB or Horizon call) and atomically INSERT it into
    ///    `payment_submissions`.  A primary-key violation means the exact same
    ///    signed XDR was already dispatched — return `Conflict` immediately
    ///    rather than risking a double payment.
    ///
    /// 2. **Terminal-status guard**: when `req.transaction_id` points to an
    ///    existing record that is already `Completed` or `Failed`, we refuse to
    ///    overwrite it — the first outcome is the authoritative one.
    ///
    /// 3. **Ambiguous-failure classification**: a connection failure or timeout
    ///    (where Horizon may have processed the transaction but we never got the
    ///    response) is recorded as `SubmittedUnconfirmed` rather than `Failed`,
    ///    so `ReconciliationService` can recover it by hash lookup — exactly as
    ///    it does for batches since #30.
    pub async fn execute_send(
        &self,
        user_id: Uuid,
        req: &SendPaymentRequest,
        tx_service: &TransactionService,
    ) -> AppResult<PaymentResult> {
        // 1. Validate the XDR is non-empty.
        if req.signed_xdr.trim().is_empty() {
            return Err(AppError::BadRequest("signed_xdr must not be empty".into()));
        }

        // 2. Compute the canonical transaction hash locally — before touching
        //    the DB or Horizon — so we have a durable key to claim atomically.
        //    An unparseable XDR is rejected here, before any DB work.
        let tx_hash = compute_transaction_hash(&req.signed_xdr, &self.network_passphrase)?;

        // 3. Look up or create the internal transaction record.
        let internal_tx = if let Some(tx_id) = req.transaction_id {
            // Retrieve existing record and verify ownership.
            let tx = tx_service
                .get_by_id(tx_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Transaction".into()))?;

            if tx.user_id != user_id {
                return Err(AppError::Forbidden);
            }

            // Guard: refuse to overwrite a record that already reached a
            // terminal state.  A Completed/Failed outcome is the authoritative
            // record of what happened; silently resubmitting over it would
            // discard that history (#35 gap 2).
            if matches!(tx.status, TransactionStatus::Completed | TransactionStatus::Failed) {
                return Err(AppError::Conflict(
                    "This transaction has already reached a terminal state and cannot be resubmitted".into(),
                ));
            }

            tx_service
                .update_status(tx_id, TransactionStatus::Submitted, None, None)
                .await?;
            tx
        } else {
            // Create a new pending record from the request metadata.
            let from_asset = req
                .from_asset
                .as_ref()
                .map(|a| format!("{}:{}", a.code, a.issuer.as_deref().unwrap_or("native")))
                .unwrap_or_else(|| "XLM".to_string());

            let to_asset = req
                .to_asset
                .as_ref()
                .map(|a| format!("{}:{}", a.code, a.issuer.as_deref().unwrap_or("native")))
                .unwrap_or_else(|| "XLM".to_string());

            tx_service
                .create(CreateTransaction {
                    user_id,
                    from_asset,
                    to_asset,
                    send_amount: req.send_amount.clone().unwrap_or_else(|| "0".to_string()),
                    receive_amount: None,
                    source_account: req.source_account.clone(),
                    destination_account: req
                        .destination_account
                        .clone()
                        .unwrap_or_default(),
                    fee_xlm: None,
                    path: None,
                })
                .await?
        };

        // 4. Atomically claim the hash in `payment_submissions`.  A
        //    primary-key violation means this exact signed transaction was
        //    already submitted (concurrent request, client retry, or a
        //    previously dispatched send that returned an ambiguous status) —
        //    reject outright.  The INSERT is what closes the race; a
        //    check-then-insert pattern would leave the same TOCTOU gap the
        //    underlying bug is made of.
        if let Err(e) = sqlx::query(
            "INSERT INTO payment_submissions (stellar_tx_hash, transaction_id) VALUES ($1, $2)",
        )
        .bind(&tx_hash)
        .bind(internal_tx.id)
        .execute(&self.pool)
        .await
        {
            if is_unique_violation(&e) {
                return Err(AppError::Conflict(
                    "A payment with this signed transaction has already been submitted".into(),
                ));
            }
            return Err(e.into());
        }

        // 5. Submit to Horizon.
        match self.stellar.submit_transaction(&req.signed_xdr).await {
            Ok(submission) => {
                // Sanity-check: our locally computed hash should match what
                // Horizon reports.  A mismatch would break the reconciliation
                // link, so surface it loudly without failing the (already-
                // successful) request.
                if submission.hash != tx_hash {
                    tracing::error!(
                        computed_hash = %tx_hash,
                        horizon_hash  = %submission.hash,
                        transaction_id = %internal_tx.id,
                        "Locally computed transaction hash does not match Horizon's reported hash"
                    );
                }

                tx_service
                    .update_status(
                        internal_tx.id,
                        TransactionStatus::Completed,
                        Some(submission.hash.clone()),
                        None,
                    )
                    .await?;

                Ok(PaymentResult {
                    tx_hash: submission.hash,
                    success: submission.successful,
                    ledger: submission.ledger,
                    fee_charged: submission.fee_charged,
                    transaction_id: internal_tx.id,
                })
            }
            Err(e) => {
                // Classify the failure before recording it (#35 gap 3 / mirrors
                // batch.rs::classify_submission_error from #30):
                //   HorizonError  → definitive rejection, safe to mark Failed.
                //   Anything else → we don't know if Horizon processed it;
                //                   mark SubmittedUnconfirmed for reconciliation.
                let status = classify_submission_error(&e);

                tx_service
                    .update_status(
                        internal_tx.id,
                        status,
                        // Persist the hash we computed — even on failure — so
                        // ReconciliationService can find this row by hash and
                        // converge it to the true on-chain outcome (#35 /
                        // mirrors the batch path from #30).
                        Some(tx_hash),
                        Some(e.to_string()),
                    )
                    .await?;

                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const NETWORK: &str = "Test SDF Network ; September 2015";

    fn dummy_pool() -> PgPool {
        PgPool::connect_lazy("postgres://user:pass@localhost/db").unwrap()
    }

    fn make_service() -> PaymentService {
        PaymentService::new(
            StellarService::new("https://horizon-testnet.stellar.org"),
            dummy_pool(),
            NETWORK,
        )
    }

    #[tokio::test]
    async fn rejects_empty_signed_xdr() {
        let svc = make_service();
        let tx_svc = TransactionService::new(dummy_pool());
        let req = SendPaymentRequest {
            signed_xdr: "   ".into(),
            transaction_id: None,
            source_account: "GSOURCE".into(),
            destination_account: Some("GDEST".into()),
            from_asset: None,
            to_asset: None,
            send_amount: None,
        };

        let err = svc
            .execute_send(Uuid::new_v4(), &req, &tx_svc)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[tokio::test]
    async fn rejects_invalid_signed_xdr_before_touching_the_database() {
        // An XDR that fails hash computation must be rejected before any DB
        // work — confirmed here by using a lazy pool that never connects.
        let svc = make_service();
        let tx_svc = TransactionService::new(dummy_pool());
        let req = SendPaymentRequest {
            signed_xdr: "not-valid-xdr".into(),
            transaction_id: None,
            source_account: "GSOURCE".into(),
            destination_account: Some("GDEST".into()),
            from_asset: None,
            to_asset: None,
            send_amount: None,
        };

        let err = svc
            .execute_send(Uuid::new_v4(), &req, &tx_svc)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn classifies_a_horizon_rejection_as_failed() {
        let err = AppError::HorizonError("tx_bad_seq".into());
        assert_eq!(classify_submission_error(&err), TransactionStatus::Failed);
    }

    #[test]
    fn classifies_connection_failure_as_submitted_unconfirmed() {
        // AppError::Internal exercises the "not a HorizonError" branch the
        // same way a real reqwest::Error (AppError::HttpClient) would, without
        // needing a live network error to construct one.
        let err = AppError::Internal(anyhow::anyhow!("connection reset"));
        assert_eq!(
            classify_submission_error(&err),
            TransactionStatus::SubmittedUnconfirmed
        );
    }
}

/// End-to-end coverage requiring a real Postgres (`DATABASE_URL`) — kept as
/// `#[ignore]`d `#[tokio::test]`s mirroring the db_tests module in
/// reconciliation.rs.  Run with `cargo test -- --ignored` against a real
/// database.
///
/// Covers the two scenarios from #35's "Testing strategy":
///
///  1. Two concurrent `execute_send` calls with the identical `signed_xdr`:
///     exactly one reaches a terminal/unconfirmed state; the other gets a
///     `Conflict`.
///  2. A submission that times out is marked `submitted_unconfirmed` (not
///     `failed`), and a retry with the identical `signed_xdr` is rejected as
///     `Conflict` — same guarantee as the batch path.
#[cfg(test)]
mod db_tests {
    use super::*;
    use crate::{
        error::AppError,
        services::{stellar::StellarService, transaction::TransactionService, tx_hash::compute_transaction_hash},
    };
    use sqlx::postgres::PgPoolOptions;
    use uuid::Uuid;
    use wiremock::{
        matchers::{method, path_regex},
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

    async fn cleanup(pool: &PgPool, hash: &str) {
        let _ = sqlx::query("DELETE FROM transactions WHERE stellar_tx_hash = $1")
            .bind(hash)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM payment_submissions WHERE stellar_tx_hash = $1")
            .bind(hash)
            .execute(pool)
            .await;
    }

    /// A syntactically valid minimal signed_xdr — real enough to parse and hash,
    /// not meant to be submitted to a live network.  Mirrors the fixture in
    /// reconciliation.rs's db_tests.
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
    async fn concurrent_duplicate_send_yields_exactly_one_success_and_one_conflict() {
        let pool = test_pool().await;
        let user_id = Uuid::new_v4();
        let signed_xdr = sample_signed_xdr();
        let hash = compute_transaction_hash(&signed_xdr, NETWORK)
            .expect("fixture xdr must be hashable");

        // Wire the mock Horizon to accept the transaction.
        let mock_horizon = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path_regex("/transactions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "hash": hash,
                "ledger": 100,
                "successful": true,
                "fee_charged": "100",
            })))
            .mount(&mock_horizon)
            .await;

        let make_svc = || {
            let pool = pool.clone();
            let horizon_url = mock_horizon.uri();
            PaymentService::new(
                StellarService::new(&horizon_url),
                pool.clone(),
                NETWORK,
            )
        };

        let req = SendPaymentRequest {
            signed_xdr: signed_xdr.clone(),
            transaction_id: None,
            source_account: "GSOURCE".into(),
            destination_account: Some("GDEST".into()),
            from_asset: None,
            to_asset: None,
            send_amount: Some("10".into()),
        };

        // Issue two concurrent sends with the exact same signed_xdr.
        let tx_svc_1 = TransactionService::new(pool.clone());
        let tx_svc_2 = TransactionService::new(pool.clone());
        let svc_1 = make_svc();
        let svc_2 = make_svc();
        let req2 = req.clone();

        let (r1, r2) = tokio::join!(
            svc_1.execute_send(user_id, &req, &tx_svc_1),
            svc_2.execute_send(user_id, &req2, &tx_svc_2),
        );

        // Exactly one must succeed (or be SubmittedUnconfirmed); the other
        // must be Conflict.  We don't mandate which is which — the DB race
        // determines that.
        let (ok_result, conflict_result) = match (&r1, &r2) {
            (Ok(_), Err(AppError::Conflict(_))) => (&r1, &r2),
            (Err(AppError::Conflict(_)), Ok(_)) => (&r2, &r1),
            _ => panic!(
                "expected exactly one Ok and one Conflict, got:\n  r1={r1:?}\n  r2={r2:?}"
            ),
        };

        assert!(ok_result.is_ok());
        assert!(matches!(conflict_result, Err(AppError::Conflict(_))));

        // Only one `payment_submissions` row should exist.
        let submission_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM payment_submissions WHERE stellar_tx_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(submission_count, 1);

        cleanup(&pool, &hash).await;
    }

    #[tokio::test]
    #[ignore]
    async fn ambiguous_submission_is_unconfirmed_not_failed_and_blocks_a_retry() {
        let pool = test_pool().await;
        let user_id = Uuid::new_v4();
        let signed_xdr = sample_signed_xdr();
        let hash = compute_transaction_hash(&signed_xdr, NETWORK)
            .expect("fixture xdr must be hashable");

        // Point at a port nothing is listening on: submit_transaction fails
        // at the TCP layer (AppError::HttpClient) — same class as a real
        // client-side timeout, Horizon never gives us a definitive answer.
        let svc = PaymentService::new(
            StellarService::new("http://127.0.0.1:1"),
            pool.clone(),
            NETWORK,
        );
        let tx_svc = TransactionService::new(pool.clone());

        let req = SendPaymentRequest {
            signed_xdr: signed_xdr.clone(),
            transaction_id: None,
            source_account: "GSOURCE".into(),
            destination_account: Some("GDEST".into()),
            from_asset: None,
            to_asset: None,
            send_amount: Some("10".into()),
        };

        // First attempt — connection refused.
        let first = svc.execute_send(user_id, &req, &tx_svc).await;
        assert!(matches!(first, Err(AppError::HttpClient(_))));

        // The transaction row must be SubmittedUnconfirmed, not Failed.
        let status: TransactionStatus =
            sqlx::query_scalar("SELECT status FROM transactions WHERE stellar_tx_hash = $1")
                .bind(&hash)
                .fetch_one(&pool)
                .await
                .expect("a transaction row should exist with the computed hash");
        assert_eq!(status, TransactionStatus::SubmittedUnconfirmed);

        // A retry with the identical signed_xdr must be Conflict, not a second
        // submission attempt.
        let tx_svc2 = TransactionService::new(pool.clone());
        let retry = svc.execute_send(user_id, &req, &tx_svc2).await;
        assert!(matches!(retry, Err(AppError::Conflict(_))));

        cleanup(&pool, &hash).await;
    }
}
