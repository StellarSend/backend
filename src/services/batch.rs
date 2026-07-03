use crate::{
    error::{AppError, AppResult},
    models::{
        batch_payment::{BatchPaymentResult, SendBatchPaymentRequest},
        transaction::TransactionStatus,
    },
    services::stellar::StellarService,
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

impl BatchPaymentService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn execute_batch(
        &self,
        user_id: Uuid,
        stellar: &StellarService,
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

        let batch_id = Uuid::new_v4();

        // Pre-create one pending transaction row per leg, all sharing batch_id.
        let mut leg_ids = Vec::with_capacity(req.legs.len());
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
                    source_account, destination_account, status, batch_id, batch_index
                )
                VALUES ($1, $2, $3, $3, $4, NULL, $5, $6, 'pending', $7, $8)
                "#,
            )
            .bind(tx_id)
            .bind(user_id)
            .bind(&asset)
            .bind(&leg.amount)
            .bind(&req.source_account)
            .bind(&leg.destination)
            .bind(batch_id)
            .bind(index as i32)
            .execute(&self.pool)
            .await?;

            leg_ids.push(tx_id);
        }

        // Submit the single, already-signed batch transaction once.
        let submission = stellar.submit_transaction(&req.signed_xdr).await;

        match submission {
            Ok(result) => {
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
                sqlx::query(
                    r#"
                    UPDATE transactions
                    SET status = $2, error_message = $3, updated_at = NOW()
                    WHERE batch_id = $1
                    "#,
                )
                .bind(batch_id)
                .bind(TransactionStatus::Failed)
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
            .execute_batch(Uuid::new_v4(), &stellar, &req)
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
            .execute_batch(Uuid::new_v4(), &stellar, &req)
            .await
            .unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }
}
