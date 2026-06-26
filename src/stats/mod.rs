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
                // 保留期清理：DELETE 已改为可命中 recorded_at 索引的范围删除，
                // 每条快照执行的成本很低（几乎只删今天之前到期的少量行）。
                let _ = db.cleanup_old_torrent_traffic(7).await;
                let _ = db.cleanup_old_speed_snapshots(7).await;
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

    let tasks = db.list_brush_tasks().await.map_err(|e| e.to_string())?;

    let mut task_stats = Vec::new();
    let mut torrent_stats = Vec::new();
    for task in tasks
        .into_iter()
        .filter(|task| task.downloader_ids.contains(&snapshot.downloader_id))
    {
        let torrents = snapshot
            .torrents
            .iter()
            .filter(|torrent| torrent_has_tag(&torrent.tags, &task.tag));

        let mut total_uploaded = 0i64;
        let mut total_downloaded = 0i64;
        let mut count = 0i64;
        for torrent in torrents {
            total_uploaded += torrent.uploaded;
            total_downloaded += torrent.downloaded;
            count += 1;
            torrent_stats.push(crate::db::TorrentSnapshotStats {
                task_id: task.id,
                hash: torrent.hash.clone(),
                uploaded: torrent.uploaded,
                downloaded: torrent.downloaded,
                download_duration_secs: torrent.time_active.max(0),
                avg_upload_speed: crate::brush::average_upload_speed(
                    torrent.uploaded,
                    torrent.time_active,
                ),
                ratio: crate::brush::calculate_ratio(
                    torrent.uploaded,
                    torrent.downloaded,
                    torrent.ratio,
                ),
            });
        }

        task_stats.push(crate::db::TaskSnapshotStats {
            task_id: task.id,
            total_uploaded,
            total_downloaded,
            torrent_count: count,
        });
    }

    // 整次快照的统计在单个事务中写入，避免 N+1 次新建连接。
    db.record_snapshot_stats(crate::db::SnapshotStatsBatch {
        downloader_id: snapshot.downloader_id,
        upload_speed,
        download_speed,
        tasks: task_stats,
        torrents: torrent_stats,
    })
    .await
    .map_err(|e| e.to_string())?;

    Ok(())
}

fn torrent_has_tag(tags: &str, tag: &str) -> bool {
    tags.split(',')
        .map(str::trim)
        .any(|value| !value.is_empty() && value == tag)
}
