use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use tokio::sync::{RwLock, broadcast};
use tokio::time::{Duration, sleep};
use tracing::{error, info, warn};

use crate::db::Database;
use crate::downloader::{DownloaderClientPool, DownloaderRecord, TorrentInfo};

/// 快照超过此秒数视为过期，需要强制刷新。
/// 设置为 3 个采集周期 (3 × 30s = 90s)，留有一定余量。
const STALE_SNAPSHOT_SECS: i64 = 90;

#[derive(Debug, Clone)]
pub struct DownloaderSnapshot {
    pub downloader_id: i64,
    pub recorded_at: String,
    pub torrents: Vec<TorrentInfo>,
}

pub struct DownloaderSnapshotCollector {
    db: Database,
    pool: Arc<DownloaderClientPool>,
    latest: RwLock<HashMap<i64, Arc<DownloaderSnapshot>>>,
    tx: broadcast::Sender<Arc<DownloaderSnapshot>>,
}

impl DownloaderSnapshotCollector {
    pub fn new(db: Database, pool: Arc<DownloaderClientPool>) -> Self {
        let (tx, _) = broadcast::channel(64);
        Self {
            db,
            pool,
            latest: RwLock::new(HashMap::new()),
            tx,
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Arc<DownloaderSnapshot>> {
        self.tx.subscribe()
    }

    pub async fn start(self: Arc<Self>) {
        info!("downloader snapshot collector started (interval: 10s)");
        loop {
            if let Err(error) = self.collect_all(true).await {
                error!("downloader snapshot collection error: {}", error);
            }
            sleep(Duration::from_secs(10)).await;
        }
    }

    pub async fn get_snapshot(&self, downloader_id: i64) -> Option<Arc<DownloaderSnapshot>> {
        self.latest.read().await.get(&downloader_id).cloned()
    }

    pub async fn get_or_refresh_snapshot(
        &self,
        downloader: &DownloaderRecord,
    ) -> Result<Arc<DownloaderSnapshot>, String> {
        if let Some(snapshot) = self.get_snapshot(downloader.id).await {
            let age_secs = snapshot_age_secs(&snapshot);
            if age_secs < STALE_SNAPSHOT_SECS {
                return Ok(snapshot);
            }
            warn!(
                "downloader '{}' snapshot is {}s old, forcing refresh",
                downloader.name, age_secs
            );
        }
        self.collect_for_downloader(downloader, false).await
    }

    pub async fn get_tagged_torrents(
        &self,
        downloader: &DownloaderRecord,
        tag: &str,
    ) -> Result<Vec<TorrentInfo>, String> {
        let snapshot = self.get_or_refresh_snapshot(downloader).await?;
        Ok(snapshot
            .torrents
            .iter()
            .filter(|torrent| torrent_has_tag(torrent, tag))
            .cloned()
            .collect())
    }

    pub async fn get_all_torrents(
        &self,
        downloader: &DownloaderRecord,
    ) -> Result<Vec<TorrentInfo>, String> {
        let snapshot = self.get_or_refresh_snapshot(downloader).await?;
        Ok(snapshot.torrents.clone())
    }

    async fn collect_all(&self, publish: bool) -> Result<(), String> {
        let downloaders = self
            .db
            .list_downloaders()
            .await
            .map_err(|e| e.to_string())?;
        for downloader in &downloaders {
            if let Err(error) = self.collect_for_downloader(downloader, publish).await {
                warn!(
                    "failed to collect snapshot for downloader '{}': {}",
                    downloader.name, error
                );
            }
        }
        Ok(())
    }

    async fn collect_for_downloader(
        &self,
        downloader: &DownloaderRecord,
        publish: bool,
    ) -> Result<Arc<DownloaderSnapshot>, String> {
        let client = self.pool.get(downloader).await?;
        let torrents = client.list_torrents(None).await?;
        let snapshot = Arc::new(DownloaderSnapshot {
            downloader_id: downloader.id,
            recorded_at: Utc::now().to_rfc3339(),
            torrents,
        });
        self.latest
            .write()
            .await
            .insert(downloader.id, snapshot.clone());
        if publish {
            let _ = self.tx.send(snapshot.clone());
        }
        Ok(snapshot)
    }
}

fn torrent_has_tag(torrent: &TorrentInfo, tag: &str) -> bool {
    torrent
        .tags
        .split(',')
        .map(str::trim)
        .any(|value| !value.is_empty() && value == tag)
}

fn snapshot_age_secs(snapshot: &DownloaderSnapshot) -> i64 {
    let recorded = match chrono::DateTime::parse_from_rfc3339(&snapshot.recorded_at) {
        Ok(t) => t.with_timezone(&Utc),
        Err(_) => return i64::MAX,
    };
    let now = Utc::now();
    (now - recorded).num_seconds().max(0)
}
