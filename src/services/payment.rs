use crate::{
    error::{AppError, AppResult},
    models::{
        payment::{PathHop, QuoteRequest, QuoteResponse, SendPaymentRequest, PaymentResult},
        transaction::{CreateTransaction, TransactionStatus},
    },
    services::{
        stellar::StellarService,
        transaction::TransactionService,
    },
};
use uuid::Uuid;

/// Business logic for quoting and executing payments.
pub struct PaymentService {
    stellar: StellarService,
}

impl PaymentService {
    pub fn new(stellar: StellarService) -> Self {
        Self { stellar }
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

        // 2. Look up or create the internal transaction record.
        let internal_tx = if let Some(tx_id) = req.transaction_id {
            // Retrieve existing record and mark as submitted.
            let tx = tx_service
                .get_by_id(tx_id)
                .await?
                .ok_or_else(|| AppError::NotFound("Transaction".into()))?;

            if tx.user_id != user_id {
                return Err(AppError::Forbidden);
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

        // 3. Submit to Horizon.
        match self.stellar.submit_transaction(&req.signed_xdr).await {
            Ok(submission) => {
                // 4a. Update the record as completed.
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
                // 4b. Mark the record as failed.
                tx_service
                    .update_status(
                        internal_tx.id,
                        TransactionStatus::Failed,
                        None,
                        Some(e.to_string()),
                    )
                    .await?;

                Err(e)
            }
        }
    }
}
