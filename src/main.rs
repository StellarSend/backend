#![allow(dead_code)] // Public API surface — fields and methods are used by callers
use anyhow::Result;
use sqlx::PgPool;
use std::{
    net::SocketAddr,
    sync::{
        atomic::{AtomicI64, Ordering},
        Arc,
    },
};
use tower_http::{
    request_id::{MakeRequestUuid, PropagateRequestIdLayer, SetRequestIdLayer},
    timeout::TimeoutLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::Level;

mod config;
mod db;
mod error;
mod middleware;
mod models;
mod routes;
mod services;

pub use config::Config;

/// Shared application state — one instance, behind an `Arc`, referenced by
/// every route handler.
pub struct AppState {
    pub pool: PgPool,
    pub config: Config,
    pub loop_health: BackgroundLoopHealth,
}

/// Last-successful-tick timestamps for the keeper and reconciliation
/// background loops, queryable so an operator (or `/health`) can tell those
/// loops are actually still alive rather than having silently stopped —
/// the observability gap this issue leaves for #25 to build alerting on
/// top of (#50). `0` means "never ticked yet"; real unix timestamps are far
/// from `0`, so it's an unambiguous sentinel.
#[derive(Default)]
pub struct BackgroundLoopHealth {
    keeper_last_tick_at: AtomicI64,
    reconciliation_last_tick_at: AtomicI64,
}

impl BackgroundLoopHealth {
    pub fn mark_keeper_ticked(&self) {
        self.keeper_last_tick_at
            .store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
    }

    pub fn mark_reconciliation_ticked(&self) {
        self.reconciliation_last_tick_at
            .store(chrono::Utc::now().timestamp(), Ordering::Relaxed);
    }

    /// Unix timestamp of the keeper loop's last completed tick, or `None`
    /// if it has never ticked (disabled, or not yet reached its first tick).
    pub fn keeper_last_tick_at(&self) -> Option<i64> {
        match self.keeper_last_tick_at.load(Ordering::Relaxed) {
            0 => None,
            ts => Some(ts),
        }
    }

    /// Unix timestamp of the reconciliation loop's last completed tick, or
    /// `None` if it has never ticked yet.
    pub fn reconciliation_last_tick_at(&self) -> Option<i64> {
        match self.reconciliation_last_tick_at.load(Ordering::Relaxed) {
            0 => None,
            ts => Some(ts),
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Load .env if present (silently ignored if missing).
    dotenvy::dotenv().ok();

    // Initialise structured logging.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "stellarsend_backend=debug,tower_http=debug,axum=info".into()),
        )
        .json()
        .init();

    tracing::info!("StellarSend backend starting…");

    // Load config.
    let config = Config::from_env()?;
    tracing::info!(
        host = %config.host,
        port = %config.port,
        horizon_url = %config.horizon_url,
        env = ?config.app_env,
        "Configuration loaded"
    );

    // Establish database pool and run migrations.
    let pool = db::create_pool(&config).await?;
    db::run_migrations(&pool).await?;

    let state = Arc::new(AppState {
        pool,
        config: config.clone(),
        loop_health: BackgroundLoopHealth::default(),
    });

    // Spawn the keeper background loop: periodically scans for subscriptions
    // that are due for execution and submits the on-chain
    // `execute_subscription` call via the keeper service account. This is a
    // plain tokio interval task — no external scheduler/queue — kept simple
    // on purpose. It only runs if a keeper account and contract id are
    // configured; the manual `/api/keeper/run-subscriptions` endpoint always
    // works as a fallback trigger.
    // Backoff shared by both supervised loops: starts at 1s, doubles up to a
    // 60s cap, and resets once a run has stayed up for 30s+ — so a single
    // transient failure doesn't leave a much-later, unrelated failure
    // waiting out a maxed-out delay, but a persistently-panicking pass still
    // backs off instead of spinning the CPU (#50).
    const LOOP_BASE_BACKOFF: std::time::Duration = std::time::Duration::from_secs(1);
    const LOOP_MAX_BACKOFF: std::time::Duration = std::time::Duration::from_secs(60);
    const LOOP_BACKOFF_RESET_AFTER: std::time::Duration = std::time::Duration::from_secs(30);

    if config.keeper_enabled {
        let keeper_state = state.clone();
        tokio::spawn(supervise_loop(
            "keeper",
            LOOP_BASE_BACKOFF,
            LOOP_MAX_BACKOFF,
            LOOP_BACKOFF_RESET_AFTER,
            move || {
                let keeper_state = keeper_state.clone();
                async move { run_keeper_loop(keeper_state).await }
            },
        ));
    } else {
        tracing::info!("Keeper background loop disabled (KEEPER_ENABLED=false)");
    }

    // Spawn the batch-reconciliation loop: periodically resolves
    // transactions stuck 'pending'/'submitted_unconfirmed' by looking their
    // precomputed hash up directly on Horizon (#30) — recovers from a crash
    // between Horizon accepting a submission and our own status UPDATE
    // committing, and from an ambiguous client-side submission timeout.
    // Unlike the keeper loop this needs no optional credentials, so it
    // always runs.
    {
        let reconciliation_state = state.clone();
        tokio::spawn(supervise_loop(
            "batch-reconciliation",
            LOOP_BASE_BACKOFF,
            LOOP_MAX_BACKOFF,
            LOOP_BACKOFF_RESET_AFTER,
            move || {
                let reconciliation_state = reconciliation_state.clone();
                async move { run_batch_reconciliation_loop(reconciliation_state).await }
            },
        ));
    }

    // Build CORS layer from config.
    let cors = middleware::cors::build_cors_layer(&config.allowed_origins);

    // Build the full router.
    let app = routes::build_router(state)
        // Request-id propagation (X-Request-Id).
        .layer(SetRequestIdLayer::x_request_id(MakeRequestUuid))
        .layer(PropagateRequestIdLayer::x_request_id())
        // Structured request/response logging via Tower Trace.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(
                    DefaultMakeSpan::new()
                        .level(Level::INFO)
                        .include_headers(false),
                )
                .on_response(
                    DefaultOnResponse::new()
                        .level(Level::INFO)
                        .include_headers(false),
                ),
        )
        // Reject requests that take longer than 30 s.
        .layer(TimeoutLayer::new(std::time::Duration::from_secs(30)))
        // CORS.
        .layer(cors);

    let addr: SocketAddr = format!("{}:{}", config.host, config.port)
        .parse()
        .expect("Invalid bind address");

    tracing::info!(address = %addr, "Listening");

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}

