pub mod naming;

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use tokio::fs;
use tokio::time::sleep;
use tracing::{info, warn};

use crate::engine::{AppRuntime, RssRuntime};
use crate::history::{FinalStatus, TorrentRunRecord};
use crate::logging::current_task_context;
use crate::net::http::{HttpError, is_expired_response, parse_api_error_response};
use crate::rss::TorrentItem;
use crate::rss::feed::refresh_download_url;
use naming::{build_target_file_name, extract_original_filename};

#[derive(Debug)]
enum DownloadAttemptError {
    ExpiredLink(String),
    RateLimited(String),
    Retriable(String),
    Fatal(String),
}

pub async fn download_torrent(
    runtime: Arc<RssRuntime>,
    item: TorrentItem,
    app_runtime: AppRuntime,
) -> TorrentRunRecord {
    let mut record = TorrentRunRecord::new(
        runtime.config.name.clone(),
        item.guid.clone(),
        item.title.clone(),
    );

    info!(
        task = %current_task_context(),
        "[RSS下载][{}] 开始下载: guid={} title={}",
        runtime.config.name, item.guid, item.title
    );

    let mut current_url = item.download_url.clone();
    let mut current_version = item.version;

    let mut attempt = 1u32;
    loop {
        if app_runtime.shutdown.load(Ordering::SeqCst) {
            record.retry_count = attempt.saturating_sub(1);
            record.final_status = FinalStatus::Failed;
            record.final_message = Some("cancelled by shutdown signal".to_string());
            warn!(
                task = %current_task_context(),
                "[RSS下载][{}] 下载取消(shutdown): guid={}",
                runtime.config.name, item.guid
            );
            return record;
        }

        if attempt > 1 {
            info!(
                task = %current_task_context(),
                "[RSS下载][{}] 重试第 {} 次: guid={}",
                runtime.config.name, attempt, item.guid
            );
        }

        match try_download_once(&runtime, &item, &current_url, &app_runtime).await {
            Ok(success) => {
                record.retry_count = attempt.saturating_sub(1);
                record.saved_path = Some(success.saved_path.display().to_string());
                record.file_name = Some(success.file_name.clone());
                record.final_status = if success.skipped {
                    FinalStatus::SkippedExisting
                } else {
                    FinalStatus::Success
                };
                record.bytes = Some(success.bytes as u64);
                if success.skipped {
                    info!(
                        task = %current_task_context(),
                        "[RSS下载][{}] 跳过(文件已存在): guid={} file={}",
                        runtime.config.name, item.guid, success.file_name
                    );
                } else {
                    info!(
                        task = %current_task_context(),
                        "[RSS下载][{}] ✓ 下载成功: guid={} file={} size={}B 重试{}次",
                        runtime.config.name, item.guid, success.file_name,
                        success.bytes, attempt.saturating_sub(1)
                    );
                }
                return record;
            }
            Err(DownloadAttemptError::ExpiredLink(_message)) => {
                info!(
                    task = %current_task_context(),
                    "[RSS下载][{}] 链接已过期，等待刷新 RSS: guid={}",
                    runtime.config.name, item.guid
                );
                // Keep retrying the refresh until it succeeds — avoids wasting
                // rate-limiter slots on downloads with a known-expired URL.
                loop {
                    if app_runtime.shutdown.load(Ordering::SeqCst) {
                        record.retry_count = attempt.saturating_sub(1);
                        record.final_status = FinalStatus::Failed;
                        record.final_message = Some("cancelled by shutdown signal".to_string());
                        return record;
                    }
                    match refresh_download_url(&runtime, &item.guid, current_version, &app_runtime)
                        .await
                    {
                        Ok((latest_url, latest_version, refreshed)) => {
                            if refreshed {
                                record.refresh_count += 1;
                                info!(
                                    task = %current_task_context(),
                                    "[RSS下载][{}] 链接刷新成功 (第{}次): guid={}",
                                    runtime.config.name, record.refresh_count, item.guid
                                );
                            }
                            current_url = latest_url;
                            current_version = latest_version;
                            break;
                        }
                        Err(error) => {
                            warn!(
                                task = %current_task_context(),
                                "[RSS下载][{}] 刷新链接失败，等待重试: guid={} err={}",
                                runtime.config.name, item.guid, error
                            );
                            record.final_message = Some(error.to_string());
                            sleep(Duration::from_secs(app_runtime.global.retry_interval_secs))
                                .await;
                        }
                    }
                }

                sleep(Duration::from_secs(app_runtime.global.retry_interval_secs)).await;
                attempt = attempt.saturating_add(1);
            }
            Err(DownloadAttemptError::RateLimited(host)) => {
                // throttle() already called by AppHttpClient — next acquire()
                // will wait for the throttle to expire. No extra sleep needed.
                warn!(
                    task = %current_task_context(),
                    "[RSS下载][{}] 触发限速，等待: host={} guid={}",
                    runtime.config.name, host, item.guid
                );
                record.final_message = Some(format!("host={} throttled", host));
                attempt = attempt.saturating_add(1);
            }
            Err(DownloadAttemptError::Retriable(message)) => {
                warn!(
                    task = %current_task_context(),
                    "[RSS下载][{}] 可重试错误，等待重试: guid={} err={}",
                    runtime.config.name, item.guid, message
                );
                record.final_message = Some(message.clone());
                sleep(Duration::from_secs(app_runtime.global.retry_interval_secs)).await;
                attempt = attempt.saturating_add(1);
            }
            Err(DownloadAttemptError::Fatal(message)) => {
                record.retry_count = attempt.saturating_sub(1);
                record.final_status = FinalStatus::Failed;
                record.final_message = Some(message.clone());
                warn!(
                    task = %current_task_context(),
                    "[RSS下载][{}] ✗ 下载失败(不可重试): guid={} err={}",
                    runtime.config.name, item.guid, message
                );
                return record;
            }
        }
    }
}

