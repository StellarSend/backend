use crate::{
    db::is_unique_violation,
    error::{AppError, AppResult},
    models::{
        batch_payment::{BatchPaymentResult, SendBatchPaymentRequest},
        transaction::TransactionStatus,
    },
    services::{stellar::StellarService, tx_hash::compute_transaction_hash},
};
use sqlx::PgPool;
use uuid::Uuid;

/// Business logic for split/batch payments: one client-signed transaction,
/// many recipients, relayed once and recorded as one `transactions` row per
/// leg (same non-custodial pattern as a normal send — the client already
/// built and signed the whole batch).
pub struct BatchPaymentService {
    pool: PgPool,
}


/// Decides what a failed submission attempt means for transaction status
/// (#30). The distinction is "did we get a definitive answer from Horizon
/// at all":
///
///  - `HorizonError` means Horizon received the request and rejected it —
///    that's a real, final answer, safe to record as `Failed`.
///  - Anything else (`HttpClient`, i.e. a connection failure or timeout —
///    the only other variant `StellarService::submit_transaction` can
///    return) means we never got Horizon's answer. The transaction may
///    still have landed, so this is `SubmittedUnconfirmed`, not `Failed` —
///    a false-negative failure here is what leads a caller to safely (but
///    wrongly) retry into a double payment. `ReconciliationService`
///    resolves it later by asking Horizon directly.
fn classify_submission_error(error: &AppError) -> TransactionStatus {
    match error {
        AppError::HorizonError(_) => TransactionStatus::Failed,
        _ => TransactionStatus::SubmittedUnconfirmed,
    }
}

