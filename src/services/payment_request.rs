use crate::{
    error::{AppError, AppResult},
    models::payment_request::{
        CreatePaymentRequestRequest, FulfillPaymentRequestRequest, PaymentRequest,
        PaymentRequestRow, PaymentRequestStatus,
    },
};
use chrono::{Duration as ChronoDuration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

pub const MAX_MEMO_BYTES: usize = 28;

pub struct PaymentRequestService {
    pool: PgPool,
}

impl PaymentRequestService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    /// Validates the request fields prior to DB insertion.
    pub fn validate_create_request(req: &CreatePaymentRequestRequest) -> AppResult<()> {
        let amount: f64 = req
            .amount
            .parse()
            .map_err(|_| AppError::Validation("amount must be a valid decimal string".into()))?;
        if amount <= 0.0 {
            return Err(AppError::Validation("amount must be positive".into()));
        }
        if req.requester_account.trim().is_empty() {
            return Err(AppError::Validation("requester_account is required".into()));
        }
        if let Some(memo) = &req.memo {
            if memo.len() > MAX_MEMO_BYTES {
                return Err(AppError::Validation(format!(
                    "memo exceeds Stellar's 28-byte limit (got {} bytes)",
                    memo.len()
                )));
            }
        }
        Ok(())
    }

    // ─── Create ───────────────────────────────────────────────────────────────

    pub async fn create(
        &self,
        requester_id: Uuid,
        req: &CreatePaymentRequestRequest,
    ) -> AppResult<PaymentRequest> {
        Self::validate_create_request(req)?;

        let expires_at = req
            .expires_in_secs
            .map(|secs| Utc::now() + ChronoDuration::seconds(secs));

        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, PaymentRequestRow>(
            r#"
            INSERT INTO payment_requests (
                id, requester_id, requester_account, payer_account, asset_code,
                asset_issuer, amount, memo, status, expires_at
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(requester_id)
        .bind(&req.requester_account)
        .bind(&req.payer_account)
        .bind(&req.asset_code)
        .bind(&req.asset_issuer)
        .bind(&req.amount)
        .bind(&req.memo)
        .bind(expires_at)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.into())
    }

    // ─── Read ─────────────────────────────────────────────────────────────────

    pub async fn get_by_id(&self, id: Uuid) -> AppResult<Option<PaymentRequest>> {
        let row =
            sqlx::query_as::<_, PaymentRequestRow>("SELECT * FROM payment_requests WHERE id = $1")
                .bind(id)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(PaymentRequest::from))
    }

    pub async fn list_for_user(&self, requester_id: Uuid) -> AppResult<Vec<PaymentRequest>> {
        let rows = sqlx::query_as::<_, PaymentRequestRow>(
            "SELECT * FROM payment_requests WHERE requester_id = $1 ORDER BY created_at DESC",
        )
        .bind(requester_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows.into_iter().map(PaymentRequest::from).collect())
    }

    // ─── Fulfill ──────────────────────────────────────────────────────────────

    /// Mark a payment request fulfilled. Called after the payer's
    /// client-signed payment (or the on-chain `fulfill_payment_request`
    /// call) has already been submitted — this endpoint only records the
    /// outcome, it never moves funds itself.
    pub async fn fulfill(
        &self,
        id: Uuid,
        req: &FulfillPaymentRequestRequest,
    ) -> AppResult<PaymentRequest> {
        let existing = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Payment request".into()))?;

        self.assert_actionable(&existing)?;

        if let Some(restricted_payer) = &existing.payer_account {
            if restricted_payer != &req.payer_account {
                return Err(AppError::Forbidden);
            }
        }

        let row = sqlx::query_as::<_, PaymentRequestRow>(
            r#"
            UPDATE payment_requests
            SET status = 'fulfilled', fulfilled_tx_hash = $2, fulfilled_by = $3, updated_at = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(&req.tx_hash)
        .bind(&req.payer_account)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.into())
    }

    /// Cancel a payment request. Only the original requester may cancel it.
    pub async fn cancel(&self, requester_id: Uuid, id: Uuid) -> AppResult<PaymentRequest> {
        let existing = self
            .get_by_id(id)
            .await?
            .ok_or_else(|| AppError::NotFound("Payment request".into()))?;

        if existing.requester_id != requester_id {
            return Err(AppError::Forbidden);
        }

        self.assert_actionable(&existing)?;

        let row = sqlx::query_as::<_, PaymentRequestRow>(
            r#"
            UPDATE payment_requests
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

    /// Reject actions on requests that are already terminal or have expired
    /// (lazily transitioning `pending` -> `expired` on read so a stale
    /// request can never be fulfilled after its deadline).
    fn assert_actionable(&self, request: &PaymentRequest) -> AppResult<()> {
        if let Some(expires_at) = request.expires_at {
            if expires_at < Utc::now() && request.status == PaymentRequestStatus::Pending {
                return Err(AppError::Expired("Payment request".into()));
            }
        }

        match request.status {
            PaymentRequestStatus::Pending => Ok(()),
            PaymentRequestStatus::Fulfilled => {
                Err(AppError::Conflict("Payment request already fulfilled".into()))
            }
            PaymentRequestStatus::Cancelled => {
                Err(AppError::Conflict("Payment request already cancelled".into()))
            }
            PaymentRequestStatus::Expired => Err(AppError::Expired("Payment request".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn sample_request(status: PaymentRequestStatus, expires_at: Option<chrono::DateTime<Utc>>) -> PaymentRequest {
        PaymentRequest {
            id: Uuid::new_v4(),
            requester_id: Uuid::new_v4(),
            requester_account: "GREQUESTER".into(),
            payer_account: None,
            asset_code: "XLM".into(),
            asset_issuer: None,
            amount: "10".into(),
            memo: None,
            status,
            onchain_request_id: None,
            fulfilled_tx_hash: None,
            fulfilled_by: None,
            expires_at,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn sample_create_req(amount: &str, requester: &str, memo: Option<&str>) -> CreatePaymentRequestRequest {
        CreatePaymentRequestRequest {
            requester_account: requester.to_string(),
            payer_account: None,
            asset_code: "XLM".to_string(),
            asset_issuer: None,
            amount: amount.to_string(),
            memo: memo.map(|s| s.to_string()),
            expires_in_secs: None,
        }
    }

    #[test]
    fn test_memo_length_validation() {
        // Valid: no memo
        let req = sample_create_req("10.0", "GREQUESTER", None);
        assert!(PaymentRequestService::validate_create_request(&req).is_ok());

        // Valid: empty memo
        let req = sample_create_req("10.0", "GREQUESTER", Some(""));
        assert!(PaymentRequestService::validate_create_request(&req).is_ok());

        // Valid: exactly 28 ASCII bytes
        let memo_28_ascii = "1234567890123456789012345678";
        assert_eq!(memo_28_ascii.len(), 28);
        let req = sample_create_req("10.0", "GREQUESTER", Some(memo_28_ascii));
        assert!(PaymentRequestService::validate_create_request(&req).is_ok());

        // Invalid: 29 ASCII bytes
        let memo_29_ascii = "12345678901234567890123456789";
        assert_eq!(memo_29_ascii.len(), 29);
        let req = sample_create_req("10.0", "GREQUESTER", Some(memo_29_ascii));
        let err = PaymentRequestService::validate_create_request(&req).unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));

        // Multi-byte UTF-8 test:
        // "🦀" is 1 char but 4 UTF-8 bytes.
        // 7 crabs = 7 chars, exactly 28 bytes -> valid.
        let memo_7_crabs = "🦀🦀🦀🦀🦀🦀🦀";
        assert_eq!(memo_7_crabs.chars().count(), 7);
        assert_eq!(memo_7_crabs.len(), 28);
        let req = sample_create_req("10.0", "GREQUESTER", Some(memo_7_crabs));
        assert!(PaymentRequestService::validate_create_request(&req).is_ok());

        // 8 crabs = 8 chars, 32 bytes (> 28 bytes) -> must be rejected even though chars.count() is only 8
        let memo_8_crabs = "🦀🦀🦀🦀🦀🦀🦀🦀";
        assert_eq!(memo_8_crabs.chars().count(), 8);
        assert_eq!(memo_8_crabs.len(), 32);
        let req = sample_create_req("10.0", "GREQUESTER", Some(memo_8_crabs));
        let err = PaymentRequestService::validate_create_request(&req).unwrap_err();
        assert!(matches!(err, AppError::Validation(_)));
    }

    #[tokio::test]
    async fn pending_unexpired_request_is_actionable() {
        let svc_check = PaymentRequestService { pool: dummy_pool() };
        let req = sample_request(PaymentRequestStatus::Pending, Some(Utc::now() + Duration::hours(1)));
        assert!(svc_check.assert_actionable(&req).is_ok());
    }

    #[tokio::test]
    async fn expired_pending_request_is_rejected() {
        let svc_check = PaymentRequestService { pool: dummy_pool() };
        let req = sample_request(PaymentRequestStatus::Pending, Some(Utc::now() - Duration::hours(1)));
        let err = svc_check.assert_actionable(&req).unwrap_err();
        assert!(matches!(err, AppError::Expired(_)));
    }

    #[tokio::test]
    async fn already_fulfilled_request_is_rejected() {
        let svc_check = PaymentRequestService { pool: dummy_pool() };
        let req = sample_request(PaymentRequestStatus::Fulfilled, None);
        let err = svc_check.assert_actionable(&req).unwrap_err();
        assert!(matches!(err, AppError::Conflict(_)));
    }

    /// A `PgPool` that is never actually connected to — fine because the
    /// functions under test here (`assert_actionable`) never touch `self.pool`.
    fn dummy_pool() -> PgPool {
        PgPool::connect_lazy("postgres://user:pass@localhost/db")
            .expect("lazy pool construction does not touch the network")
    }
}
