mod brush;
mod cli;
mod collector;
mod config;
mod db;
mod downloader;
mod error;
mod indexer;
mod logging;
mod media;
mod monitor;
mod net;
mod openlist;
mod ptd_backup;
mod ptd_site_catalog;
mod ptd_sites;
mod relocation;
mod rss;
mod sign_in;
mod site;
mod site_stats;
mod stats;
mod tag_rule;
mod torrent_watcher;
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
    let listener = tokio::net::TcpListener::bind(listen_addr)
        .await
        .map_err(|error| AppError::Server {
            message: format!("failed to bind {listen_addr}: {error}"),
        })?;
    let db = db::Database::open(&db_dir).await?;
    let self_use = self_use_enabled(std::env::var("SELF_USE").ok().as_deref());
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
    let media_service = media::service::MediaService::new(db.clone(), pool.clone());
    let media_scheduler = media::scheduler::MediaScheduler::new(media_service.clone());

    let collector = std::sync::Arc::new(collector::DownloaderSnapshotCollector::new(
        db.clone(),
        pool.clone(),
    ));
    let stats_db = db.clone();
    let stats_rx = collector.subscribe();

    // 构建共享 HTTP 客户端（代理 + 限流），供刷流调度器使用
    let proxy = settings.proxy.as_deref();
    let limiter = Arc::new(SharedRateLimiter::new());
    let policy = RateLimitPolicy::new(5, Duration::from_secs(1), Duration::from_secs(60));
    let http = Arc::new(
        AppHttpClient::new(limiter.clone(), policy, proxy).map_err(|e| {
            AppError::InvalidConfig {
                message: format!("failed to build HTTP client: {}", e),
            }
        })?,
    );

    let scheduler = std::sync::Arc::new(brush::scheduler::BrushScheduler::new(
        db.clone(),
        collector.clone(),
        pool.clone(),
        http,
    ));

    let sign_in_scheduler = std::sync::Arc::new(sign_in::scheduler::SignInScheduler::new(
        db.clone(),
        base_dir.clone(),
    ));

    let site_stats_refresher = std::sync::Arc::new(site_stats::SiteStatsRefresher::new(db.clone()));

    let monitor = std::sync::Arc::new(monitor::SystemMonitor::new(db.clone()));

    let tag_rule_scheduler = tag_rule::scheduler::TagRuleScheduler::new(db.clone(), pool.clone());
    let new_torrent_publisher = torrent_watcher::NewTorrentPublisher::new(db.clone(), pool.clone());
    let new_torrent_notifications = new_torrent_publisher.subscribe();
    let relocation_scheduler =
        relocation::RelocationScheduler::new(db.clone(), pool.clone(), self_use);

    let media_scheduler_ref = media_scheduler.clone();
    let mut media_scheduler_handle = tokio::spawn(async move {
        media_scheduler_ref.start().await;
    });
    let collector_ref = collector.clone();
    let collector_handle = tokio::spawn(async move {
        collector_ref.start().await;
    });
    let stats_handle = tokio::spawn(async move {
        stats::start_stats_consumer(stats_db, stats_rx).await;
    });
    let scheduler_ref = scheduler.clone();
    let scheduler_handle = tokio::spawn(async move {
        scheduler_ref.start().await;
    });
    let sign_in_scheduler_ref = sign_in_scheduler.clone();
    let sign_in_scheduler_handle = tokio::spawn(async move {
        sign_in_scheduler_ref.start().await;
    });
    let site_stats_refresher_ref = site_stats_refresher.clone();
    let site_stats_handle = tokio::spawn(async move {
        site_stats_refresher_ref.start().await;
    });
    let monitor_ref = monitor.clone();
    let monitor_handle = tokio::spawn(async move {
        monitor_ref.start().await;
    });
    let tag_rule_scheduler_ref = tag_rule_scheduler.clone();
    let tag_rule_scheduler_handle = tokio::spawn(async move {
        tag_rule_scheduler_ref.start().await;
    });
    let tag_rule_subscriber_ref = tag_rule_scheduler.clone();
    let tag_rule_subscriber_handle = tokio::spawn(async move {
        tag_rule_subscriber_ref
            .start_new_torrent_subscriber(new_torrent_notifications)
            .await;
    });
    let new_torrent_publisher_ref = new_torrent_publisher.clone();
    let new_torrent_publisher_handle = tokio::spawn(async move {
        new_torrent_publisher_ref.start().await;
    });
    let relocation_scheduler_ref = relocation_scheduler.clone();
    let relocation_scheduler_handle = tokio::spawn(async move {
        relocation_scheduler_ref.start().await;
    });

    let web_result = web::serve(
        listener,
        db,
        scheduler,
        sign_in_scheduler,
        site_stats_refresher,
        collector,
        pool,
        media_service,
        media_scheduler.clone(),
        monitor,
        tag_rule_scheduler,
        relocation_scheduler.clone(),
        self_use,
    )
    .await;

    media_scheduler.stop();
    relocation_scheduler.stop();
    collector_handle.abort();
    stats_handle.abort();
    scheduler_handle.abort();
    sign_in_scheduler_handle.abort();
    site_stats_handle.abort();
    monitor_handle.abort();
    tag_rule_scheduler_handle.abort();
    tag_rule_subscriber_handle.abort();
    new_torrent_publisher_handle.abort();
    relocation_scheduler_handle.abort();

    if tokio::time::timeout(Duration::from_secs(10), &mut media_scheduler_handle)
        .await
        .is_err()
    {
        media_scheduler_handle.abort();
        let _ = media_scheduler_handle.await;
    }
    match media_scheduler.release_owned_leases().await {
        Ok((0, 0)) => {}
        Ok((subscriptions, downloads)) => info!(
            recovered_subscriptions = subscriptions,
            recovered_downloads = downloads,
            "released interrupted media leases during shutdown"
        ),
        Err(error) => {
            tracing::error!(%error, "failed to release media leases during shutdown")
        }
    }

    let _ = collector_handle.await;
    let _ = stats_handle.await;
    let _ = scheduler_handle.await;
    let _ = sign_in_scheduler_handle.await;
    let _ = site_stats_handle.await;
    let _ = monitor_handle.await;
    let _ = tag_rule_scheduler_handle.await;
    let _ = tag_rule_subscriber_handle.await;
    let _ = new_torrent_publisher_handle.await;
    let _ = relocation_scheduler_handle.await;

    web_result
}

fn self_use_enabled(value: Option<&str>) -> bool {
    value == Some("true")
}

#[cfg(test)]
mod feature_gate_tests {
    use super::self_use_enabled;

    #[test]
    fn self_use_requires_exact_true_literal() {
        assert!(self_use_enabled(Some("true")));
        for value in [None, Some("TRUE"), Some("1"), Some(" true"), Some("")] {
            assert!(!self_use_enabled(value));
        }
    }
}
