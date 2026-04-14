use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use futures::stream::{self, StreamExt};
use tokio::fs;
use tokio::sync::{Mutex, RwLock};
use tokio::time::sleep;
use tracing::{info, warn};

use crate::config::{AppConfig, RssConfig};
use crate::download::naming::sanitize_component;
use crate::engine::{AppRuntime, RssRuntime};
use crate::error::AppError;
use crate::logging::{TASK_LOG_CONTEXT, current_task_context, next_async_task_id};
use crate::net::http::HttpError;
use crate::rss::FeedSnapshot;

pub async fn fetch_all_rss(
    config: &AppConfig,
    cwd: &Path,
    app_runtime: &AppRuntime,
) -> Result<(Vec<Arc<RssRuntime>>, Vec<crate::history::RssRunSummary>), AppError> {
    let concurrency = config.global.max_concurrent_rss_fetches;
    let outcomes = stream::iter(config.rss.iter().cloned())
        .map(|rss_config| {
            let cwd = cwd.to_path_buf();
            let app_runtime = app_runtime.clone();
            let task_context = format!(
                "rss_fetch_task#{} rss={}",
                next_async_task_id(),
                rss_config.name
            );
            TASK_LOG_CONTEXT.scope(task_context, async move {
                let output_dir = cwd.join(sanitize_component(&rss_config.name));
                fs::create_dir_all(&output_dir)
                    .await
                    .map_err(|source| AppError::CreateDir {
                        path: output_dir.display().to_string(),
                        source,
                    })?;

                let (snapshot, fetch_attempts) =
                    fetch_feed_snapshot_until_success(&rss_config, 1, &app_runtime).await;

                Ok(Arc::new(RssRuntime {
                    config: rss_config,
                    output_dir,
                    snapshot: Arc::new(RwLock::new(snapshot)),
                    refresh_lock: Arc::new(Mutex::new(())),
                    initial_fetch_attempts: fetch_attempts,
                }))
            })
        })
        .buffer_unordered(concurrency)
        .collect::<Vec<Result<Arc<RssRuntime>, AppError>>>()
        .await;

    let mut runtimes = Vec::new();
    for outcome in outcomes {
        runtimes.push(outcome?);
    }

    Ok((runtimes, Vec::new()))
}

pub async fn fetch_feed_snapshot_once(
    rss_config: &RssConfig,
    version: u64,
    app_runtime: &AppRuntime,
) -> Result<FeedSnapshot, AppError> {
    let purpose = if version == 1 {
        "fetch_rss_initial"
    } else {
        "fetch_rss_refresh"
    };

    let response = app_runtime
        .http
        .get(purpose, &rss_config.url)
        .await
        .map_err(|e| match e {
            HttpError::RateLimited { .. } => AppError::RateLimited {
                name: rss_config.name.clone(),
            },
            HttpError::InvalidUrl(msg) => AppError::InvalidConfig { message: msg },
            HttpError::Transport { source } => AppError::FetchRss {
                name: rss_config.name.clone(),
                source,
            },
        })?;

    let xml = std::str::from_utf8(&response.body).map_err(|_| AppError::ParseRss {
        name: rss_config.name.clone(),
        source: crate::rss::RssParseError::UnexpectedEof,
    })?;

    let parsed = crate::rss::parse_feed(xml).map_err(|source| AppError::ParseRss {
        name: rss_config.name.clone(),
        source,
    })?;

    Ok(parsed.into_snapshot(rss_config.name.clone(), version))
}

async fn fetch_feed_snapshot_until_success(
    rss_config: &RssConfig,
    version: u64,
    app_runtime: &AppRuntime,
) -> (FeedSnapshot, u32) {
    let mut attempt = 1u32;
    loop {
        match fetch_feed_snapshot_once(rss_config, version, app_runtime).await {
            Ok(snapshot) => {
                if attempt > 1 {
                    info!(
                        task = %current_task_context(),
                        "rss fetch recovered: rss={} attempt={}",
                        rss_config.name, attempt
                    );
                }
                return (snapshot, attempt);
            }
            Err(error) => {
                warn!(
                    task = %current_task_context(),
                    "rss fetch failed: rss={} attempt={} retry_in_secs={} detail={}",
                    rss_config.name, attempt, app_runtime.global.retry_interval_secs, error
                );
                sleep(Duration::from_secs(app_runtime.global.retry_interval_secs)).await;
                attempt = attempt.saturating_add(1);
            }
        }
    }
}

pub async fn refresh_download_url(
    runtime: &RssRuntime,
    guid: &str,
    seen_version: u64,
    app_runtime: &AppRuntime,
) -> Result<(String, u64, bool), AppError> {
    {
        let snapshot = runtime.snapshot.read().await;
        if snapshot.version > seen_version {
            if let Some(item) = snapshot.items.get(guid) {
                return Ok((item.download_url.clone(), snapshot.version, false));
            }
            return Err(AppError::TorrentMissing {
                name: runtime.config.name.clone(),
                guid: guid.to_string(),
            });
        }
    }

    // Single-flight gate: only one task per RSS refreshes at a time.
    let _guard = runtime.refresh_lock.lock().await;

    let snapshot = runtime.snapshot.read().await;
    if snapshot.version > seen_version {
        if let Some(item) = snapshot.items.get(guid) {
            return Ok((item.download_url.clone(), snapshot.version, false));
        }
        return Err(AppError::TorrentMissing {
            name: runtime.config.name.clone(),
            guid: guid.to_string(),
        });
    }
    let next_version = snapshot.version + 1;
    drop(snapshot);

    let refreshed = fetch_feed_snapshot_once(&runtime.config, next_version, app_runtime).await?;
    let refreshed_version = refreshed.version;
    let refreshed_url = refreshed
        .items
        .get(guid)
        .map(|item| item.download_url.clone())
        .ok_or_else(|| AppError::TorrentMissing {
            name: runtime.config.name.clone(),
            guid: guid.to_string(),
        })?;

    let mut snapshot = runtime.snapshot.write().await;
    *snapshot = refreshed;

    Ok((refreshed_url, refreshed_version, true))
}