impl BatchPaymentService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn execute_batch(
        &self,
        user_id: Uuid,
        stellar: &StellarService,
        network_passphrase: &str,
        req: &SendBatchPaymentRequest,
    ) -> AppResult<BatchPaymentResult> {
        if req.legs.is_empty() {
            return Err(AppError::Validation(
                "A batch payment must include at least one recipient".into(),
            ));
        }
        if req.legs.len() > 100 {
            // Stellar caps operations per transaction at 100.
            return Err(AppError::Validation(
                "A batch payment cannot include more than 100 recipients".into(),
            ));
        }
        for leg in &req.legs {
            let amount: f64 = leg
                .amount
                .parse()
                .map_err(|_| AppError::Validation(format!("Invalid amount for {}", leg.destination)))?;
            if amount <= 0.0 {
                return Err(AppError::Validation(format!(
                    "Amount for {} must be positive",
                    leg.destination
                )));
            }
        }
        if req.signed_xdr.trim().is_empty() {
            return Err(AppError::BadRequest("signed_xdr must not be empty".into()));
        }

        // Compute the transaction hash locally, before ever talking to
        // Horizon, so a crash between here and the post-submission UPDATE
        // still leaves a durable link between our rows and the actual
        // on-chain transaction — reconciliation can look it up by this
        // hash even if the submission response itself is lost (#30).
        let tx_hash = compute_transaction_hash(&req.signed_xdr, network_passphrase)?;

        // Claim the hash before doing anything else. A primary-key
        // violation here means this exact signed transaction was already
        // submitted (by this caller retrying, or a genuinely concurrent
        // request) — reject outright rather than risk a double payment.
        // The atomic INSERT is what actually closes this race; checking
        // "does a row for this hash exist?" first and inserting after
        // would leave the identical TOCTOU gap the underlying bug is made
        // of, just moved one level up.
        let batch_id = Uuid::new_v4();
        if let Err(e) =
            sqlx::query("INSERT INTO batch_submissions (stellar_tx_hash, batch_id) VALUES ($1, $2)")
                .bind(&tx_hash)
                .bind(batch_id)
                .execute(&self.pool)
                .await
        {
            if is_unique_violation(&e) {
                return Err(AppError::Conflict(
                    "A batch payment with this signed transaction has already been submitted"
                        .into(),
                ));
            }
            return Err(e.into());
        }

        // All-or-nothing: insert every leg row in a single DB transaction
        // so a crash mid-loop can never leave a partial batch (gaps in
        // batch_index) that was never actually submitted.
        let mut leg_ids = Vec::with_capacity(req.legs.len());
        let mut db_tx = self.pool.begin().await?;
        for (index, leg) in req.legs.iter().enumerate() {
            let asset = match &leg.asset_issuer {
                Some(issuer) => format!("{}:{}", leg.asset_code, issuer),
                None => leg.asset_code.clone(),
            };

            let tx_id = Uuid::new_v4();
            sqlx::query(
                r#"
                INSERT INTO transactions (
                    id, user_id, from_asset, to_asset, send_amount, receive_amount,
                    source_account, destination_account, status, stellar_tx_hash,
                    batch_id, batch_index
                )
                VALUES ($1, $2, $3, $3, $4, NULL, $5, $6, 'pending', $7, $8, $9)
                "#,
            )
            .bind(tx_id)
            .bind(user_id)
            .bind(&asset)
            .bind(&leg.amount)
            .bind(&req.source_account)
            .bind(&leg.destination)
            .bind(&tx_hash)
            .bind(batch_id)
            .bind(index as i32)
            .execute(&mut *db_tx)
            .await?;

            leg_ids.push(tx_id);
        }
        db_tx.commit().await?;

        // Submit the single, already-signed batch transaction once.
        let submission = stellar.submit_transaction(&req.signed_xdr).await;

        match submission {
            Ok(result) => {
                if result.hash != tx_hash {
                    // Should never happen — would mean our local hash
                    // computation disagrees with Horizon's, which breaks
                    // the reconciliation link for this batch. Surface it
                    // loudly without failing the (already-successful)
                    // request.
                    tracing::error!(
                        computed_hash = %tx_hash,
                        horizon_hash = %result.hash,
                        batch_id = %batch_id,
                        "Locally computed transaction hash does not match Horizon's reported hash"
                    );
                }

                sqlx::query(
                    r#"
                    UPDATE transactions
                    SET status = $2, stellar_tx_hash = $3, updated_at = NOW()
                    WHERE batch_id = $1
                    "#,
                )
                .bind(batch_id)
                .bind(TransactionStatus::Completed)
                .bind(&result.hash)
                .execute(&self.pool)
                .await?;

                Ok(BatchPaymentResult {
                    batch_id,
                    tx_hash: result.hash,
                    success: result.successful,
                    ledger: result.ledger,
                    fee_charged: result.fee_charged,
                    leg_transaction_ids: leg_ids,
                })
            }
            Err(e) => {
                let status = classify_submission_error(&e);

                sqlx::query(
                    r#"
                    UPDATE transactions
                    SET status = $2, error_message = $3, updated_at = NOW()
                    WHERE batch_id = $1
                    "#,
                )
                .bind(batch_id)
                .bind(status)
                .bind(e.to_string())
                .execute(&self.pool)
                .await?;

                Err(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::batch_payment::BatchPaymentLeg;

    const NETWORK: &str = "Test SDF Network ; September 2015";

    fn dummy_pool() -> PgPool {
        PgPool::connect_lazy("postgres://user:pass@localhost/db").unwrap()
    }

    #[tokio::test]
    async fn rejects_empty_batch() {
        let svc = BatchPaymentService::new(dummy_pool());
        let stellar = StellarService::new("https://horizon-testnet.stellar.org");
        let req = SendBatchPaymentRequest {
            source_account: "GSOURCE".into(),
            signed_xdr: "AAAA".into(),
            legs: vec![],
        };

        let err = svc
            .execute_batch(Uuid::new_v4(), &stellar, NETWORK, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn rejects_non_positive_leg_amount() {
        let svc = BatchPaymentService::new(dummy_pool());
        let stellar = StellarService::new("https://horizon-testnet.stellar.org");
        let req = SendBatchPaymentRequest {
            source_account: "GSOURCE".into(),
            signed_xdr: "AAAA".into(),
            legs: vec![BatchPaymentLeg {
                destination: "GDEST".into(),
                amount: "0".into(),
                asset_code: "XLM".into(),
                asset_issuer: None,
            }],
        };

        let err = svc
            .execute_batch(Uuid::new_v4(), &stellar, NETWORK, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn rejects_invalid_signed_xdr_before_touching_the_database() {
        // A batch that passes leg validation but carries un-parseable
        // signed_xdr must fail at hash computation, before any DB call —
        // exercised here via a lazy (never-connects) pool to prove it
        // never tries to touch the database.
        let svc = BatchPaymentService::new(dummy_pool());
        let stellar = StellarService::new("https://horizon-testnet.stellar.org");
        let req = SendBatchPaymentRequest {
            source_account: "GSOURCE".into(),
            signed_xdr: "not-valid-xdr".into(),
            legs: vec![BatchPaymentLeg {
                destination: "GDEST".into(),
                amount: "10".into(),
                asset_code: "XLM".into(),
                asset_issuer: None,
            }],
        };

        let err = svc
            .execute_batch(Uuid::new_v4(), &stellar, NETWORK, &req)
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
    fn classifies_anything_else_as_submitted_unconfirmed() {
        // AppError::HttpClient wraps a reqwest::Error, which has no public
        // constructor; Internal(anyhow) exercises the same "not a
        // HorizonError" branch classify_submission_error actually
        // switches on, without needing a real network error to build one.
        let err = AppError::Internal(anyhow::anyhow!("connection reset"));
        assert_eq!(
            classify_submission_error(&err),
            TransactionStatus::SubmittedUnconfirmed
        );
    }
}