struct DownloadSuccess {
    file_name: String,
    saved_path: PathBuf,
    bytes: usize,
    skipped: bool,
}

async fn try_download_once(
    runtime: &RssRuntime,
    item: &TorrentItem,
    download_url: &str,
    app_runtime: &AppRuntime,
) -> Result<DownloadSuccess, DownloadAttemptError> {
    let purpose = format!(
        "download_torrent rss={} guid={}",
        runtime.config.name, item.guid
    );

    let response = app_runtime
        .http
        .get(&purpose, download_url)
        .await
        .map_err(|e| match e {
            HttpError::RateLimited { key } => DownloadAttemptError::RateLimited(key),
            HttpError::InvalidUrl(msg) => DownloadAttemptError::Fatal(msg),
            HttpError::Transport { source } => DownloadAttemptError::Retriable(source.to_string()),
        })?;

    if !response.status.is_success() {
        if let Some(detail) = parse_api_error_response(&response.body) {
            return Err(DownloadAttemptError::Fatal(detail));
        }
        return Err(DownloadAttemptError::Retriable(format!(
            "HTTP status {}",
            response.status
        )));
    }

    if is_expired_response(&response.body) {
        warn!(
            task = %current_task_context(),
            "expired download URL: purpose=\"{}\" url={}",
            purpose, download_url
        );
        return Err(DownloadAttemptError::ExpiredLink(
            "download URL expired, RSS refresh required".to_string(),
        ));
    }

    if let Some(detail) = parse_api_error_response(&response.body) {
        warn!(
            task = %current_task_context(),
            "API error: purpose=\"{}\" url={} detail={}",
            purpose, download_url, detail
        );
        return Err(DownloadAttemptError::Fatal(detail));
    }

    if response.body.is_empty() {
        return Err(DownloadAttemptError::Retriable(
            "download response body is empty".to_string(),
        ));
    }

    let original_name = extract_original_filename(&response.headers);
    let file_name = build_target_file_name(original_name.as_deref(), item);
    let output_path = runtime.output_dir.join(&file_name);

    if fs::try_exists(&output_path)
        .await
        .map_err(|error| DownloadAttemptError::Fatal(error.to_string()))?
    {
        return Ok(DownloadSuccess {
            file_name,
            saved_path: output_path,
            bytes: response.body.len(),
            skipped: true,
        });
    }

    // Atomic write: write to .tmp then rename to prevent partial files.
    let tmp_path = output_path.with_extension("torrent.tmp");
    fs::write(&tmp_path, &response.body)
        .await
        .map_err(|error| DownloadAttemptError::Fatal(error.to_string()))?;
    fs::rename(&tmp_path, &output_path)
        .await
        .map_err(|error| DownloadAttemptError::Fatal(error.to_string()))?;

    Ok(DownloadSuccess {
        file_name,
        saved_path: output_path,
        bytes: response.body.len(),
        skipped: false,
    })
}