/// Runs `pass` — a single unit of background-loop work — isolated inside its
/// own tokio task, so a panic anywhere in `pass` (an `unwrap()`, an
/// out-of-bounds index, a `chrono::Duration` overflow — anything) cannot
/// bring down the caller's loop. Tokio already isolates a panicking task
/// from the rest of the runtime; this just makes that isolation boundary
/// explicit and gives the caller a `JoinError` to log through the
/// structured `tracing` pipeline instead of losing the loop silently (#50).
async fn run_isolated_pass<F, T>(pass: F) -> Result<T, tokio::task::JoinError>
where
    F: std::future::Future<Output = T> + Send + 'static,
    T: Send + 'static,
{
    tokio::spawn(pass).await
}

/// Supervises a background loop task, restarting it with a capped
/// exponential backoff if it ever terminates — whether by returning (which
/// an infinite `loop {}` body should never do on its own) or by panicking
/// outside whatever inner panic isolation it uses (#50). This is the outer,
/// defense-in-depth layer; `run_isolated_pass` is the inner one that keeps
/// most panics from ever reaching this far.
///
/// `spawn_task` is called once per (re)start to produce the future to run —
/// a closure rather than a single future, since a consumed future can't be
/// re-awaited after it resolves.
///
/// Backoff resets to `base_backoff` whenever a run lasts at least
/// `backoff_reset_after`, so a single transient failure long in the past
/// doesn't leave later, unrelated failures waiting out a maxed-out delay —
/// only a *persistently* failing task should ever see delays climb toward
/// `max_backoff`, per the issue's "don't spin the CPU" concern.
async fn supervise_loop<F, Fut>(
    name: &'static str,
    base_backoff: std::time::Duration,
    max_backoff: std::time::Duration,
    backoff_reset_after: std::time::Duration,
    mut spawn_task: F,
) where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let mut backoff = base_backoff;

    loop {
        let started_at = std::time::Instant::now();
        let handle = tokio::spawn(spawn_task());

        match handle.await {
            Ok(()) => {
                tracing::error!(
                    loop_name = name,
                    "Background loop exited (should run forever) — restarting"
                );
            }
            Err(join_err) => {
                tracing::error!(
                    loop_name = name,
                    error = %join_err,
                    panicked = join_err.is_panic(),
                    "Background loop task terminated unexpectedly — restarting"
                );
            }
        }

        if started_at.elapsed() >= backoff_reset_after {
            backoff = base_backoff;
        }

        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(max_backoff);
    }
}

