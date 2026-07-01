use crate::config::Config;
use anyhow::{Context, Result};
use sqlx::{postgres::PgPoolOptions, PgPool};
use std::time::Duration;

/// Create and configure the PostgreSQL connection pool.
pub async fn create_pool(config: &Config) -> Result<PgPool> {
    let pool = PgPoolOptions::new()
        .max_connections(config.database_max_connections)
        .min_connections(config.database_min_connections)
        .acquire_timeout(Duration::from_secs(config.database_connect_timeout_secs))
        .connect(&config.database_url)
        .await
        .context("Failed to connect to the database")?;

    tracing::info!("Database pool established");
    Ok(pool)
}

/// Run all embedded SQL migrations in order.
pub async fn run_migrations(pool: &PgPool) -> Result<()> {
    tracing::info!("Running database migrations…");
    sqlx::migrate!("./migrations")
        .run(pool)
        .await
        .context("Failed to run database migrations")?;
    tracing::info!("Migrations complete");
    Ok(())
}
