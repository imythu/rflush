use std::sync::Arc;

use tokio::sync::broadcast;
use tracing::{debug, error, info};

use crate::collector::DownloaderSnapshot;
use crate::db::Database;

/// 统计快照记录
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TaskStatsSnapshot {
    pub id: i64,
    pub task_id: i64,
    pub total_uploaded: i64,
    pub total_downloaded: i64,
    pub torrent_count: i64,
    pub recorded_at: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct DownloaderSpeedSnapshot {
    pub id: i64,
    pub downloader_id: i64,
    pub upload_speed: i64,
    pub download_speed: i64,
    pub recorded_at: String,
}

pub async fn start_stats_consumer(
    db: Database,
    mut rx: broadcast::Receiver<Arc<DownloaderSnapshot>>,
) {
    info!("stats consumer started");
    loop {
        match rx.recv().await {
            Ok(snapshot) => {
                if let Err(error) = process_snapshot(&db, &snapshot).await {
                    error!(
                        "stats consumer error for downloader {} at {}: {}",
                        snapshot.downloader_id, snapshot.recorded_at, error
                    );
                }
            }
            Err(broadcast::error::RecvError::Lagged(skipped)) => {
                debug!("stats consumer lagged, skipped {} snapshot(s)", skipped);
            }
            Err(broadcast::error::RecvError::Closed) => {
                info!("stats consumer stopped: snapshot publisher closed");
                break;
            }
        }
    }
}

async fn process_snapshot(db: &Database, snapshot: &DownloaderSnapshot) -> Result<(), String> {
    let upload_speed: i64 = snapshot
        .torrents
        .iter()
        .map(|torrent| torrent.upload_speed)
        .sum();
    let download_speed: i64 = snapshot
        .torrents
        .iter()
        .map(|torrent| torrent.download_speed)
        .sum();
    let _ = db
        .save_downloader_speed_snapshot(snapshot.downloader_id, upload_speed, download_speed)
        .await;

    let tasks = db.list_brush_tasks().await.map_err(|e| e.to_string())?;
    for task in tasks
        .into_iter()
        .filter(|task| task.downloader_id == snapshot.downloader_id)
    {
        let torrents: Vec<_> = snapshot
            .torrents
            .iter()
            .filter(|torrent| torrent_has_tag(&torrent.tags, &task.tag))
            .cloned()
            .collect();

        let total_uploaded: i64 = torrents.iter().map(|torrent| torrent.uploaded).sum();
        let total_downloaded: i64 = torrents.iter().map(|torrent| torrent.downloaded).sum();
        let count = torrents.len() as i64;

        let _ = db
            .save_task_stats_snapshot(task.id, total_uploaded, total_downloaded, count)
            .await;

        for torrent in &torrents {
            let _ = db
                .save_torrent_traffic(task.id, &torrent.hash, torrent.uploaded, torrent.downloaded)
                .await;
            let _ = db
                .update_brush_torrent_stats(
                    task.id,
                    &torrent.hash,
                    torrent.uploaded,
                    torrent.downloaded,
                    torrent.time_active.max(0),
                    average_upload_speed(torrent.uploaded, torrent.time_active),
                    calculate_ratio(torrent.uploaded, torrent.downloaded, torrent.ratio),
                )
                .await;
        }
    }

    let _ = db.cleanup_old_torrent_traffic(7).await;

    Ok(())
}

fn torrent_has_tag(tags: &str, tag: &str) -> bool {
    tags.split(',')
        .map(str::trim)
        .any(|value| !value.is_empty() && value == tag)
}

fn average_upload_speed(uploaded_bytes: i64, duration_secs: i64) -> f64 {
    if duration_secs <= 0 {
        0.0
    } else {
        uploaded_bytes as f64 / duration_secs as f64
    }
}

fn calculate_ratio(uploaded_bytes: i64, downloaded_bytes: i64, fallback: f64) -> f64 {
    if downloaded_bytes > 0 {
        uploaded_bytes as f64 / downloaded_bytes as f64
    } else if uploaded_bytes > 0 {
        fallback.max(0.0)
    } else {
        0.0
    }
}