/// Background keeper loop: every `keeper_poll_interval_secs`, look for
/// subscriptions due for execution and submit the on-chain call for each.
/// Errors (including "keeper not configured") are logged and the loop keeps
/// running — a transient RPC/Horizon outage should not crash the server.
async fn run_keeper_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
        state.config.keeper_poll_interval_secs.max(5),
    ));

    loop {
        interval.tick().await;

        let pass_state = state.clone();
        let pass_result = run_isolated_pass(async move {
            use services::{
                soroban::SorobanService, stellar::StellarService,
                subscription::SubscriptionService, transaction::TransactionService,
            };

            let sub_svc = SubscriptionService::new(pass_state.pool.clone());
            let stellar = StellarService::new(&pass_state.config.horizon_url);
            let soroban = SorobanService::new(&pass_state.config.soroban_rpc_url);
            let tx_svc = TransactionService::new(pass_state.pool.clone());

            sub_svc
                .run_due_executions(&pass_state.config, &stellar, &soroban, &tx_svc)
                .await
        })
        .await;

        // Record the tick regardless of outcome — a caught panic still
        // means the *loop* is alive, which is exactly what liveness proves.
        state.loop_health.mark_keeper_ticked();

        match pass_result {
            Ok(Ok(summary)) => {
                if summary.considered > 0 {
                    tracing::info!(
                        executed = summary.executed,
                        failed = summary.failed,
                        considered = summary.considered,
                        "Keeper pass complete"
                    );
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "Keeper pass failed");
            }
            Err(join_err) => {
                // A panic anywhere inside the pass — logged through the same
                // structured tracing pipeline as every other error, instead
                // of a bare stderr backtrace outside the log stream (#50).
                tracing::error!(
                    error = %join_err,
                    panicked = join_err.is_panic(),
                    "Keeper pass panicked — loop continues to next tick"
                );
            }
        }
    }
}

