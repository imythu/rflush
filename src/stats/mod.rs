use tokio::time::{Duration, sleep};
use tracing::{debug, error, info};

use crate::db::Database;
use crate::downloader::{DownloaderType, create_downloader_client};

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

/// 启动统计数据采集循环 (每60秒)
pub async fn start_stats_collector(db: Database) {
    info!("stats collector started (interval: 60s)");
    loop {
        if let Err(e) = collect_stats(&db).await {
            error!("stats collection error: {}", e);
        }
        sleep(Duration::from_secs(60)).await;
    }
}

async fn collect_stats(db: &Database) -> Result<(), String> {
    let tasks = db
        .list_brush_tasks()
        .await
        .map_err(|e| e.to_string())?;

    let downloaders = db
        .list_downloaders()
        .await
        .map_err(|e| e.to_string())?;

    for downloader in &downloaders {
        let dl_type = match DownloaderType::from_str(&downloader.downloader_type) {
            Some(t) => t,
            None => continue,
        };

        let client = create_downloader_client(
            dl_type,
            &downloader.url,
            &downloader.username,
            &downloader.password,
        );

        match client.list_torrents(None).await {
            Ok(torrents) => {
                let upload_speed: i64 = torrents.iter().map(|t| t.upload_speed).sum();
                let download_speed: i64 = torrents.iter().map(|t| t.download_speed).sum();
                let _ = db
                    .save_downloader_speed_snapshot(
                        downloader.id,
                        upload_speed,
                        download_speed,
                    )
                    .await;
            }
            Err(e) => {
                debug!("failed to collect downloader speed for '{}': {}", downloader.name, e);
            }
        }
    }

    for task in &tasks {
        if !task.enabled {
            continue;
        }

        let downloader = match db
            .get_downloader(task.downloader_id)
            .await
            .map_err(|e| e.to_string())?
        {
            Some(d) => d,
            None => continue,
        };

        let dl_type = match DownloaderType::from_str(&downloader.downloader_type) {
            Some(t) => t,
            None => continue,
        };

        let client = create_downloader_client(
            dl_type,
            &downloader.url,
            &downloader.username,
            &downloader.password,
        );

        match client.list_torrents(Some(&task.tag)).await {
            Ok(torrents) => {
                let total_uploaded: i64 = torrents.iter().map(|t| t.uploaded).sum();
                let total_downloaded: i64 = torrents.iter().map(|t| t.downloaded).sum();
                let count = torrents.len() as i64;

                // 保存任务级别快照
                let _ = db
                    .save_task_stats_snapshot(task.id, total_uploaded, total_downloaded, count)
                    .await;

                // 保存每个种子的流量快照 (用于速度计算)
                for torrent in &torrents {
                    let _ = db
                        .save_torrent_traffic(task.id, &torrent.hash, torrent.uploaded, torrent.downloaded)
                        .await;
                }

                debug!(
                    "stats collected for task '{}': {} torrents, up={}, down={}",
                    task.name, count, total_uploaded, total_downloaded
                );
            }
            Err(e) => {
                debug!("failed to collect stats for task '{}': {}", task.name, e);
            }
        }
    }

    // 清理旧的种子流量快照 (保留7天)
    let _ = db.cleanup_old_torrent_traffic(7).await;

    Ok(())
}
