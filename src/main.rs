mod brush;
mod cli;
mod collector;
mod config;
mod db;
mod download;
mod downloader;
mod engine;
mod error;
mod history;
mod logging;
mod net;
mod rss;
mod sign_in;
mod site;
mod site_stats;
mod stats;
mod web;

use std::sync::Arc;
use std::time::Duration;

use clap::Parser;
use error::AppError;
use net::http::AppHttpClient;
use net::rate_limiter::{RateLimitPolicy, SharedRateLimiter};
use tracing::info;

#[tokio::main]
async fn main() {
    if let Err(error) = bootstrap_and_run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn bootstrap_and_run() -> Result<(), AppError> {
    let cli = cli::Cli::parse();
    let cwd = std::env::current_dir().map_err(|source| AppError::CreateDir {
        path: ".".to_string(),
        source,
    })?;
    let (base_dir, db_dir) = cli.resolve_paths(&cwd);
    let listen_addr = cli.resolve_listen_addr()?;
    let db = db::Database::open(&db_dir).await?;
    let settings = db.get_settings().await?;
    let log_filter = logging::build_log_filter(settings.log_level.as_deref())?;
    logging::init_logging(log_filter);
    info!(
        "startup configuration: listen_addr={} data_dir={} database_dir={}",
        listen_addr,
        base_dir.display(),
        db_dir.display()
    );

    let pool = downloader::DownloaderClientPool::new(db.clone());

    let collector = std::sync::Arc::new(collector::DownloaderSnapshotCollector::new(db.clone(), pool.clone()));
    let collector_ref = collector.clone();
    let collector_handle = tokio::spawn(async move {
        collector_ref.start().await;
    });

    let stats_db = db.clone();
    let stats_rx = collector.subscribe();
    let stats_handle = tokio::spawn(async move {
        stats::start_stats_consumer(stats_db, stats_rx).await;
    });

    // 构建共享 HTTP 客户端（代理 + 限流），供刷流调度器使用
    let proxy = settings.proxy.as_deref();
    let limiter = Arc::new(SharedRateLimiter::new());
    let policy = RateLimitPolicy::new(5, Duration::from_secs(1), Duration::from_secs(60));
    let http = Arc::new(
        AppHttpClient::new(limiter, policy, proxy).map_err(|e| AppError::InvalidConfig {
            message: format!("failed to build HTTP client: {}", e),
        })?,
    );

    // 启动刷流调度器
    let scheduler = std::sync::Arc::new(brush::scheduler::BrushScheduler::new(
        db.clone(),
        collector.clone(),
        pool.clone(),
        http,
    ));
    let scheduler_ref = scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        scheduler_ref.start().await;
    });

    let sign_in_scheduler = std::sync::Arc::new(sign_in::scheduler::SignInScheduler::new(
        db.clone(),
        base_dir.clone(),
    ));
    let sign_in_scheduler_ref = sign_in_scheduler.clone();
    let sign_in_scheduler_handle = tokio::spawn(async move {
        sign_in_scheduler_ref.start().await;
    });

    let site_stats_refresher = std::sync::Arc::new(site_stats::SiteStatsRefresher::new(db.clone()));
    let site_stats_refresher_ref = site_stats_refresher.clone();
    let site_stats_handle = tokio::spawn(async move {
        site_stats_refresher_ref.start().await;
    });

    let web_result = web::serve(
        base_dir,
        listen_addr,
        db,
        scheduler,
        sign_in_scheduler,
        site_stats_refresher,
        collector,
        pool,
    )
    .await;

    collector_handle.abort();
    stats_handle.abort();
    scheduler_handle.abort();
    sign_in_scheduler_handle.abort();
    site_stats_handle.abort();

    let _ = collector_handle.await;
    let _ = stats_handle.await;
    let _ = scheduler_handle.await;
    let _ = sign_in_scheduler_handle.await;
    let _ = site_stats_handle.await;

    web_result
}
