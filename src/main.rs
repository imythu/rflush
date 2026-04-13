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
mod brush;
mod stats;
mod web;

use error::AppError;

#[tokio::main]
async fn main() {
    if let Err(error) = bootstrap_and_run().await {
        eprintln!("error: {error}");
        std::process::exit(1);
    }
}

async fn bootstrap_and_run() -> Result<(), AppError> {
    let cwd = std::env::current_dir().map_err(|source| AppError::CreateDir {
        path: ".".to_string(),
        source,
    })?;
    let db = db::Database::open(&cwd).await?;
    let settings = db.get_settings().await?;
    let log_filter = logging::build_log_filter(settings.log_level.as_deref())?;
    logging::init_logging(log_filter);

    // 启动刷流调度器
    let scheduler = std::sync::Arc::new(brush::scheduler::BrushScheduler::new(db.clone()));
    let scheduler_ref = scheduler.clone();
    tokio::spawn(async move {
        scheduler_ref.start().await;
    });

    web::serve(cwd, db, scheduler).await
}