/// Background reconciliation loop: every `reconciliation_poll_interval_secs`,
/// resolve transactions stuck 'pending'/'submitted_unconfirmed' for longer
/// than `reconciliation_stale_after_secs` by checking Horizon directly (#30).
async fn run_batch_reconciliation_loop(state: Arc<AppState>) {
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(
        state.config.reconciliation_poll_interval_secs.max(5),
    ));

    loop {
        interval.tick().await;

        let pass_state = state.clone();
        let pass_result = run_isolated_pass(async move {
            use services::{reconciliation::ReconciliationService, stellar::StellarService};

            let reconciliation_svc = ReconciliationService::new(pass_state.pool.clone());
            let stellar = StellarService::new(&pass_state.config.horizon_url);

            // chrono::Duration::seconds panics on overflow near i64::MAX —
            // exactly the kind of panic this isolation exists to survive.
            reconciliation_svc
                .reconcile_stuck_transactions(
                    &stellar,
                    chrono::Duration::seconds(pass_state.config.reconciliation_stale_after_secs),
                )
                .await
        })
        .await;

        state.loop_health.mark_reconciliation_ticked();

        match pass_result {
            Ok(Ok(summary)) => {
                if summary.considered > 0 {
                    tracing::info!(
                        considered = summary.considered,
                        resolved_completed = summary.resolved_completed,
                        resolved_failed = summary.resolved_failed,
                        still_unconfirmed = summary.still_unconfirmed,
                        "Batch reconciliation pass complete"
                    );
                }
            }
            Ok(Err(e)) => {
                tracing::warn!(error = %e, "Batch reconciliation pass failed");
            }
            Err(join_err) => {
                tracing::error!(
                    error = %join_err,
                    panicked = join_err.is_panic(),
                    "Batch reconciliation pass panicked — loop continues to next tick"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── run_isolated_pass (#50) ─────────────────────────────────────────────

    #[tokio::test]
    async fn isolated_pass_success_returns_ok() {
        let result = run_isolated_pass(async { 42 }).await;
        assert_eq!(result.unwrap(), 42);
    }

    #[tokio::test]
    async fn isolated_pass_panic_is_caught_as_a_join_error_not_propagated() {
        // The panic must surface as a value here, not unwind through this
        // test — proving the caller's own task survives a panicking pass.
        let result = run_isolated_pass(async {
            panic!("simulated panic inside a background-loop pass");
        })
        .await;

        let join_err = result.expect_err("a panicking pass must return Err, not panic the caller");
        assert!(
            join_err.is_panic(),
            "JoinError must report the task panicked"
        );
    }

    // ── supervise_loop (#50) ─────────────────────────────────────────────────

    #[tokio::test]
    async fn supervise_loop_restarts_after_a_panicking_task() {
        use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

        let run_count = Arc::new(AtomicU32::new(0));
        let already_panicked = Arc::new(AtomicBool::new(false));

        let run_count_for_closure = run_count.clone();
        let already_panicked_for_closure = already_panicked.clone();

        let supervisor = tokio::spawn(supervise_loop(
            "test-loop",
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(60), // never resets backoff mid-test
            move || {
                let run_count = run_count_for_closure.clone();
                let already_panicked = already_panicked_for_closure.clone();
                async move {
                    run_count.fetch_add(1, Ordering::SeqCst);
                    if !already_panicked.swap(true, Ordering::SeqCst) {
                        panic!("simulated panic on first run");
                    }
                    // Subsequent restarts behave like a real infinite loop.
                    std::future::pending::<()>().await;
                }
            },
        ));

        // Give the supervisor time to: run once (panics), sleep out the
        // (tiny, test-only) backoff, and restart.
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        supervisor.abort();

        assert!(
            run_count.load(Ordering::SeqCst) >= 2,
            "supervise_loop must restart the task after it panics, not leave it dead"
        );
    }

    #[tokio::test]
    async fn supervise_loop_restarts_after_a_task_that_returns_normally() {
        use std::sync::atomic::{AtomicU32, Ordering};

        let run_count = Arc::new(AtomicU32::new(0));
        let run_count_for_closure = run_count.clone();

        let supervisor = tokio::spawn(supervise_loop(
            "test-loop-returns",
            std::time::Duration::from_millis(1),
            std::time::Duration::from_millis(5),
            std::time::Duration::from_secs(60),
            move || {
                let run_count = run_count_for_closure.clone();
                async move {
                    // A well-behaved infinite loop should never reach here,
                    // but supervise_loop must still restart if it somehow
                    // does — not treat "returned Ok" as "done supervising".
                    run_count.fetch_add(1, Ordering::SeqCst);
                }
            },
        ));

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        supervisor.abort();

        assert!(
            run_count.load(Ordering::SeqCst) >= 2,
            "supervise_loop must keep restarting a task that returns instead of running forever"
        );
    }

    // ── end-to-end: a keeper-loop-shaped task survives a panic (#50) ────────

    #[tokio::test]
    async fn a_keeper_loop_style_task_survives_a_panicking_pass_and_keeps_ticking() {
        // Mirrors run_keeper_loop's/run_batch_reconciliation_loop's actual
        // shape — interval.tick() → an isolated pass → match on the result
        // — without needing a live Postgres/Horizon connection, proving the
        // *structure itself* (not just run_isolated_pass in isolation)
        // survives a panic and keeps ticking, per this issue's acceptance
        // criteria.
        use std::sync::atomic::{AtomicU32, Ordering};

        let tick_count = Arc::new(AtomicU32::new(0));
        let tick_count_for_loop = tick_count.clone();

        let loop_task = tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_millis(5));
            for _ in 0..5 {
                interval.tick().await;
                let n = tick_count_for_loop.fetch_add(1, Ordering::SeqCst);

                let pass_result = run_isolated_pass(async move {
                    if n == 1 {
                        panic!("simulated panic on the second pass");
                    }
                })
                .await;

                if let Err(join_err) = pass_result {
                    // In the real loops this becomes a tracing::error! and
                    // the loop simply proceeds to its next tick, exactly as
                    // it does here.
                    assert!(join_err.is_panic());
                }
            }
        });

        loop_task
            .await
            .expect("the outer loop task itself must never panic");

        assert_eq!(
            tick_count.load(Ordering::SeqCst),
            5,
            "all 5 ticks must run even though the second pass panicked"
        );
    }
}
