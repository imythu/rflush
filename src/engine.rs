use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;
use std::time::Duration;

use chrono::Utc;
use futures::stream::{self, StreamExt};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

use crate::config::{AppConfig, GlobalConfig, RssConfig};
use crate::db::Database;
use crate::download::download_torrent;
use crate::downloader::DownloaderClientPool;
use crate::error::AppError;
use crate::history::{OutputLogger, RssRunSummary, RunHistory, RunSummary, TorrentRunRecord};
use crate::logging::{TASK_LOG_CONTEXT, next_async_task_id};
use crate::net::http::AppHttpClient;
use crate::net::rate_limiter::{RateLimitPolicy, SharedRateLimiter};
use crate::rss::FeedSnapshot;
use crate::rss::feed::fetch_all_rss;

#[derive(Clone)]
pub struct RssRuntime {
    pub config: RssConfig,
    pub snapshot: Arc<RwLock<FeedSnapshot>>,
    pub refresh_lock: Arc<Mutex<()>>,
    pub initial_fetch_attempts: u32,
}

#[derive(Clone)]
pub struct AppRuntime {
    pub global: GlobalConfig,
    pub http: Arc<AppHttpClient>,
    pub shutdown: Arc<AtomicBool>,
    pub pool: Option<Arc<DownloaderClientPool>>,
    pub db: Option<Database>,
}

#[derive(Clone)]
pub struct DownloadEngine {
    limiter: Arc<SharedRateLimiter>,
}

impl DownloadEngine {
    pub fn new(limiter: Arc<SharedRateLimiter>) -> Self {
        Self { limiter }
    }

    pub async fn run_with_shutdown(
        &self,
        config: AppConfig,
        shutdown: Arc<AtomicBool>,
        pool: Option<Arc<DownloaderClientPool>>,
        db: Option<Database>,
    ) -> Result<RunHistory, AppError> {
        let app_runtime = self.build_app_runtime(&config, shutdown, pool, db)?;
        let run_started_at = Utc::now();
        let run_start_instant = std::time::Instant::now();

        info!(
            "[RSS下载] 开始执行，共 {} 个订阅，最大并发 {}",
            config.rss.len(),
            config.global.max_concurrent_downloads
        );

        let (runtimes, mut rss_summaries) = fetch_all_rss(&config, &app_runtime).await?;

        let mut torrent_records = Vec::new();
        let mut download_jobs = Vec::new();
        for runtime in &runtimes {
            let snapshot = runtime.snapshot.read().await;
            let mut items = snapshot.items.values().cloned().collect::<Vec<_>>();
            items.sort_by(|left, right| left.guid.cmp(&right.guid));
            let total_in_feed = items.len();
            drop(snapshot);

            for item in items {
                download_jobs.push((runtime.clone(), item));
            }
            info!(
                "[RSS下载][{}] Feed 解析完成: 共 {} 条",
                runtime.config.name, total_in_feed
            );
        }

        info!("[RSS下载] 开始下载，共 {} 个任务排队", download_jobs.len());

        let concurrency = config.global.max_concurrent_downloads;
        let mut downloaded_records = stream::iter(download_jobs.into_iter())
            .map(|(runtime, item)| {
                let app_runtime = app_runtime.clone();
                let task_context = format!(
                    "download_task#{} rss={} guid={}",
                    next_async_task_id(),
                    runtime.config.name,
                    item.guid
                );
                TASK_LOG_CONTEXT.scope(task_context, async move {
                    download_torrent(runtime, item, app_runtime).await
                })
            })
            .buffer_unordered(concurrency)
            .collect::<Vec<_>>()
            .await;
        torrent_records.append(&mut downloaded_records);

        let succeeded = torrent_records
            .iter()
            .filter(|r| matches!(r.final_status, crate::history::FinalStatus::Success))
            .count();
        let failed = torrent_records
            .iter()
            .filter(|r| matches!(r.final_status, crate::history::FinalStatus::Failed))
            .count();
        let skipped_total = torrent_records
            .iter()
            .filter(|r| matches!(r.final_status, crate::history::FinalStatus::SkippedExisting))
            .count();
        let elapsed = run_start_instant.elapsed();
        info!(
            "[RSS下载] 执行完成: 成功 {}, 失败 {}, 跳过 {}, 耗时 {:.1}s",
            succeeded,
            failed,
            skipped_total,
            elapsed.as_secs_f64()
        );
        if failed > 0 {
            for r in torrent_records
                .iter()
                .filter(|r| matches!(r.final_status, crate::history::FinalStatus::Failed))
            {
                warn!(
                    "[RSS下载][{}] 失败: guid={} msg={}",
                    r.rss_name,
                    r.guid,
                    r.final_message.as_deref().unwrap_or("-")
                );
            }
        }

        rss_summaries.extend(build_rss_summaries(&runtimes, &torrent_records).await);
        let history = RunHistory {
            started_at: run_started_at.to_rfc3339(),
            finished_at: Utc::now().to_rfc3339(),
            retry_delay_secs: app_runtime.global.retry_interval_secs,
            summary: RunSummary::from_records(&torrent_records),
            rss: rss_summaries,
            torrents: torrent_records,
        };
        OutputLogger::log(&history);
        Ok(history)
    }

