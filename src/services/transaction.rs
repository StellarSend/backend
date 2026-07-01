use crate::{
    error::{AppError, AppResult},
    models::transaction::{
        CreateTransaction, PaginatedTransactions, Transaction, TransactionListParams,
        TransactionRow, TransactionStatus,
    },
};
use sqlx::PgPool;
use uuid::Uuid;

/// CRUD operations for the `transactions` table.
pub struct TransactionService {
    pool: PgPool,
}

impl TransactionService {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    // ─── Create ───────────────────────────────────────────────────────────────

    pub async fn create(&self, data: CreateTransaction) -> AppResult<Transaction> {
        let id = Uuid::new_v4();
        let path_json = data.path.map(|p| serde_json::to_value(p).ok()).flatten();

        let row = sqlx::query_as::<_, TransactionRow>(
            r#"
            INSERT INTO transactions (
                id, user_id, from_asset, to_asset, send_amount, receive_amount,
                source_account, destination_account, status, fee_xlm, path
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'pending', $9, $10)
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(data.user_id)
        .bind(&data.from_asset)
        .bind(&data.to_asset)
        .bind(&data.send_amount)
        .bind(&data.receive_amount)
        .bind(&data.source_account)
        .bind(&data.destination_account)
        .bind(&data.fee_xlm)
        .bind(&path_json)
        .fetch_one(&self.pool)
        .await?;

        Ok(row.into())
    }

    // ─── Read ─────────────────────────────────────────────────────────────────

    pub async fn get_by_id(&self, id: Uuid) -> AppResult<Option<Transaction>> {
        let row = sqlx::query_as::<_, TransactionRow>(
            "SELECT * FROM transactions WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;

        Ok(row.map(Transaction::from))
    }

    /// List transactions for a specific user with optional filters and pagination.
    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        params: &TransactionListParams,
    ) -> AppResult<PaginatedTransactions> {
        let page = params.page.unwrap_or(1).max(1);
        let per_page = params.per_page.unwrap_or(20).clamp(1, 100);
        let offset = (page - 1) * per_page;

        // Build WHERE clauses dynamically.
        let mut conditions = vec!["user_id = $1".to_string()];
        let mut param_idx = 2usize;

        if params.status.is_some() {
            conditions.push(format!("status = ${param_idx}"));
            param_idx += 1;
        }
        if params.from_asset.is_some() {
            conditions.push(format!("from_asset = ${param_idx}"));
            param_idx += 1;
        }
        if params.to_asset.is_some() {
            conditions.push(format!("to_asset = ${param_idx}"));
            param_idx += 1;
        }

        let where_clause = conditions.join(" AND ");
        let count_sql = format!("SELECT COUNT(*) FROM transactions WHERE {where_clause}");
        let data_sql = format!(
            "SELECT * FROM transactions WHERE {where_clause} ORDER BY created_at DESC LIMIT ${param_idx} OFFSET ${}",
            param_idx + 1
        );

        // Count query
        let mut count_query = sqlx::query_scalar::<_, i64>(&count_sql).bind(user_id);
        if let Some(s) = &params.status {
            count_query = count_query.bind(s);
        }
        if let Some(f) = &params.from_asset {
            count_query = count_query.bind(f);
        }
        if let Some(t) = &params.to_asset {
            count_query = count_query.bind(t);
        }
        let total: i64 = count_query.fetch_one(&self.pool).await?;

        // Data query
        let mut data_query = sqlx::query_as::<_, TransactionRow>(&data_sql).bind(user_id);
        if let Some(s) = &params.status {
            data_query = data_query.bind(s);
        }
        if let Some(f) = &params.from_asset {
            data_query = data_query.bind(f);
        }
        if let Some(t) = &params.to_asset {
            data_query = data_query.bind(t);
        }
        let rows = data_query
            .bind(per_page as i64)
            .bind(offset as i64)
            .fetch_all(&self.pool)
            .await?;

        let items: Vec<Transaction> = rows.into_iter().map(Transaction::from).collect();
        let total_pages = ((total as f64) / (per_page as f64)).ceil() as u32;

        Ok(PaginatedTransactions {
            items,
            total,
            page,
            per_page,
            total_pages,
        })
    }

    // ─── Update ───────────────────────────────────────────────────────────────

    /// Update a transaction's status, optionally recording the Stellar hash and/or error.
    pub async fn update_status(
        &self,
        id: Uuid,
        status: TransactionStatus,
        stellar_tx_hash: Option<String>,
        error_message: Option<String>,
    ) -> AppResult<Transaction> {
        let row = sqlx::query_as::<_, TransactionRow>(
            r#"
            UPDATE transactions
            SET
                status = $2,
                stellar_tx_hash = COALESCE($3, stellar_tx_hash),
                error_message   = COALESCE($4, error_message),
                updated_at      = NOW()
            WHERE id = $1
            RETURNING *
            "#,
        )
        .bind(id)
        .bind(status)
        .bind(stellar_tx_hash)
        .bind(error_message)
        .fetch_optional(&self.pool)
        .await?
        .ok_or_else(|| AppError::NotFound("Transaction".into()))?;

        Ok(row.into())
    }
}
