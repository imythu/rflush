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
mod site;
mod stats;
mod web;

use clap::Parser;
use error::AppError;
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

    let collector = std::sync::Arc::new(collector::DownloaderSnapshotCollector::new(db.clone()));
    let collector_ref = collector.clone();
    let collector_handle = tokio::spawn(async move {
        collector_ref.start().await;
    });

    let stats_db = db.clone();
    let stats_rx = collector.subscribe();
    let stats_handle = tokio::spawn(async move {
        stats::start_stats_consumer(stats_db, stats_rx).await;
    });

    // 启动刷流调度器
    let scheduler = std::sync::Arc::new(brush::scheduler::BrushScheduler::new(
        db.clone(),
        collector.clone(),
    ));
    let scheduler_ref = scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        scheduler_ref.start().await;
    });

    let web_result = web::serve(base_dir, listen_addr, db, scheduler, collector).await;

    collector_handle.abort();
    stats_handle.abort();
    scheduler_handle.abort();

    let _ = collector_handle.await;
    let _ = stats_handle.await;
    let _ = scheduler_handle.await;

    web_result
}