    fn build_app_runtime(
        &self,
        config: &AppConfig,
        shutdown: Arc<AtomicBool>,
        pool: Option<Arc<DownloaderClientPool>>,
        db: Option<Database>,
    ) -> Result<AppRuntime, AppError> {
        validate_config(config)?;
        let policy = RateLimitPolicy::new(
            config.global.download_rate_limit.requests,
            config.global.download_rate_limit.interval_duration(),
            Duration::from_secs(config.global.throttle_interval_secs),
        );

        let http = Arc::new(
            AppHttpClient::new(self.limiter.clone(), policy, config.global.proxy.as_deref())
                .map_err(|e| AppError::InvalidConfig {
                    message: format!("failed to build HTTP client: {}", e),
                })?,
        );

        Ok(AppRuntime {
            global: config.global.clone(),
            http,
            shutdown,
            pool,
            db,
        })
    }
}

fn validate_config(config: &AppConfig) -> Result<(), AppError> {
    if config.global.download_rate_limit.requests == 0 {
        return Err(AppError::InvalidConfig {
            message: "global.download_rate_limit.requests must be >= 1".to_string(),
        });
    }
    if config.global.download_rate_limit.interval == 0 {
        return Err(AppError::InvalidConfig {
            message: "global.download_rate_limit.interval must be >= 1".to_string(),
        });
    }
    if config.global.retry_interval_secs == 0 {
        return Err(AppError::InvalidConfig {
            message: "global.retry_interval_secs must be >= 1".to_string(),
        });
    }
    if let Some(proxy) = config.global.proxy.as_deref() {
        validate_proxy(proxy).map_err(|message| AppError::InvalidConfig {
            message: format!("global.proxy {}", message),
        })?;
    }

    let mut seen_names = HashSet::new();
    for rss in &config.rss {
        if !seen_names.insert(&rss.name) {
            return Err(AppError::InvalidConfig {
                message: format!("duplicate RSS name: `{}`", rss.name),
            });
        }
    }
    Ok(())
}

fn validate_proxy(proxy: &str) -> Result<(), &'static str> {
    let proxy = proxy.trim();
    if proxy.is_empty() {
        return Ok(());
    }
    if proxy.starts_with("http://")
        || proxy.starts_with("https://")
        || proxy.starts_with("socks5://")
        || proxy.starts_with("socks5h://")
    {
        return Ok(());
    }
    Err("must start with http://, https://, socks5://, or socks5h://")
}

async fn build_rss_summaries(
    runtimes: &[Arc<RssRuntime>],
    torrent_records: &[TorrentRunRecord],
) -> Vec<RssRunSummary> {
    let mut records_by_rss: HashMap<&str, Vec<&TorrentRunRecord>> = HashMap::new();
    for record in torrent_records {
        records_by_rss
            .entry(record.rss_name.as_str())
            .or_default()
            .push(record);
    }

    let mut summaries = Vec::with_capacity(runtimes.len());
    for runtime in runtimes {
        let snapshot = runtime.snapshot.read().await;
        let records = records_by_rss
            .get(runtime.config.name.as_str())
            .cloned()
            .unwrap_or_default();
        summaries.push(RssRunSummary::from_records(
            runtime.config.name.clone(),
            runtime.config.url.clone(),
            runtime.initial_fetch_attempts,
            snapshot.version,
            snapshot.items.len(),
            &records,
        ));
    }
    summaries
}
