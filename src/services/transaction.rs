use crate::{
    error::{AppError, AppResult},
    models::transaction::{
        CreateTransaction, PaginatedTransactions, Transaction, TransactionListParams,
        TransactionRow, TransactionStatus,
    },
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use chrono::{DateTime, SecondsFormat, Utc};
use sqlx::PgPool;
use uuid::Uuid;

/// Encodes a `(created_at, id)` keyset-pagination cursor as an opaque
/// base64 string. Microsecond precision matches Postgres `timestamptz`, so
/// re-decoding round-trips exactly rather than losing precision the DB
/// would then compare against.
fn encode_cursor(created_at: DateTime<Utc>, id: Uuid) -> String {
    let raw = format!(
        "{}|{}",
        created_at.to_rfc3339_opts(SecondsFormat::Micros, true),
        id
    );
    URL_SAFE_NO_PAD.encode(raw)
}

/// Decodes a cursor produced by `encode_cursor`. Any malformed input
/// (tampered, truncated, or simply not a cursor this service issued) is
/// rejected as a client error rather than panicking or silently producing
/// a wrong page.
fn decode_cursor(cursor: &str) -> AppResult<(DateTime<Utc>, Uuid)> {
    let invalid = || AppError::BadRequest("Invalid pagination cursor".to_string());

    let raw = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
    let raw = String::from_utf8(raw).map_err(|_| invalid())?;
    let (created_at_str, id_str) = raw.split_once('|').ok_or_else(invalid)?;

    let created_at = DateTime::parse_from_rfc3339(created_at_str)
        .map_err(|_| invalid())?
        .with_timezone(&Utc);
    let id = Uuid::parse_str(id_str).map_err(|_| invalid())?;

    Ok((created_at, id))
}

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
        let row = sqlx::query_as::<_, TransactionRow>("SELECT * FROM transactions WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;

        Ok(row.map(Transaction::from))
    }

    /// List transactions for a specific user with optional filters and pagination.
    ///
    /// Defaults to cursor/keyset pagination (see `list_for_user_cursor`),
    /// which never runs a `COUNT(*)`. Passing `page` opts into the legacy
    /// offset-based mode (see `list_for_user_offset`) for existing
    /// consumers during the transition — see #54.
    pub async fn list_for_user(
        &self,
        user_id: Uuid,
        params: &TransactionListParams,
    ) -> AppResult<PaginatedTransactions> {
        if params.page.is_some() {
            self.list_for_user_offset(user_id, params).await
        } else {
            self.list_for_user_cursor(user_id, params).await
        }
    }

    /// Legacy `page`/`per_page` pagination: a full `COUNT(*)` plus an
    /// `OFFSET`-skip on every request. O(offset) in Postgres and rescans
    /// every matching row on every page — see #54. Kept only for backward
    /// compatibility; new/updated callers should use the cursor mode.
    async fn list_for_user_offset(
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
            total: Some(total),
            page: Some(page),
            per_page,
            total_pages: Some(total_pages),
            next_cursor: None,
        })
    }

    /// Cursor/keyset pagination: default mode, avoiding both the
    /// `COUNT(*)` and the `OFFSET`-skip cost of `list_for_user_offset`.
    /// Seeks from `(created_at, id)` rather than counting past N rows, so
    /// cost is independent of how deep into the history the cursor is.
    /// `(created_at, id)` (rather than `created_at` alone) breaks ties
    /// between rows sharing an identical timestamp — real for batch-payment
    /// legs inserted in the same transaction — without skipping or
    /// duplicating rows across pages. See #54.
    async fn list_for_user_cursor(
        &self,
        user_id: Uuid,
        params: &TransactionListParams,
    ) -> AppResult<PaginatedTransactions> {
        let per_page = params.per_page.unwrap_or(20).clamp(1, 100);
        let cursor = params.cursor.as_deref().map(decode_cursor).transpose()?;

        // Filter conditions only, kept separate from the cursor predicate
        // below so an opt-in `total` reflects everything matching the
        // filters, not just what's left after the cursor position.
        let mut filter_conditions = vec!["user_id = $1".to_string()];
        let mut param_idx = 2usize;

        if params.status.is_some() {
            filter_conditions.push(format!("status = ${param_idx}"));
            param_idx += 1;
        }
        if params.from_asset.is_some() {
            filter_conditions.push(format!("from_asset = ${param_idx}"));
            param_idx += 1;
        }
        if params.to_asset.is_some() {
            filter_conditions.push(format!("to_asset = ${param_idx}"));
            param_idx += 1;
        }

        let total = if params.include_total.unwrap_or(false) {
            let count_sql = format!(
                "SELECT COUNT(*) FROM transactions WHERE {}",
                filter_conditions.join(" AND ")
            );
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
            Some(count_query.fetch_one(&self.pool).await?)
        } else {
            None
        };

        let mut data_conditions = filter_conditions;
        let cursor_param_idx = param_idx;
        if cursor.is_some() {
            data_conditions.push(format!(
                "(created_at, id) < (${cursor_param_idx}, ${})",
                cursor_param_idx + 1
            ));
            param_idx += 2;
        }

        let limit_idx = param_idx;
        let data_sql = format!(
            "SELECT * FROM transactions WHERE {} ORDER BY created_at DESC, id DESC LIMIT ${limit_idx}",
            data_conditions.join(" AND ")
        );

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
        if let Some((created_at, id)) = cursor {
            data_query = data_query.bind(created_at).bind(id);
        }

        // Fetch one extra row so a next page can be detected without a
        // separate COUNT(*).
        let mut rows = data_query
            .bind(per_page as i64 + 1)
            .fetch_all(&self.pool)
            .await?;

        let next_cursor = if rows.len() > per_page as usize {
            rows.truncate(per_page as usize);
            rows.last().map(|r| encode_cursor(r.created_at, r.id))
        } else {
            None
        };

        let items: Vec<Transaction> = rows.into_iter().map(Transaction::from).collect();
        let total_pages = total.map(|t| ((t as f64) / (per_page as f64)).ceil() as u32);

        Ok(PaginatedTransactions {
            items,
            total,
            page: None,
            per_page,
            total_pages,
            next_cursor,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decode_cursor_round_trips_through_encode_cursor() {
        let created_at = Utc::now();
        let id = Uuid::new_v4();

        let cursor = encode_cursor(created_at, id);
        let (decoded_created_at, decoded_id) =
            decode_cursor(&cursor).expect("a cursor we just encoded should decode");

        // Round-tripped through microsecond-precision RFC3339 text, so
        // compare at that precision rather than requiring bit-identical
        // DateTime<Utc> values.
        assert_eq!(
            decoded_created_at.timestamp_micros(),
            created_at.timestamp_micros()
        );
        assert_eq!(decoded_id, id);
    }

    #[test]
    fn decode_cursor_rejects_garbage_input() {
        let err = decode_cursor("not-valid-base64!!!").unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn decode_cursor_rejects_base64_that_is_not_a_cursor_shape() {
        // Valid base64, but the decoded text has no `|` separator.
        let cursor = URL_SAFE_NO_PAD.encode("not-a-cursor");
        let err = decode_cursor(&cursor).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }

    #[test]
    fn decode_cursor_rejects_an_invalid_uuid_half() {
        let raw = format!(
            "{}|not-a-uuid",
            Utc::now().to_rfc3339_opts(SecondsFormat::Micros, true)
        );
        let cursor = URL_SAFE_NO_PAD.encode(raw);
        let err = decode_cursor(&cursor).unwrap_err();
        assert!(matches!(err, AppError::BadRequest(_)));
    }
}

/// End-to-end coverage requiring a real Postgres (`DATABASE_URL`) — this
/// crate is bin-only (no `src/lib.rs`), so these live here as `#[ignore]`d
/// `#[tokio::test]`s rather than in `tests/`, following
/// `escrow::db_tests`/`reconciliation::db_tests`. Run with
/// `cargo test -- --ignored` against a real database. Covers #54's
/// acceptance criteria: a full result set traversed via cursor comes back
/// with no gaps or duplicates, including across rows sharing an identical
/// `created_at`.
#[cfg(test)]
mod db_tests {
    use super::*;
    use chrono::Duration;
    use sqlx::postgres::PgPoolOptions;
    use std::collections::HashSet;

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

    async fn seed_transaction(pool: &PgPool, user_id: Uuid, created_at: DateTime<Utc>) -> Uuid {
        let id = Uuid::new_v4();
        sqlx::query(
            r#"
            INSERT INTO transactions (
                id, user_id, from_asset, to_asset, send_amount,
                source_account, destination_account, status,
                created_at, updated_at
            )
            VALUES ($1, $2, 'XLM', 'USDC', '10', 'GSOURCE', 'GDEST', 'completed', $3, $3)
            "#,
        )
        .bind(id)
        .bind(user_id)
        .bind(created_at)
        .execute(pool)
        .await
        .expect("seed transaction insert should succeed");
        id
    }

    async fn cleanup(pool: &PgPool, user_id: Uuid) {
        let _ = sqlx::query("DELETE FROM transactions WHERE user_id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("DELETE FROM users WHERE id = $1")
            .bind(user_id)
            .execute(pool)
            .await;
    }

    /// Pages through every row for `user_id` via cursor mode, `per_page` at
    /// a time, until `next_cursor` runs out. Returns the ids in the order
    /// returned so callers can also assert on ordering, not just set
    /// membership.
    async fn collect_all_via_cursor(
        svc: &TransactionService,
        user_id: Uuid,
        per_page: u32,
    ) -> Vec<Uuid> {
        let mut all = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let params = TransactionListParams {
                status: None,
                from_asset: None,
                to_asset: None,
                page: None,
                per_page: Some(per_page),
                cursor: cursor.clone(),
                include_total: None,
            };
            let page = svc
                .list_for_user(user_id, &params)
                .await
                .expect("list_for_user should succeed");

            assert!(
                page.items.len() as u32 <= per_page,
                "a single page must never return more than per_page rows"
            );

            all.extend(page.items.iter().map(|t| t.id));

            match page.next_cursor {
                Some(next) => cursor = Some(next),
                None => break,
            }

            assert!(
                all.len() < 10_000,
                "cursor pagination did not terminate — likely a bug in the keyset predicate"
            );
        }

        all
    }

    #[tokio::test]
    #[ignore]
    async fn cursor_pagination_returns_full_set_with_no_gaps_or_duplicates() {
        let pool = test_pool().await;
        let user_id = seed_user(&pool).await;
        let svc = TransactionService::new(pool.clone());

        let base = Utc::now() - Duration::days(1);
        let mut seeded = HashSet::new();
        for i in 0..125 {
            let id = seed_transaction(&pool, user_id, base + Duration::seconds(i)).await;
            seeded.insert(id);
        }

        let collected = collect_all_via_cursor(&svc, user_id, 20).await;
        let collected_set: HashSet<Uuid> = collected.iter().copied().collect();

        assert_eq!(
            collected.len(),
            125,
            "every seeded row should come back exactly once"
        );
        assert_eq!(
            collected_set, seeded,
            "no row should be skipped or duplicated"
        );

        cleanup(&pool, user_id).await;
    }

    #[tokio::test]
    #[ignore]
    async fn cursor_pagination_preserves_rows_sharing_an_identical_created_at() {
        let pool = test_pool().await;
        let user_id = seed_user(&pool).await;
        let svc = TransactionService::new(pool.clone());

        let base = Utc::now() - Duration::days(1);
        let mut seeded = HashSet::new();

        // Individual rows before the tied group.
        for i in 0..9 {
            let id = seed_transaction(&pool, user_id, base + Duration::seconds(i)).await;
            seeded.insert(id);
        }
        // A block of rows sharing the exact same created_at, simulating
        // batch-payment legs inserted within one transaction (#54).
        let tied_at = base + Duration::seconds(9);
        for _ in 0..12 {
            let id = seed_transaction(&pool, user_id, tied_at).await;
            seeded.insert(id);
        }
        // Individual rows after the tied group.
        for i in 10..19 {
            let id = seed_transaction(&pool, user_id, base + Duration::seconds(i)).await;
            seeded.insert(id);
        }

        assert_eq!(seeded.len(), 30);

        // per_page = 5 guarantees at least one page boundary falls inside
        // the 12-row tied group.
        let collected = collect_all_via_cursor(&svc, user_id, 5).await;
        let collected_set: HashSet<Uuid> = collected.iter().copied().collect();

        assert_eq!(
            collected.len(),
            30,
            "every seeded row, tied or not, should come back exactly once"
        );
        assert_eq!(
            collected_set, seeded,
            "no row should be skipped or duplicated across the tie boundary"
        );

        cleanup(&pool, user_id).await;
    }

    #[tokio::test]
    #[ignore]
    async fn legacy_offset_pagination_still_returns_correct_total_and_items() {
        let pool = test_pool().await;
        let user_id = seed_user(&pool).await;
        let svc = TransactionService::new(pool.clone());

        let base = Utc::now() - Duration::days(1);
        for i in 0..25 {
            seed_transaction(&pool, user_id, base + Duration::seconds(i)).await;
        }

        let params = TransactionListParams {
            status: None,
            from_asset: None,
            to_asset: None,
            page: Some(1),
            per_page: Some(10),
            cursor: None,
            include_total: None,
        };
        let page = svc
            .list_for_user(user_id, &params)
            .await
            .expect("legacy list_for_user should succeed");

        assert_eq!(page.items.len(), 10);
        assert_eq!(page.total, Some(25));
        assert_eq!(page.page, Some(1));
        assert_eq!(page.total_pages, Some(3));
        assert_eq!(page.next_cursor, None, "legacy mode never returns a cursor");

        cleanup(&pool, user_id).await;
    }

    #[tokio::test]
    #[ignore]
    async fn cursor_mode_omits_total_unless_include_total_is_set() {
        let pool = test_pool().await;
        let user_id = seed_user(&pool).await;
        let svc = TransactionService::new(pool.clone());

        let base = Utc::now() - Duration::days(1);
        for i in 0..5 {
            seed_transaction(&pool, user_id, base + Duration::seconds(i)).await;
        }

        let default_params = TransactionListParams {
            status: None,
            from_asset: None,
            to_asset: None,
            page: None,
            per_page: Some(20),
            cursor: None,
            include_total: None,
        };
        let default_page = svc
            .list_for_user(user_id, &default_params)
            .await
            .expect("cursor list_for_user should succeed");
        assert_eq!(
            default_page.total, None,
            "default cursor mode must not run a COUNT(*)"
        );
        assert_eq!(default_page.total_pages, None);

        let with_total_params = TransactionListParams {
            include_total: Some(true),
            ..default_params
        };
        let with_total_page = svc
            .list_for_user(user_id, &with_total_params)
            .await
            .expect("cursor list_for_user with include_total should succeed");
        assert_eq!(with_total_page.total, Some(5));
        assert_eq!(with_total_page.total_pages, Some(1));

        cleanup(&pool, user_id).await;
    }
}
