use std::path::{Path, PathBuf};

use chrono::Utc;
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::brush::{BrushTaskRecord, BrushTaskRequest, BrushTorrentRecord};
use crate::config::{GlobalConfig, RssConfig, RssSubscription, TimeUnit};
use crate::downloader::DownloaderRecord;
use crate::error::AppError;
use crate::history::{FinalStatus, RunHistory, TorrentRunRecord};
use crate::sign_in::{SignInRecord, SignInResult, SignInTaskRecord, SignInTaskRequest};
use crate::site::{SiteRecord, SiteStatsRecord, SiteWithStats, UserStats};
use crate::stats::{DownloaderSpeedSnapshot, TaskStatsSnapshot};

#[derive(Clone)]
pub struct Database {
    path: PathBuf,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadHistoryRecord {
    pub id: i64,
    pub run_id: i64,
    pub task_id: Option<i64>,
    pub finished_at: String,
    pub rss_name: String,
    pub guid: String,
    pub title: String,
    pub retry_count: u32,
    pub refresh_count: u32,
    pub bytes: Option<u64>,
    pub final_status: String,
    pub final_message: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloadRunRecord {
    pub id: i64,
    pub started_at: String,
    pub finished_at: String,
    pub retry_delay_secs: u64,
    pub total: usize,
    pub succeeded: usize,
    pub skipped_existing: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaginatedRunRecords {
    pub run: DownloadRunRecord,
    pub page: usize,
    pub page_size: usize,
    pub total_records: usize,
    pub records: Vec<DownloadHistoryRecord>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PaginatedBrushTorrents {
    pub page: usize,
    pub page_size: usize,
    pub total_records: usize,
    pub records: Vec<BrushTorrentRecord>,
}

/// 一次下载器采集快照的统计写入批次（在单事务中提交）。
pub struct SnapshotStatsBatch {
    pub downloader_id: i64,
    pub upload_speed: i64,
    pub download_speed: i64,
    pub tasks: Vec<TaskSnapshotStats>,
    pub torrents: Vec<TorrentSnapshotStats>,
}

pub struct TaskSnapshotStats {
    pub task_id: i64,
    pub total_uploaded: i64,
    pub total_downloaded: i64,
    pub torrent_count: i64,
}

pub struct TorrentSnapshotStats {
    pub task_id: i64,
    pub hash: String,
    pub uploaded: i64,
    pub downloaded: i64,
    pub download_duration_secs: i64,
    pub avg_upload_speed: f64,
    pub ratio: f64,
}

impl Database {
    pub async fn open(data_dir: &Path) -> Result<Self, AppError> {
        tokio::fs::create_dir_all(&data_dir)
            .await
            .map_err(|source| AppError::CreateDir {
                path: data_dir.display().to_string(),
                source,
            })?;
        let path = data_dir.join("rflush.db");
        let db = Self { path };
        db.init().await?;
        Ok(db)
    }

    pub async fn get_settings(&self) -> Result<GlobalConfig, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<GlobalConfig, AppError> {
            let conn = open_connection(&path)?;
            let settings = conn
                .query_row(
                    "SELECT download_rate_limit_requests, download_rate_limit_interval, download_rate_limit_unit, retry_interval_secs, log_level, max_concurrent_downloads, max_concurrent_rss_fetches, throttle_interval_secs, proxy, use_proxy_for_lightpanda, tag_rule_scan_interval_mins FROM global_settings WHERE id = 1",
                    [],
                    |row| {
                        Ok(GlobalConfig {
                            download_rate_limit: crate::config::DownloadRateLimit {
                                requests: row.get(0)?,
                                interval: row.get(1)?,
                                unit: parse_time_unit(row.get::<_, String>(2)?),
                            },
                            retry_interval_secs: row.get(3)?,
                            log_level: row.get(4)?,
                            max_concurrent_downloads: row.get(5)?,
                            max_concurrent_rss_fetches: row.get(6)?,
                            throttle_interval_secs: row.get(7)?,
                            proxy: row.get(8)?,
                            use_proxy_for_lightpanda: row.get::<_, i32>(9).unwrap_or(1) != 0,
                            tag_rule_scan_interval_mins: row.get::<_, i64>(10).unwrap_or(7) as u64,
                        })
                    },
                )
                .map_err(sql_error)?;
            Ok(settings)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_settings(&self, settings: &GlobalConfig) -> Result<(), AppError> {
        let path = self.path.clone();
        let settings = settings.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE global_settings SET download_rate_limit_requests = ?, download_rate_limit_interval = ?, download_rate_limit_unit = ?, retry_interval_secs = ?, log_level = ?, max_concurrent_downloads = ?, max_concurrent_rss_fetches = ?, throttle_interval_secs = ?, proxy = ?, use_proxy_for_lightpanda = ?, tag_rule_scan_interval_mins = ? WHERE id = 1",
                params![
                    settings.download_rate_limit.requests,
                    settings.download_rate_limit.interval,
                    time_unit_name(settings.download_rate_limit.unit),
                    settings.retry_interval_secs,
                    settings.log_level,
                    settings.max_concurrent_downloads,
                    settings.max_concurrent_rss_fetches,
                    settings.throttle_interval_secs,
                    settings.proxy.as_deref().and_then(|value| {
                        let value = value.trim();
                        (!value.is_empty()).then_some(value)
                    }),
                    settings.use_proxy_for_lightpanda as i32,
                    settings.tag_rule_scan_interval_mins as i64
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_rss(&self) -> Result<Vec<RssSubscription>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<RssSubscription>, AppError> {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, url, enabled, downloader_id, created_at, updated_at FROM rss_subscriptions ORDER BY id DESC",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    map_rss_subscription(row)
                })
                .map_err(sql_error)?;
            let mut rss = Vec::new();
            for row in rows {
                rss.push(row.map_err(sql_error)?);
            }
            Ok(rss)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_rss(&self, id: i64) -> Result<Option<RssSubscription>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<RssSubscription>, AppError> {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, url, enabled, downloader_id, created_at, updated_at FROM rss_subscriptions WHERE id = ?",
                [id],
                map_rss_subscription,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_rss(
        &self,
        rss: RssConfig,
        enabled: bool,
    ) -> Result<RssSubscription, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<RssSubscription, AppError> {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO rss_subscriptions (name, url, enabled, downloader_id, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![rss.name, rss.url, if enabled { 1 } else { 0 }, rss.downloader_id, now, now],
            )
            .map_err(sql_error)?;
            let id = conn.last_insert_rowid();
            conn.query_row(
                "SELECT id, name, url, enabled, downloader_id, created_at, updated_at FROM rss_subscriptions WHERE id = ?",
                [id],
                map_rss_subscription,
            )
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_rss_enabled(&self, ids: &[i64], enabled: bool) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }

        let path = self.path.clone();
        let ids = ids.to_vec();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            for id in ids {
                tx.execute(
                    "UPDATE rss_subscriptions SET enabled = ?, updated_at = ? WHERE id = ?",
                    params![if enabled { 1 } else { 0 }, now, id],
                )
                .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_all_rss_enabled(&self, enabled: bool) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE rss_subscriptions SET enabled = ?, updated_at = ?",
                params![if enabled { 1 } else { 0 }, Utc::now().to_rfc3339()],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_rss(&self, id: i64) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<bool, AppError> {
            let conn = open_connection(&path)?;
            let changed = conn
                .execute("DELETE FROM rss_subscriptions WHERE id = ?", [id])
                .map_err(sql_error)?;
            Ok(changed > 0)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_rss_batch(&self, ids: &[i64]) -> Result<(), AppError> {
        if ids.is_empty() {
            return Ok(());
        }

        let path = self.path.clone();
        let ids = ids.to_vec();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            for id in ids {
                tx.execute("DELETE FROM rss_subscriptions WHERE id = ?", [id])
                    .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_history(&self, limit: usize) -> Result<Vec<DownloadHistoryRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<DownloadHistoryRecord>, AppError> {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT dr.id, dr.run_id, dr.task_id, runs.finished_at, dr.rss_name, dr.guid, dr.title, dr.retry_count, dr.refresh_count, dr.bytes, dr.file_name, dr.saved_path, dr.final_status, dr.final_message, dr.file_deleted
                     FROM download_records dr
                     JOIN download_runs runs ON runs.id = dr.run_id
                     ORDER BY dr.id DESC
                     LIMIT ?",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([limit as i64], |row| {
                    map_history_record(row)
                })
                .map_err(sql_error)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(sql_error)?);
            }
            Ok(records)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_runs(&self, limit: usize) -> Result<Vec<DownloadRunRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<DownloadRunRecord>, AppError> {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, started_at, finished_at, retry_delay_secs, total, succeeded, skipped_existing, failed
                     FROM download_runs
                     ORDER BY id DESC
                     LIMIT ?",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([limit as i64], |row| {
                    Ok(DownloadRunRecord {
                        id: row.get(0)?,
                        started_at: row.get(1)?,
                        finished_at: row.get(2)?,
                        retry_delay_secs: row.get::<_, i64>(3)? as u64,
                        total: row.get::<_, i64>(4)? as usize,
                        succeeded: row.get::<_, i64>(5)? as usize,
                        skipped_existing: row.get::<_, i64>(6)? as usize,
                        failed: row.get::<_, i64>(7)? as usize,
                    })
                })
                .map_err(sql_error)?;
            let mut runs = Vec::new();
            for row in rows {
                runs.push(row.map_err(sql_error)?);
            }
            Ok(runs)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_run_records(
        &self,
        run_id: i64,
        page: usize,
        page_size: usize,
    ) -> Result<Option<PaginatedRunRecords>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Option<PaginatedRunRecords>, AppError> {
            let conn = open_connection(&path)?;
            let run = conn
                .query_row(
                    "SELECT id, started_at, finished_at, retry_delay_secs, total, succeeded, skipped_existing, failed
                     FROM download_runs
                     WHERE id = ?",
                    [run_id],
                    |row| {
                        Ok(DownloadRunRecord {
                            id: row.get(0)?,
                            started_at: row.get(1)?,
                            finished_at: row.get(2)?,
                            retry_delay_secs: row.get::<_, i64>(3)? as u64,
                            total: row.get::<_, i64>(4)? as usize,
                            succeeded: row.get::<_, i64>(5)? as usize,
                            skipped_existing: row.get::<_, i64>(6)? as usize,
                            failed: row.get::<_, i64>(7)? as usize,
                        })
                    },
                )
                .optional()
                .map_err(sql_error)?;
            let Some(run) = run else {
                return Ok(None);
            };

            let page = page.max(1);
            let page_size = page_size.clamp(1, 100);
            let offset = (page - 1) * page_size;

            let total_records = conn
                .query_row(
                    "SELECT COUNT(*) FROM download_records WHERE run_id = ?",
                    [run_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sql_error)? as usize;

            let mut stmt = conn
                .prepare(
                    "SELECT id, run_id, task_id, finished_at, rss_name, guid, title, retry_count, refresh_count, bytes, file_name, saved_path, final_status, final_message, file_deleted
                     FROM download_records
                     WHERE run_id = ?
                     ORDER BY id DESC
                     LIMIT ? OFFSET ?",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![run_id, page_size as i64, offset as i64], |row| {
                    map_history_record(row)
                })
                .map_err(sql_error)?;

            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(sql_error)?);
            }

            Ok(Some(PaginatedRunRecords {
                run,
                page,
                page_size,
                total_records,
                records,
            }))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_task_records(
        &self,
        task_id: i64,
        page: usize,
        page_size: usize,
    ) -> Result<Vec<DownloadHistoryRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<DownloadHistoryRecord>, AppError> {
            let conn = open_connection(&path)?;
            let page = page.max(1);
            let page_size = page_size.clamp(1, 100);
            let offset = (page - 1) * page_size;
            let mut stmt = conn
                .prepare(
                    "SELECT id, run_id, task_id, finished_at, rss_name, guid, title, retry_count, refresh_count, bytes, file_name, saved_path, final_status, final_message, file_deleted
                     FROM download_records
                     WHERE task_id = ?
                     ORDER BY id DESC
                     LIMIT ? OFFSET ?",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![task_id, page_size as i64, offset as i64], map_history_record)
                .map_err(sql_error)?;
            let mut records = Vec::new();
            for row in rows {
                records.push(row.map_err(sql_error)?);
            }
            Ok(records)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn count_task_records(&self, task_id: i64) -> Result<usize, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, AppError> {
            let conn = open_connection(&path)?;
            let total = conn
                .query_row(
                    "SELECT COUNT(*) FROM download_records WHERE task_id = ?",
                    [task_id],
                    |row| row.get::<_, i64>(0),
                )
                .map_err(sql_error)?;
            Ok(total as usize)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn save_history(
        &self,
        history: &RunHistory,
        task_id: Option<i64>,
        task_name: Option<&str>,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let history = history.clone();
        let task_name = task_name.map(str::to_string);
        tokio::task::spawn_blocking(move || -> Result<i64, AppError> {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            tx.execute(
                "INSERT INTO download_runs (task_id, task_name, started_at, finished_at, retry_delay_secs, total, succeeded, skipped_existing, failed) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    task_id,
                    task_name,
                    history.started_at,
                    history.finished_at,
                    history.retry_delay_secs,
                    history.summary.total as i64,
                    history.summary.succeeded as i64,
                    history.summary.skipped_existing as i64,
                    history.summary.failed as i64
                ],
            )
            .map_err(sql_error)?;
            let run_id = tx.last_insert_rowid();
            for record in history.torrents {
                insert_record(&tx, run_id, task_id, &history.finished_at, &record)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(run_id)
        })
        .await
        .map_err(join_error)?
    }

    // ========== Sites ==========

    pub async fn list_sites(&self) -> Result<Vec<SiteRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT id, name, site_type, base_url, auth_config, use_proxy, created_at, updated_at FROM sites ORDER BY id")
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(SiteRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        use_proxy: row.get::<_, i32>(5).unwrap_or(1) != 0,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(sql_error)?;
            let mut sites = Vec::new();
            for row in rows {
                sites.push(row.map_err(sql_error)?);
            }
            Ok(sites)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_sites_with_stats(&self) -> Result<Vec<SiteWithStats>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT s.id, s.name, s.site_type, s.base_url, s.auth_config, s.use_proxy, s.created_at, s.updated_at,
                            st.site_id, st.uid, st.username, st.uploaded, st.downloaded, st.ratio, st.bonus,
                            st.seeding_count, st.leeching_count, st.updated_at, st.last_checked_at, st.last_error
                     FROM sites s
                     LEFT JOIN site_stats st ON st.site_id = s.id
                     ORDER BY s.id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    let stats_site_id: Option<i64> = row.get(8)?;
                    Ok(SiteWithStats {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        use_proxy: row.get::<_, i32>(5).unwrap_or(1) != 0,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                        stats: stats_site_id.map(|site_id| SiteStatsRecord {
                            site_id,
                            uid: row.get(9).ok().flatten(),
                            username: row.get(10).ok().flatten(),
                            uploaded: row.get::<_, Option<i64>>(11).ok().flatten().map(|v| v as u64),
                            downloaded: row.get::<_, Option<i64>>(12).ok().flatten().map(|v| v as u64),
                            ratio: row.get(13).ok().flatten(),
                            bonus: row.get(14).ok().flatten(),
                            seeding_count: row
                                .get::<_, Option<i64>>(15)
                                .ok()
                                .flatten()
                                .map(|v| v as u32),
                            leeching_count: row
                                .get::<_, Option<i64>>(16)
                                .ok()
                                .flatten()
                                .map(|v| v as u32),
                            updated_at: row.get(17).ok().flatten(),
                            last_checked_at: row.get(18).unwrap_or_default(),
                            last_error: row.get(19).ok().flatten(),
                        }),
                    })
                })
                .map_err(sql_error)?;
            let mut sites = Vec::new();
            for row in rows {
                sites.push(row.map_err(sql_error)?);
            }
            Ok(sites)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_site(&self, id: i64) -> Result<Option<SiteRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, site_type, base_url, auth_config, use_proxy, created_at, updated_at FROM sites WHERE id = ?",
                params![id],
                |row| {
                    Ok(SiteRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        use_proxy: row.get::<_, i32>(5).unwrap_or(1) != 0,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_site(
        &self,
        name: &str,
        site_type: &str,
        base_url: &str,
        auth_config: &str,
        use_proxy: bool,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, site_type, base_url, auth_config) = (
            name.to_string(),
            site_type.to_string(),
            base_url.to_string(),
            auth_config.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO sites (name, site_type, base_url, auth_config, use_proxy, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![name, site_type, base_url, auth_config, use_proxy as i32, now, now],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_site(
        &self,
        id: i64,
        name: &str,
        site_type: &str,
        base_url: &str,
        auth_config: &str,
        use_proxy: bool,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, site_type, base_url, auth_config) = (
            name.to_string(),
            site_type.to_string(),
            base_url.to_string(),
            auth_config.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE sites SET name = ?, site_type = ?, base_url = ?, auth_config = ?, use_proxy = ?, updated_at = ? WHERE id = ?",
                params![name, site_type, base_url, auth_config, use_proxy as i32, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_site(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM sites WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn upsert_site_stats_success(
        &self,
        site_id: i64,
        stats: &UserStats,
        checked_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let stats = stats.clone();
        let checked_at = checked_at.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO site_stats
                 (site_id, uid, username, uploaded, downloaded, ratio, bonus, seeding_count, leeching_count, updated_at, last_checked_at, last_error)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, NULL)
                 ON CONFLICT(site_id) DO UPDATE SET
                   uid = excluded.uid,
                   username = excluded.username,
                   uploaded = excluded.uploaded,
                   downloaded = excluded.downloaded,
                   ratio = excluded.ratio,
                   bonus = excluded.bonus,
                   seeding_count = excluded.seeding_count,
                   leeching_count = excluded.leeching_count,
                   updated_at = excluded.updated_at,
                   last_checked_at = excluded.last_checked_at,
                   last_error = NULL",
                params![
                    site_id,
                    stats.uid,
                    stats.username,
                    clamp_u64_to_i64(stats.uploaded),
                    clamp_u64_to_i64(stats.downloaded),
                    stats.ratio,
                    stats.bonus,
                    stats.seeding_count.map(|v| v as i64),
                    stats.leeching_count.map(|v| v as i64),
                    checked_at,
                    checked_at,
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn upsert_site_stats_error(
        &self,
        site_id: i64,
        message: &str,
        checked_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let message = message.to_string();
        let checked_at = checked_at.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO site_stats (site_id, last_checked_at, last_error)
                 VALUES (?, ?, ?)
                 ON CONFLICT(site_id) DO UPDATE SET
                   last_checked_at = excluded.last_checked_at,
                   last_error = excluded.last_error",
                params![site_id, checked_at, message],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Sign-in Tasks ==========

    pub async fn list_sign_in_tasks(&self) -> Result<Vec<SignInTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, site_id, cron_expression, lightpanda_endpoint, lightpanda_token,
                     lightpanda_region, browser, proxy, country, enabled, last_status, last_message,
                     last_run_at, created_at, updated_at
                     FROM sign_in_tasks ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], map_sign_in_task)
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_sign_in_task(&self, id: i64) -> Result<Option<SignInTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, site_id, cron_expression, lightpanda_endpoint, lightpanda_token,
                 lightpanda_region, browser, proxy, country, enabled, last_status, last_message,
                 last_run_at, created_at, updated_at
                 FROM sign_in_tasks WHERE id = ?",
                params![id],
                map_sign_in_task,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_sign_in_task(&self, req: &SignInTaskRequest) -> Result<i64, AppError> {
        let path = self.path.clone();
        let req = req.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO sign_in_tasks
                 (name, site_id, cron_expression, lightpanda_endpoint, lightpanda_token,
                  lightpanda_region, browser, proxy, country, enabled, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
                params![
                    req.name,
                    req.site_id,
                    req.cron_expression,
                    req.lightpanda_endpoint,
                    req.lightpanda_token,
                    req.lightpanda_region
                        .unwrap_or_else(|| "euwest".to_string()),
                    req.browser.unwrap_or_else(|| "lightpanda".to_string()),
                    req.proxy.unwrap_or_else(|| "fast_dc".to_string()),
                    req.country,
                    now,
                    now,
                ],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_sign_in_task(
        &self,
        id: i64,
        req: &SignInTaskRequest,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let req = req.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE sign_in_tasks SET
                 name = ?, site_id = ?, cron_expression = ?, lightpanda_endpoint = ?, lightpanda_token = ?,
                 lightpanda_region = ?, browser = ?, proxy = ?, country = ?, updated_at = ?
                 WHERE id = ?",
                params![
                    req.name,
                    req.site_id,
                    req.cron_expression,
                    req.lightpanda_endpoint,
                    req.lightpanda_token,
                    req.lightpanda_region.unwrap_or_else(|| "euwest".to_string()),
                    req.browser.unwrap_or_else(|| "lightpanda".to_string()),
                    req.proxy.unwrap_or_else(|| "fast_dc".to_string()),
                    req.country,
                    now,
                    id,
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_sign_in_task(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM sign_in_tasks WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_sign_in_task_enabled(&self, id: i64, enabled: bool) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE sign_in_tasks SET enabled = ?, updated_at = ? WHERE id = ?",
                params![enabled as i32, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_sign_in_task_result(
        &self,
        id: i64,
        status: &str,
        message: &str,
        run_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let (status, message, run_at) =
            (status.to_string(), message.to_string(), run_at.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE sign_in_tasks SET last_status = ?, last_message = ?, last_run_at = ?, updated_at = ? WHERE id = ?",
                params![status, message, run_at, Utc::now().to_rfc3339(), id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn insert_sign_in_record(
        &self,
        task: &SignInTaskRecord,
        site_id: i64,
        site_name: &str,
        result: &SignInResult,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let task = task.clone();
        let site_name = site_name.to_string();
        let result = result.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO sign_in_records
                 (task_id, site_id, site_name, started_at, finished_at, status, message)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    task.id,
                    site_id,
                    site_name,
                    result.started_at,
                    result.finished_at,
                    result.status,
                    result.message
                ],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_sign_in_records(
        &self,
        task_id: Option<i64>,
        limit: usize,
    ) -> Result<Vec<SignInRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let limit = limit.clamp(1, 500) as i64;
            let mut records = Vec::new();
            if let Some(task_id) = task_id {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, task_id, site_id, site_name, started_at, finished_at, status, message
                         FROM sign_in_records WHERE task_id = ? ORDER BY id DESC LIMIT ?",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![task_id, limit], map_sign_in_record)
                    .map_err(sql_error)?;
                for row in rows {
                    records.push(row.map_err(sql_error)?);
                }
            } else {
                let mut stmt = conn
                    .prepare(
                        "SELECT id, task_id, site_id, site_name, started_at, finished_at, status, message
                         FROM sign_in_records ORDER BY id DESC LIMIT ?",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![limit], map_sign_in_record)
                    .map_err(sql_error)?;
                for row in rows {
                    records.push(row.map_err(sql_error)?);
                }
            }
            Ok(records)
        })
        .await
        .map_err(join_error)?
    }

    // ========== Downloaders ==========

    pub async fn list_downloaders(&self) -> Result<Vec<DownloaderRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare("SELECT id, name, downloader_type, url, username, password, created_at, updated_at FROM downloaders ORDER BY id")
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(DownloaderRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        downloader_type: row.get(2)?,
                        url: row.get(3)?,
                        username: row.get(4)?,
                        password: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_downloader(&self, id: i64) -> Result<Option<DownloaderRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, downloader_type, url, username, password, created_at, updated_at FROM downloaders WHERE id = ?",
                params![id],
                |row| {
                    Ok(DownloaderRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        downloader_type: row.get(2)?,
                        url: row.get(3)?,
                        username: row.get(4)?,
                        password: row.get(5)?,
                        created_at: row.get(6)?,
                        updated_at: row.get(7)?,
                    })
                },
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_downloader(
        &self,
        name: &str,
        dtype: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, dtype, url, username, password) = (
            name.to_string(),
            dtype.to_string(),
            url.to_string(),
            username.to_string(),
            password.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO downloaders (name, downloader_type, url, username, password, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![name, dtype, url, username, password, now, now],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_downloader(
        &self,
        id: i64,
        name: &str,
        dtype: &str,
        url: &str,
        username: &str,
        password: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (name, dtype, url, username, password) = (
            name.to_string(),
            dtype.to_string(),
            url.to_string(),
            username.to_string(),
            password.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE downloaders SET name = ?, downloader_type = ?, url = ?, username = ?, password = ?, updated_at = ? WHERE id = ?",
                params![name, dtype, url, username, password, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_downloader(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM downloaders WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Brush Tasks ==========

    pub async fn list_brush_tasks(&self) -> Result<Vec<BrushTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, cron_expression, site_id, downloader_id, tag, rss_url,
                     seed_volume_gb, save_dir, active_time_windows,
                     promotion, skip_hit_and_run, max_concurrent,
                     download_speed_limit, upload_speed_limit,
                     size_ranges, seeder_ranges, min_free_hours,
                     delete_mode, delete_on_free_expiry, min_seed_time_hours, hr_min_seed_time_hours,
                     target_ratio, max_upload_gb, download_timeout_hours,
                     min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                     enabled, created_at, updated_at, downloader_ranges
                     FROM brush_tasks ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| row_to_brush_task(row))
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_brush_task(&self, id: i64) -> Result<Option<BrushTaskRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, cron_expression, site_id, downloader_id, tag, rss_url,
                 seed_volume_gb, save_dir, active_time_windows,
                 promotion, skip_hit_and_run, max_concurrent,
                 download_speed_limit, upload_speed_limit,
                 size_ranges, seeder_ranges, min_free_hours,
                 delete_mode, delete_on_free_expiry, min_seed_time_hours, hr_min_seed_time_hours,
                 target_ratio, max_upload_gb, download_timeout_hours,
                 min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                 enabled, created_at, updated_at, downloader_ranges
                 FROM brush_tasks WHERE id = ?",
                params![id],
                |row| row_to_brush_task(row),
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_brush_task(&self, req: &BrushTaskRequest) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let promotion = req.promotion.unwrap_or_else(|| "all".to_string());
            let min_free_hours = if promotion == "free" {
                req.min_free_hours
            } else {
                None
            };
            let skip_hr = req.skip_hit_and_run.unwrap_or(true) as i32;
            let max_concurrent = req.max_concurrent.unwrap_or(100);
            let delete_mode = req.delete_mode.unwrap_or_else(|| "or".to_string());
            let delete_on_free_expiry = req.delete_on_free_expiry.unwrap_or(false) as i32;
            conn.execute(
                "INSERT INTO brush_tasks (name, cron_expression, site_id, downloader_id, tag, rss_url,
                 seed_volume_gb, save_dir, active_time_windows,
                 promotion, skip_hit_and_run, max_concurrent,
                 download_speed_limit, upload_speed_limit,
                 size_ranges, seeder_ranges, downloader_ranges, min_free_hours,
                 delete_mode, delete_on_free_expiry, min_seed_time_hours, hr_min_seed_time_hours,
                 target_ratio, max_upload_gb, download_timeout_hours,
                 min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                 enabled, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
                params![
                    req.name, req.cron_expression, req.site_id, req.downloader_id, req.tag, req.rss_url,
                    req.seed_volume_gb, req.save_dir, req.active_time_windows,
                    promotion, skip_hr, max_concurrent,
                    req.download_speed_limit, req.upload_speed_limit,
                    req.size_ranges, req.seeder_ranges, req.downloader_ranges, min_free_hours,
                    delete_mode, delete_on_free_expiry, req.min_seed_time_hours, req.hr_min_seed_time_hours,
                    req.target_ratio, req.max_upload_gb, req.download_timeout_hours,
                    req.min_avg_upload_speed_kbs, req.max_inactive_hours, req.min_disk_space_gb,
                    now, now
                ],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_brush_task(&self, id: i64, req: &BrushTaskRequest) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let promotion = req.promotion.unwrap_or_else(|| "all".to_string());
            let min_free_hours = if promotion == "free" {
                req.min_free_hours
            } else {
                None
            };
            let skip_hr = req.skip_hit_and_run.unwrap_or(true) as i32;
            let max_concurrent = req.max_concurrent.unwrap_or(100);
            let delete_mode = req.delete_mode.unwrap_or_else(|| "or".to_string());
            let delete_on_free_expiry = req.delete_on_free_expiry.unwrap_or(false) as i32;
            conn.execute(
                "UPDATE brush_tasks SET name = ?, cron_expression = ?, site_id = ?, downloader_id = ?, tag = ?, rss_url = ?,
                 seed_volume_gb = ?, save_dir = ?, active_time_windows = ?,
                 promotion = ?, skip_hit_and_run = ?, max_concurrent = ?,
                 download_speed_limit = ?, upload_speed_limit = ?,
                 size_ranges = ?, seeder_ranges = ?, downloader_ranges = ?, min_free_hours = ?,
                 delete_mode = ?, delete_on_free_expiry = ?, min_seed_time_hours = ?, hr_min_seed_time_hours = ?,
                 target_ratio = ?, max_upload_gb = ?, download_timeout_hours = ?,
                 min_avg_upload_speed_kbs = ?, max_inactive_hours = ?, min_disk_space_gb = ?,
                 updated_at = ? WHERE id = ?",
                params![
                    req.name, req.cron_expression, req.site_id, req.downloader_id, req.tag, req.rss_url,
                    req.seed_volume_gb, req.save_dir, req.active_time_windows,
                    promotion, skip_hr, max_concurrent,
                    req.download_speed_limit, req.upload_speed_limit,
                    req.size_ranges, req.seeder_ranges, req.downloader_ranges, min_free_hours,
                    delete_mode, delete_on_free_expiry, req.min_seed_time_hours, req.hr_min_seed_time_hours,
                    req.target_ratio, req.max_upload_gb, req.download_timeout_hours,
                    req.min_avg_upload_speed_kbs, req.max_inactive_hours, req.min_disk_space_gb,
                    now, id
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_brush_task(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM brush_tasks WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_brush_task_enabled(&self, id: i64, enabled: bool) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_tasks SET enabled = ?, updated_at = ? WHERE id = ?",
                params![enabled as i32, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Brush Task Torrents ==========

    pub async fn list_brush_task_torrents(
        &self,
        task_id: i64,
        page: usize,
        page_size: usize,
        keyword: Option<&str>,
    ) -> Result<PaginatedBrushTorrents, AppError> {
        let path = self.path.clone();
        let keyword = keyword
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let page = page.max(1);
            let page_size = page_size.clamp(1, 100);
            let offset = (page - 1) * page_size;

            let like = keyword.as_ref().map(|value| format!("%{}%", value));
            let total_records: usize = if let Some(ref like) = like {
                conn.query_row(
                    "SELECT COUNT(*) FROM brush_task_torrents
                     WHERE task_id = ?
                       AND (torrent_name LIKE ? OR COALESCE(torrent_id, '') LIKE ?)",
                    params![task_id, like, like],
                    |row| row.get(0),
                )
                .map_err(sql_error)?
            } else {
                conn.query_row(
                    "SELECT COUNT(*) FROM brush_task_torrents WHERE task_id = ?",
                    params![task_id],
                    |row| row.get(0),
                )
                .map_err(sql_error)?
            };

            let sql = if like.is_some() {
                "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, removed_at, remove_reason,
                        uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at
                 FROM brush_task_torrents
                 WHERE task_id = ?
                   AND (torrent_name LIKE ? OR COALESCE(torrent_id, '') LIKE ?)
                 ORDER BY CASE WHEN removed_at IS NULL THEN 0 ELSE 1 END, added_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            } else {
                "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, removed_at, remove_reason,
                        uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at
                 FROM brush_task_torrents
                 WHERE task_id = ?
                 ORDER BY CASE WHEN removed_at IS NULL THEN 0 ELSE 1 END, added_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            };

            let mut stmt = conn.prepare(sql).map_err(sql_error)?;
            let mut list = Vec::new();
            if let Some(like) = like {
                let rows = stmt
                    .query_map(params![task_id, like, like, page_size as i64, offset as i64], map_brush_torrent_record)
                    .map_err(sql_error)?;
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
            } else {
                let rows = stmt
                    .query_map(params![task_id, page_size as i64, offset as i64], map_brush_torrent_record)
                    .map_err(sql_error)?;
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
            }
            Ok(PaginatedBrushTorrents {
                page,
                page_size,
                total_records,
                records: list,
            })
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_active_brush_torrents(
        &self,
        task_id: i64,
    ) -> Result<Vec<BrushTorrentRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status, removed_at, remove_reason,
                            uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at
                     FROM brush_task_torrents WHERE task_id = ? AND status = 'active' ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![task_id], map_brush_torrent_record)
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_non_removed_brush_torrents_with_tasks(
        &self,
    ) -> Result<Vec<(BrushTorrentRecord, BrushTaskRecord)>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT bt.id, bt.task_id, bt.torrent_id, bt.torrent_link, bt.torrent_hash, bt.torrent_name,
                            bt.added_at, bt.size_bytes, bt.is_hr, bt.free_end_timestamp, bt.status, bt.removed_at,
                            bt.remove_reason, bt.uploaded_bytes, bt.downloaded_bytes, bt.download_duration_secs,
                            bt.avg_upload_speed, bt.ratio, bt.last_stats_at,
                            t.id, t.name, t.cron_expression, t.site_id, t.downloader_id, t.tag, t.rss_url,
                            t.seed_volume_gb, t.save_dir, t.active_time_windows,
                            t.promotion, t.skip_hit_and_run, t.max_concurrent,
                            t.download_speed_limit, t.upload_speed_limit,
                            t.size_ranges, t.seeder_ranges, t.min_free_hours,
                            t.delete_mode, t.delete_on_free_expiry, t.min_seed_time_hours, t.hr_min_seed_time_hours,
                            t.target_ratio, t.max_upload_gb, t.download_timeout_hours,
                            t.min_avg_upload_speed_kbs, t.max_inactive_hours, t.min_disk_space_gb,
                            t.enabled, t.created_at, t.updated_at, t.downloader_ranges
                     FROM brush_task_torrents bt
                     INNER JOIN brush_tasks t ON bt.task_id = t.id
                     WHERE bt.status != 'removed'",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    let torrent = map_brush_torrent_record(row)?;
                    let task = row_to_brush_task_at(row, 19)?;
                    Ok((torrent, task))
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn add_brush_torrent(
        &self,
        task_id: i64,
        torrent_id: Option<&str>,
        torrent_link: Option<&str>,
        hash: &str,
        name: &str,
        size_bytes: Option<i64>,
        is_hr: bool,
        free_end_timestamp: Option<i64>,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (torrent_id, torrent_link, hash, name) = (
            torrent_id.map(|value| value.to_string()),
            torrent_link.map(|value| value.to_string()),
            hash.to_string(),
            name.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO brush_task_torrents (task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, free_end_timestamp, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'active')
                 ON CONFLICT(task_id, torrent_hash) DO UPDATE SET
                    torrent_id = excluded.torrent_id,
                    torrent_link = excluded.torrent_link,
                    torrent_name = excluded.torrent_name,
                    added_at = excluded.added_at,
                    size_bytes = excluded.size_bytes,
                    is_hr = excluded.is_hr,
                    free_end_timestamp = excluded.free_end_timestamp,
                    status = 'active',
                    removed_at = NULL,
                    remove_reason = NULL",
                params![task_id, torrent_id, torrent_link, hash, name, now, size_bytes, is_hr as i32, free_end_timestamp],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_brush_torrent_status(
        &self,
        task_id: i64,
        hash: &str,
        status: &str,
        reason: Option<&str>,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let (hash, status) = (hash.to_string(), status.to_string());
        let reason = reason.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_task_torrents SET status = ?, removed_at = ?, remove_reason = ? WHERE task_id = ? AND torrent_hash = ? AND status = 'active'",
                params![status, now, reason, task_id, hash],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Stats Snapshots ==========

    /// 在单个连接 + 单个事务中写入一次采集快照的全部统计数据。
    /// 替代此前每个种子各开一个连接的 N+1 写法，显著降低高频采集下的开销。
    pub async fn record_snapshot_stats(&self, batch: SnapshotStatsBatch) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let mut conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let tx = conn.transaction().map_err(sql_error)?;

            tx.execute(
                "INSERT INTO downloader_speed_snapshots (downloader_id, upload_speed, download_speed, recorded_at) VALUES (?, ?, ?, ?)",
                params![batch.downloader_id, batch.upload_speed, batch.download_speed, now],
            )
            .map_err(sql_error)?;

            for task in &batch.tasks {
                tx.execute(
                    "INSERT INTO task_stats_snapshots (task_id, total_uploaded, total_downloaded, torrent_count, recorded_at) VALUES (?, ?, ?, ?, ?)",
                    params![task.task_id, task.total_uploaded, task.total_downloaded, task.torrent_count, now],
                )
                .map_err(sql_error)?;
            }

            for torrent in &batch.torrents {
                tx.execute(
                    "INSERT INTO torrent_traffic (task_id, torrent_hash, uploaded_bytes, downloaded_bytes, recorded_at) VALUES (?, ?, ?, ?, ?)",
                    params![torrent.task_id, torrent.hash, torrent.uploaded, torrent.downloaded, now],
                )
                .map_err(sql_error)?;
                tx.execute(
                    "UPDATE brush_task_torrents
                     SET uploaded_bytes = ?, downloaded_bytes = ?, download_duration_secs = ?,
                         avg_upload_speed = ?, ratio = ?, last_stats_at = ?
                     WHERE task_id = ? AND torrent_hash = ?",
                    params![
                        torrent.uploaded,
                        torrent.downloaded,
                        torrent.download_duration_secs,
                        torrent.avg_upload_speed,
                        torrent.ratio,
                        now,
                        torrent.task_id,
                        torrent.hash
                    ],
                )
                .map_err(sql_error)?;
            }

            tx.commit().map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_brush_torrent_stats(
        &self,
        task_id: i64,
        hash: &str,
        uploaded_bytes: i64,
        downloaded_bytes: i64,
        download_duration_secs: i64,
        avg_upload_speed: f64,
        ratio: f64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_task_torrents
                 SET uploaded_bytes = ?, downloaded_bytes = ?, download_duration_secs = ?,
                     avg_upload_speed = ?, ratio = ?, last_stats_at = ?
                 WHERE task_id = ? AND torrent_hash = ?",
                params![
                    uploaded_bytes,
                    downloaded_bytes,
                    download_duration_secs,
                    avg_upload_speed,
                    ratio,
                    now,
                    task_id,
                    hash
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_brush_task_transfer_totals(
        &self,
        task_id: i64,
    ) -> Result<(i64, i64, i64), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let totals = conn
                .query_row(
                    "SELECT
                        COALESCE(SUM(uploaded_bytes), 0),
                        COALESCE(SUM(downloaded_bytes), 0),
                        COUNT(*)
                     FROM brush_task_torrents
                     WHERE task_id = ?",
                    params![task_id],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )
                .map_err(sql_error)?;
            Ok(totals)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_downloader_speed_snapshots(
        &self,
        downloader_id: Option<i64>,
        since: &str,
        until: &str,
        bucket_secs: Option<i64>,
    ) -> Result<Vec<DownloaderSpeedSnapshot>, AppError> {
        let path = self.path.clone();
        let (since, until) = (since.to_string(), until.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;

            // When bucket_secs is set, aggregate (AVG) within time buckets per downloader.
            // Buckets are computed by truncating recorded_at to the nearest interval.
            if let Some(bs) = bucket_secs.filter(|&bs| bs > 0) {
                // Build a strftime format string that truncates to the given interval.
                // 10s: seconds digit is 0, 60s: seconds are :00, 300s: round to 5min, 3600s: round to hour
                let bucket_expr = if bs < 60 {
                    // Sub-minute: truncate seconds to nearest bucket (e.g., 10s → floor to 0,10,20,…)
                    format!(
                        "strftime('%Y-%m-%dT%H:%M:', recorded_at) || printf('%02d', (CAST(strftime('%S', recorded_at) AS INTEGER) / {bs}) * {bs}) || 'Z'"
                    )
                } else if bs < 3600 {
                    let mins = bs / 60;
                    format!(
                        "strftime('%Y-%m-%dT%H:', recorded_at) || printf('%02d', (CAST(strftime('%M', recorded_at) AS INTEGER) / {mins}) * {mins}) || ':00Z'"
                    )
                } else {
                    let hours = bs / 3600;
                    format!(
                        "strftime('%Y-%m-%dT', recorded_at) || printf('%02d', (CAST(strftime('%H', recorded_at) AS INTEGER) / {hours}) * {hours}) || ':00:00Z'"
                    )
                };

                let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                    if let Some(did) = downloader_id {
                        (
                            format!(
                                "SELECT 0, downloader_id, CAST(avg(upload_speed) AS INTEGER), CAST(avg(download_speed) AS INTEGER), {bucket_expr} AS bucket
                                 FROM downloader_speed_snapshots
                                 WHERE downloader_id = ?
                                   AND datetime(recorded_at) >= datetime(?)
                                   AND datetime(recorded_at) <= datetime(?)
                                 GROUP BY downloader_id, bucket
                                 ORDER BY bucket"
                            ),
                            vec![Box::new(did), Box::new(since), Box::new(until)],
                        )
                    } else {
                        (
                            format!(
                                "SELECT 0, downloader_id, CAST(avg(upload_speed) AS INTEGER), CAST(avg(download_speed) AS INTEGER), {bucket_expr} AS bucket
                                 FROM downloader_speed_snapshots
                                 WHERE datetime(recorded_at) >= datetime(?)
                                   AND datetime(recorded_at) <= datetime(?)
                                 GROUP BY downloader_id, bucket
                                 ORDER BY bucket"
                            ),
                            vec![Box::new(since), Box::new(until)],
                        )
                    };

                let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
                let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                    params_vec.iter().map(|p| p.as_ref()).collect();
                let rows = stmt
                    .query_map(params_refs.as_slice(), |row| {
                        Ok(DownloaderSpeedSnapshot {
                            id: row.get(0)?,
                            downloader_id: row.get(1)?,
                            upload_speed: row.get(2)?,
                            download_speed: row.get(3)?,
                            recorded_at: row.get(4)?,
                        })
                    })
                    .map_err(sql_error)?;
                let mut list = Vec::new();
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
                return Ok(list);
            }

            // Raw query (no aggregation)
            let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) =
                if let Some(downloader_id) = downloader_id {
                    (
                        "SELECT id, downloader_id, upload_speed, download_speed, recorded_at
                         FROM downloader_speed_snapshots
                         WHERE downloader_id = ?
                           AND datetime(recorded_at) >= datetime(?)
                           AND datetime(recorded_at) <= datetime(?)
                         ORDER BY datetime(recorded_at)"
                            .to_string(),
                        vec![Box::new(downloader_id), Box::new(since), Box::new(until)],
                    )
                } else {
                    (
                        "SELECT id, downloader_id, upload_speed, download_speed, recorded_at
                         FROM downloader_speed_snapshots
                         WHERE datetime(recorded_at) >= datetime(?)
                           AND datetime(recorded_at) <= datetime(?)
                         ORDER BY datetime(recorded_at)"
                            .to_string(),
                        vec![Box::new(since), Box::new(until)],
                    )
                };
            let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> =
                params_vec.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok(DownloaderSpeedSnapshot {
                        id: row.get(0)?,
                        downloader_id: row.get(1)?,
                        upload_speed: row.get(2)?,
                        download_speed: row.get(3)?,
                        recorded_at: row.get(4)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_task_stats_snapshots(
        &self,
        task_id: Option<i64>,
        since: &str,
        until: &str,
    ) -> Result<Vec<TaskStatsSnapshot>, AppError> {
        let path = self.path.clone();
        let (since, until) = (since.to_string(), until.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = if let Some(tid) = task_id {
                (
                    "SELECT id, task_id, total_uploaded, total_downloaded, torrent_count, recorded_at
                     FROM task_stats_snapshots
                     WHERE task_id = ?
                       AND datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     ORDER BY datetime(recorded_at)".to_string(),
                    vec![Box::new(tid), Box::new(since), Box::new(until)],
                )
            } else {
                (
                    "SELECT id, task_id, total_uploaded, total_downloaded, torrent_count, recorded_at
                     FROM task_stats_snapshots
                     WHERE datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     ORDER BY datetime(recorded_at)".to_string(),
                    vec![Box::new(since), Box::new(until)],
                )
            };
            let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
            let params_refs: Vec<&dyn rusqlite::types::ToSql> = params_vec.iter().map(|p| p.as_ref()).collect();
            let rows = stmt
                .query_map(params_refs.as_slice(), |row| {
                    Ok(TaskStatsSnapshot {
                        id: row.get(0)?,
                        task_id: row.get(1)?,
                        total_uploaded: row.get(2)?,
                        total_downloaded: row.get(3)?,
                        torrent_count: row.get(4)?,
                        recorded_at: row.get(5)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn save_torrent_traffic(
        &self,
        task_id: i64,
        hash: &str,
        uploaded: i64,
        downloaded: i64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let hash = hash.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO torrent_traffic (task_id, torrent_hash, uploaded_bytes, downloaded_bytes, recorded_at) VALUES (?, ?, ?, ?, ?)",
                params![task_id, hash, uploaded, downloaded, now],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_recent_torrent_traffic(
        &self,
        task_id: i64,
        hash: &str,
        minutes: i64,
    ) -> Result<Vec<(i64, i64, String)>, AppError> {
        let path = self.path.clone();
        let hash = hash.to_string();
        let since = (Utc::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT uploaded_bytes, downloaded_bytes, recorded_at FROM torrent_traffic
                     WHERE task_id = ? AND torrent_hash = ? AND datetime(recorded_at) >= datetime(?)
                     ORDER BY datetime(recorded_at)",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![task_id, hash, since], |row| {
                    Ok((
                        row.get::<_, i64>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, String>(2)?,
                    ))
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn cleanup_old_torrent_traffic(&self, days: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            // recorded_at 全部为同格式 UTC RFC3339 字符串，字典序即时间序，
            // 直接比较裸列可命中 recorded_at 索引，避免 datetime() 包裹导致的全表扫描。
            conn.execute(
                "DELETE FROM torrent_traffic WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn cleanup_old_speed_snapshots(&self, days: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "DELETE FROM downloader_speed_snapshots WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            conn.execute(
                "DELETE FROM task_stats_snapshots WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn insert_system_snapshot(
        &self,
        snap: &crate::monitor::SystemSnapshotRecord,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let snap = snap.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO system_snapshots (process_cpu_usage, process_memory_bytes, system_cpu_usage, system_total_memory_bytes, system_used_memory_bytes, system_available_memory_bytes, recorded_at) VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![
                    snap.process_cpu_usage,
                    snap.process_memory_bytes,
                    snap.system_cpu_usage,
                    snap.system_total_memory_bytes,
                    snap.system_used_memory_bytes,
                    snap.system_available_memory_bytes,
                    snap.recorded_at,
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_system_snapshots(
        &self,
        since: &str,
        until: &str,
        bucket_secs: Option<i64>,
    ) -> Result<Vec<crate::monitor::SystemSnapshotRecord>, AppError> {
        let path = self.path.clone();
        let since = since.to_string();
        let until = until.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;

            if let Some(bs) = bucket_secs.filter(|&bs| bs > 0) {
                let bucket_expr = if bs < 60 {
                    format!(
                        "strftime('%Y-%m-%dT%H:%M:', recorded_at) || printf('%02d', (CAST(strftime('%S', recorded_at) AS INTEGER) / {bs}) * {bs}) || 'Z'"
                    )
                } else if bs < 3600 {
                    let mins = bs / 60;
                    format!(
                        "strftime('%Y-%m-%dT%H:', recorded_at) || printf('%02d', (CAST(strftime('%M', recorded_at) AS INTEGER) / {mins}) * {mins}) || ':00Z'"
                    )
                } else {
                    let hours = bs / 3600;
                    format!(
                        "strftime('%Y-%m-%dT', recorded_at) || printf('%02d', (CAST(strftime('%H', recorded_at) AS INTEGER) / {hours}) * {hours}) || ':00:00Z'"
                    )
                };

                let sql = format!(
                    "SELECT 0,
                            CAST(avg(process_cpu_usage) AS REAL),
                            CAST(avg(process_memory_bytes) AS INTEGER),
                            CAST(avg(system_cpu_usage) AS REAL),
                            CAST(avg(system_total_memory_bytes) AS INTEGER),
                            CAST(avg(system_used_memory_bytes) AS INTEGER),
                            CAST(avg(system_available_memory_bytes) AS INTEGER),
                            {bucket_expr} AS bucket
                     FROM system_snapshots
                     WHERE datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     GROUP BY bucket
                     ORDER BY bucket"
                );

                let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
                let rows = stmt
                    .query_map(params![since, until], |row| {
                        Ok(crate::monitor::SystemSnapshotRecord {
                            id: row.get(0)?,
                            process_cpu_usage: row.get(1)?,
                            process_memory_bytes: row.get(2)?,
                            system_cpu_usage: row.get(3)?,
                            system_total_memory_bytes: row.get(4)?,
                            system_used_memory_bytes: row.get(5)?,
                            system_available_memory_bytes: row.get(6)?,
                            recorded_at: row.get(7)?,
                        })
                    })
                    .map_err(sql_error)?;
                let mut list = Vec::new();
                for row in rows {
                    list.push(row.map_err(sql_error)?);
                }
                return Ok(list);
            }

            let mut stmt = conn
                .prepare(
                    "SELECT id, process_cpu_usage, process_memory_bytes,
                            system_cpu_usage, system_total_memory_bytes,
                            system_used_memory_bytes, system_available_memory_bytes,
                            recorded_at
                     FROM system_snapshots
                     WHERE datetime(recorded_at) >= datetime(?)
                       AND datetime(recorded_at) <= datetime(?)
                     ORDER BY datetime(recorded_at)",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map(params![since, until], |row| {
                    Ok(crate::monitor::SystemSnapshotRecord {
                        id: row.get(0)?,
                        process_cpu_usage: row.get(1)?,
                        process_memory_bytes: row.get(2)?,
                        system_cpu_usage: row.get(3)?,
                        system_total_memory_bytes: row.get(4)?,
                        system_used_memory_bytes: row.get(5)?,
                        system_available_memory_bytes: row.get(6)?,
                        recorded_at: row.get(7)?,
                    })
                })
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn cleanup_old_system_snapshots(&self, days: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let cutoff = (Utc::now() - chrono::Duration::days(days)).to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "DELETE FROM system_snapshots WHERE recorded_at < ?",
                params![cutoff],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Tag Rules ==========

    pub async fn list_tag_rules(&self) -> Result<Vec<crate::tag_rule::TagRuleRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, tag_name, match_rules, enabled, downloader_ids, tagged_torrent_count, tagged_total_size, created_at, updated_at
                     FROM tag_rules ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| row_to_tag_rule(row))
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_enabled_tag_rules(&self) -> Result<Vec<crate::tag_rule::TagRuleRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, tag_name, match_rules, enabled, downloader_ids, tagged_torrent_count, tagged_total_size, created_at, updated_at
                     FROM tag_rules WHERE enabled = 1 ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| row_to_tag_rule(row))
                .map_err(sql_error)?;
            let mut list = Vec::new();
            for row in rows {
                list.push(row.map_err(sql_error)?);
            }
            Ok(list)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_tag_rule(&self, id: i64) -> Result<Option<crate::tag_rule::TagRuleRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, tag_name, match_rules, enabled, downloader_ids, tagged_torrent_count, tagged_total_size, created_at, updated_at
                 FROM tag_rules WHERE id = ?",
                params![id],
                |row| row_to_tag_rule(row),
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_tag_rule(&self, req: &crate::tag_rule::TagRuleRequest) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let match_rules_json = serde_json::to_string(&req.match_rules)
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let downloader_ids_json = req
                .downloader_ids
                .map(|ids| serde_json::to_string(&ids))
                .transpose()
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let enabled = req.enabled.unwrap_or(true) as i32;
            conn.execute(
                "INSERT INTO tag_rules (name, tag_name, match_rules, enabled, downloader_ids, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![req.name, req.tag_name, match_rules_json, enabled, downloader_ids_json, now, now],
            )
            .map_err(sql_error)?;
            Ok(conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_tag_rule(&self, id: i64, req: &crate::tag_rule::TagRuleRequest) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let match_rules_json = serde_json::to_string(&req.match_rules)
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let downloader_ids_json = req
                .downloader_ids
                .map(|ids| serde_json::to_string(&ids))
                .transpose()
                .map_err(|e| sql_error(rusqlite::Error::ToSqlConversionFailure(Box::new(e))))?;
            let enabled = req.enabled.unwrap_or(true) as i32;
            conn.execute(
                "UPDATE tag_rules SET name = ?, tag_name = ?, match_rules = ?, enabled = ?, downloader_ids = ?, updated_at = ?
                 WHERE id = ?",
                params![req.name, req.tag_name, match_rules_json, enabled, downloader_ids_json, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_tag_rule(&self, id: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM tag_rules WHERE id = ?", params![id])
                .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_tag_rule_enabled(&self, id: i64, enabled: bool) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE tag_rules SET enabled = ?, updated_at = ? WHERE id = ?",
                params![enabled as i32, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_tag_rule_stats(&self, id: i64, count: i64, total_size: i64) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE tag_rules SET tagged_torrent_count = ?, tagged_total_size = ?, updated_at = ? WHERE id = ?",
                params![count, total_size, now, id],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    async fn init(&self) -> Result<(), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let conn = open_connection(&path)?;
            conn.execute_batch(
                "
                PRAGMA journal_mode = WAL;
                PRAGMA foreign_keys = ON;

                CREATE TABLE IF NOT EXISTS global_settings (
                    id INTEGER PRIMARY KEY CHECK (id = 1),
                    download_rate_limit_requests INTEGER NOT NULL,
                    download_rate_limit_interval INTEGER NOT NULL,
                    download_rate_limit_unit TEXT NOT NULL,
                    retry_interval_secs INTEGER NOT NULL,
                    log_level TEXT,
                    max_concurrent_downloads INTEGER NOT NULL,
                    max_concurrent_rss_fetches INTEGER NOT NULL,
                    throttle_interval_secs INTEGER NOT NULL,
                    proxy TEXT
                );

                CREATE TABLE IF NOT EXISTS rss_subscriptions (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    url TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS download_runs (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER REFERENCES rss_subscriptions(id) ON DELETE SET NULL,
                    task_name TEXT,
                    started_at TEXT NOT NULL,
                    finished_at TEXT NOT NULL,
                    retry_delay_secs INTEGER NOT NULL,
                    total INTEGER NOT NULL,
                    succeeded INTEGER NOT NULL,
                    skipped_existing INTEGER NOT NULL,
                    failed INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS download_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    run_id INTEGER NOT NULL REFERENCES download_runs(id) ON DELETE CASCADE,
                    task_id INTEGER REFERENCES rss_subscriptions(id) ON DELETE SET NULL,
                    finished_at TEXT NOT NULL,
                    rss_name TEXT NOT NULL,
                    guid TEXT NOT NULL,
                    title TEXT NOT NULL,
                    retry_count INTEGER NOT NULL,
                    refresh_count INTEGER NOT NULL,
                    bytes INTEGER,
                    file_name TEXT,
                    saved_path TEXT,
                    final_status TEXT NOT NULL,
                    final_message TEXT,
                    file_deleted INTEGER NOT NULL DEFAULT 0
                );

                CREATE TABLE IF NOT EXISTS sites (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    site_type TEXT NOT NULL,
                    base_url TEXT NOT NULL,
                    auth_config TEXT NOT NULL,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS site_stats (
                    site_id INTEGER PRIMARY KEY REFERENCES sites(id) ON DELETE CASCADE,
                    uid TEXT,
                    username TEXT,
                    uploaded INTEGER,
                    downloaded INTEGER,
                    ratio REAL,
                    bonus REAL,
                    seeding_count INTEGER,
                    leeching_count INTEGER,
                    updated_at TEXT,
                    last_checked_at TEXT NOT NULL,
                    last_error TEXT
                );

                CREATE TABLE IF NOT EXISTS downloaders (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    downloader_type TEXT NOT NULL,
                    url TEXT NOT NULL,
                    username TEXT NOT NULL DEFAULT '',
                    password TEXT NOT NULL DEFAULT '',
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS brush_tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    cron_expression TEXT NOT NULL,
                    site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL,
                    downloader_id INTEGER NOT NULL REFERENCES downloaders(id),
                    tag TEXT NOT NULL,
                    rss_url TEXT NOT NULL,
                    seed_volume_gb REAL,
                    save_dir TEXT,
                    active_time_windows TEXT,
                    promotion TEXT NOT NULL DEFAULT 'all',
                    skip_hit_and_run INTEGER NOT NULL DEFAULT 1,
                    max_concurrent INTEGER NOT NULL DEFAULT 100,
                    download_speed_limit INTEGER,
                    upload_speed_limit INTEGER,
                    size_ranges TEXT,
                    seeder_ranges TEXT,
                    min_free_hours REAL,
                    delete_mode TEXT NOT NULL DEFAULT 'or',
                    delete_on_free_expiry INTEGER NOT NULL DEFAULT 0,
                    min_seed_time_hours REAL,
                    hr_min_seed_time_hours REAL,
                    target_ratio REAL,
                    max_upload_gb REAL,
                    download_timeout_hours REAL,
                    min_avg_upload_speed_kbs REAL,
                    max_inactive_hours REAL,
                    min_disk_space_gb REAL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS brush_task_torrents (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL REFERENCES brush_tasks(id) ON DELETE CASCADE,
                    torrent_id TEXT,
                    torrent_link TEXT,
                    torrent_hash TEXT NOT NULL,
                    torrent_name TEXT NOT NULL,
                    added_at TEXT NOT NULL,
                    size_bytes INTEGER,
                    is_hr INTEGER NOT NULL DEFAULT 0,
                    free_end_timestamp INTEGER,
                    status TEXT NOT NULL DEFAULT 'active',
                    removed_at TEXT,
                    remove_reason TEXT,
                    uploaded_bytes INTEGER NOT NULL DEFAULT 0,
                    downloaded_bytes INTEGER NOT NULL DEFAULT 0,
                    download_duration_secs INTEGER NOT NULL DEFAULT 0,
                    avg_upload_speed REAL NOT NULL DEFAULT 0,
                    ratio REAL NOT NULL DEFAULT 0,
                    last_stats_at TEXT,
                    UNIQUE(task_id, torrent_hash)
                );

                CREATE TABLE IF NOT EXISTS task_stats_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL REFERENCES brush_tasks(id) ON DELETE CASCADE,
                    total_uploaded INTEGER NOT NULL,
                    total_downloaded INTEGER NOT NULL,
                    torrent_count INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS torrent_traffic (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL,
                    torrent_hash TEXT NOT NULL,
                    uploaded_bytes INTEGER NOT NULL,
                    downloaded_bytes INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS downloader_speed_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    downloader_id INTEGER NOT NULL REFERENCES downloaders(id) ON DELETE CASCADE,
                    upload_speed INTEGER NOT NULL,
                    download_speed INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sign_in_tasks (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    site_id INTEGER NOT NULL REFERENCES sites(id) ON DELETE CASCADE,
                    cron_expression TEXT NOT NULL,
                    lightpanda_endpoint TEXT,
                    lightpanda_token TEXT NOT NULL,
                    lightpanda_region TEXT NOT NULL DEFAULT 'euwest',
                    browser TEXT NOT NULL DEFAULT 'lightpanda',
                    proxy TEXT NOT NULL DEFAULT 'fast_dc',
                    country TEXT,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    last_status TEXT,
                    last_message TEXT,
                    last_run_at TEXT,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );

                CREATE TABLE IF NOT EXISTS sign_in_records (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    task_id INTEGER NOT NULL REFERENCES sign_in_tasks(id) ON DELETE CASCADE,
                    site_id INTEGER NOT NULL,
                    site_name TEXT NOT NULL,
                    started_at TEXT NOT NULL,
                    finished_at TEXT NOT NULL,
                    status TEXT NOT NULL,
                    message TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_brush_task_torrents_task ON brush_task_torrents(task_id, status);
                CREATE INDEX IF NOT EXISTS idx_site_stats_checked_at ON site_stats(last_checked_at);
                CREATE INDEX IF NOT EXISTS idx_task_stats_task ON task_stats_snapshots(task_id, recorded_at);
                CREATE INDEX IF NOT EXISTS idx_torrent_traffic_lookup ON torrent_traffic(task_id, torrent_hash, recorded_at);
                CREATE TABLE IF NOT EXISTS system_snapshots (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    process_cpu_usage REAL NOT NULL,
                    process_memory_bytes INTEGER NOT NULL,
                    system_cpu_usage REAL NOT NULL,
                    system_total_memory_bytes INTEGER NOT NULL,
                    system_used_memory_bytes INTEGER NOT NULL,
                    system_available_memory_bytes INTEGER NOT NULL,
                    recorded_at TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_downloader_speed_snapshots_lookup ON downloader_speed_snapshots(downloader_id, recorded_at);
                CREATE INDEX IF NOT EXISTS idx_sign_in_records_lookup ON sign_in_records(task_id, finished_at);
                -- 保留期清理按 recorded_at 范围删除，需单列索引才能避免全表扫描。
                CREATE INDEX IF NOT EXISTS idx_torrent_traffic_recorded_at ON torrent_traffic(recorded_at);
                CREATE INDEX IF NOT EXISTS idx_task_stats_recorded_at ON task_stats_snapshots(recorded_at);
                CREATE INDEX IF NOT EXISTS idx_downloader_speed_recorded_at ON downloader_speed_snapshots(recorded_at);
                CREATE INDEX IF NOT EXISTS idx_system_snapshots_recorded_at ON system_snapshots(recorded_at);
                ",
            )
            .map_err(sql_error)?;

            ensure_column(
                &conn,
                "brush_task_torrents",
                "torrent_id",
                "ALTER TABLE brush_task_torrents ADD COLUMN torrent_id TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "torrent_link",
                "ALTER TABLE brush_task_torrents ADD COLUMN torrent_link TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "uploaded_bytes",
                "ALTER TABLE brush_task_torrents ADD COLUMN uploaded_bytes INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "downloaded_bytes",
                "ALTER TABLE brush_task_torrents ADD COLUMN downloaded_bytes INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "download_duration_secs",
                "ALTER TABLE brush_task_torrents ADD COLUMN download_duration_secs INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "avg_upload_speed",
                "ALTER TABLE brush_task_torrents ADD COLUMN avg_upload_speed REAL NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "ratio",
                "ALTER TABLE brush_task_torrents ADD COLUMN ratio REAL NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "last_stats_at",
                "ALTER TABLE brush_task_torrents ADD COLUMN last_stats_at TEXT",
            )?;
            ensure_column(
                &conn,
                "brush_task_torrents",
                "free_end_timestamp",
                "ALTER TABLE brush_task_torrents ADD COLUMN free_end_timestamp INTEGER",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "min_free_hours",
                "ALTER TABLE brush_tasks ADD COLUMN min_free_hours REAL",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "delete_on_free_expiry",
                "ALTER TABLE brush_tasks ADD COLUMN delete_on_free_expiry INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "site_id",
                "ALTER TABLE brush_tasks ADD COLUMN site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "brush_tasks",
                "downloader_ranges",
                "ALTER TABLE brush_tasks ADD COLUMN downloader_ranges TEXT",
            )?;
            ensure_column(
                &conn,
                "rss_subscriptions",
                "enabled",
                "ALTER TABLE rss_subscriptions ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1",
            )?;
            ensure_column(
                &conn,
                "rss_subscriptions",
                "downloader_id",
                "ALTER TABLE rss_subscriptions ADD COLUMN downloader_id INTEGER REFERENCES downloaders(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "download_runs",
                "task_id",
                "ALTER TABLE download_runs ADD COLUMN task_id INTEGER REFERENCES rss_subscriptions(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "download_runs",
                "task_name",
                "ALTER TABLE download_runs ADD COLUMN task_name TEXT",
            )?;
            ensure_column(
                &conn,
                "download_records",
                "task_id",
                "ALTER TABLE download_records ADD COLUMN task_id INTEGER REFERENCES rss_subscriptions(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "download_records",
                "file_deleted",
                "ALTER TABLE download_records ADD COLUMN file_deleted INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "proxy",
                "ALTER TABLE global_settings ADD COLUMN proxy TEXT",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "use_proxy_for_lightpanda",
                "ALTER TABLE global_settings ADD COLUMN use_proxy_for_lightpanda INTEGER NOT NULL DEFAULT 1",
            )?;
            ensure_column(
                &conn,
                "global_settings",
                "tag_rule_scan_interval_mins",
                "ALTER TABLE global_settings ADD COLUMN tag_rule_scan_interval_mins INTEGER NOT NULL DEFAULT 7",
            )?;
            ensure_column(
                &conn,
                "sites",
                "use_proxy",
                "ALTER TABLE sites ADD COLUMN use_proxy INTEGER NOT NULL DEFAULT 1",
            )?;

            conn.execute_batch(
                "CREATE TABLE IF NOT EXISTS tag_rules (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    name TEXT NOT NULL UNIQUE,
                    tag_name TEXT NOT NULL,
                    match_rules TEXT NOT NULL,
                    enabled INTEGER NOT NULL DEFAULT 1,
                    downloader_ids TEXT,
                    tagged_torrent_count INTEGER NOT NULL DEFAULT 0,
                    tagged_total_size INTEGER NOT NULL DEFAULT 0,
                    created_at TEXT NOT NULL,
                    updated_at TEXT NOT NULL
                );",
            )
            .map_err(sql_error)?;

            ensure_column(
                &conn,
                "tag_rules",
                "tagged_torrent_count",
                "ALTER TABLE tag_rules ADD COLUMN tagged_torrent_count INTEGER NOT NULL DEFAULT 0",
            )?;
            ensure_column(
                &conn,
                "tag_rules",
                "tagged_total_size",
                "ALTER TABLE tag_rules ADD COLUMN tagged_total_size INTEGER NOT NULL DEFAULT 0",
            )?;

            conn.execute(
                "INSERT OR IGNORE INTO global_settings (id, download_rate_limit_requests, download_rate_limit_interval, download_rate_limit_unit, retry_interval_secs, log_level, max_concurrent_downloads, max_concurrent_rss_fetches, throttle_interval_secs, proxy, use_proxy_for_lightpanda, tag_rule_scan_interval_mins) VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![2, 1, "second", 5, "info", 32, 8, 30, Option::<String>::None, 1, 7],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }
}

fn row_to_brush_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrushTaskRecord> {
    row_to_brush_task_at(row, 0)
}

fn row_to_brush_task_at(row: &rusqlite::Row<'_>, offset: usize) -> rusqlite::Result<BrushTaskRecord> {
    Ok(BrushTaskRecord {
        id: row.get(offset)?,
        name: row.get(offset + 1)?,
        cron_expression: row.get(offset + 2)?,
        site_id: row.get(offset + 3)?,
        downloader_id: row.get(offset + 4)?,
        tag: row.get(offset + 5)?,
        rss_url: row.get(offset + 6)?,
        seed_volume_gb: row.get(offset + 7)?,
        save_dir: row.get(offset + 8)?,
        active_time_windows: row.get(offset + 9)?,
        promotion: row.get(offset + 10)?,
        skip_hit_and_run: row.get::<_, i32>(offset + 11)? != 0,
        max_concurrent: row.get(offset + 12)?,
        download_speed_limit: row.get(offset + 13)?,
        upload_speed_limit: row.get(offset + 14)?,
        size_ranges: row.get(offset + 15)?,
        seeder_ranges: row.get(offset + 16)?,
        min_free_hours: row.get(offset + 17)?,
        delete_mode: row.get(offset + 18)?,
        delete_on_free_expiry: row.get::<_, i32>(offset + 19)? != 0,
        min_seed_time_hours: row.get(offset + 20)?,
        hr_min_seed_time_hours: row.get(offset + 21)?,
        target_ratio: row.get(offset + 22)?,
        max_upload_gb: row.get(offset + 23)?,
        download_timeout_hours: row.get(offset + 24)?,
        min_avg_upload_speed_kbs: row.get(offset + 25)?,
        max_inactive_hours: row.get(offset + 26)?,
        min_disk_space_gb: row.get(offset + 27)?,
        enabled: row.get::<_, i32>(offset + 28)? != 0,
        created_at: row.get(offset + 29)?,
        updated_at: row.get(offset + 30)?,
        downloader_ranges: row.get(offset + 31)?,
    })
}

fn insert_record(
    tx: &rusqlite::Transaction<'_>,
    run_id: i64,
    task_id: Option<i64>,
    finished_at: &str,
    record: &TorrentRunRecord,
) -> Result<(), AppError> {
    tx.execute(
        "INSERT INTO download_records (run_id, task_id, finished_at, rss_name, guid, title, retry_count, refresh_count, bytes, file_name, saved_path, final_status, final_message, file_deleted) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 0)",
        params![
            run_id,
            task_id,
            finished_at,
            record.rss_name,
            record.guid,
            record.title,
            record.retry_count as i64,
            record.refresh_count as i64,
            record.bytes.map(|v| v as i64),
            None::<String>,
            None::<String>,
            final_status_name(record.final_status),
            record.final_message,
        ],
    )
    .map_err(sql_error)?;
    Ok(())
}

fn map_rss_subscription(row: &rusqlite::Row<'_>) -> rusqlite::Result<RssSubscription> {
    Ok(RssSubscription {
        id: row.get(0)?,
        name: row.get(1)?,
        url: row.get(2)?,
        enabled: row.get::<_, i64>(3)? != 0,
        downloader_id: row.get(4)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
    })
}

fn map_sign_in_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<SignInTaskRecord> {
    Ok(SignInTaskRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        site_id: row.get(2)?,
        cron_expression: row.get(3)?,
        lightpanda_endpoint: row.get(4)?,
        lightpanda_token: row.get(5)?,
        lightpanda_region: row.get(6)?,
        browser: row.get(7)?,
        proxy: row.get(8)?,
        country: row.get(9)?,
        enabled: row.get::<_, i32>(10)? != 0,
        last_status: row.get(11)?,
        last_message: row.get(12)?,
        last_run_at: row.get(13)?,
        created_at: row.get(14)?,
        updated_at: row.get(15)?,
    })
}

fn map_sign_in_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<SignInRecord> {
    Ok(SignInRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        site_id: row.get(2)?,
        site_name: row.get(3)?,
        started_at: row.get(4)?,
        finished_at: row.get(5)?,
        status: row.get(6)?,
        message: row.get(7)?,
    })
}

fn map_history_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<DownloadHistoryRecord> {
    Ok(DownloadHistoryRecord {
        id: row.get(0)?,
        run_id: row.get(1)?,
        task_id: row.get(2)?,
        finished_at: row.get(3)?,
        rss_name: row.get(4)?,
        guid: row.get(5)?,
        title: row.get(6)?,
        retry_count: row.get::<_, i64>(7)? as u32,
        refresh_count: row.get::<_, i64>(8)? as u32,
        bytes: row.get::<_, Option<i64>>(9)?.map(|v| v as u64),
        final_status: row.get(12)?,
        final_message: row.get(13)?,
    })
}

fn map_brush_torrent_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrushTorrentRecord> {
    Ok(BrushTorrentRecord {
        id: row.get(0)?,
        task_id: row.get(1)?,
        torrent_id: row.get(2)?,
        torrent_link: row.get(3)?,
        torrent_hash: row.get(4)?,
        torrent_name: row.get(5)?,
        added_at: row.get(6)?,
        size_bytes: row.get(7)?,
        is_hr: row.get::<_, i32>(8)? != 0,
        free_end_timestamp: row.get(9)?,
        status: row.get(10)?,
        removed_at: row.get(11)?,
        remove_reason: row.get(12)?,
        uploaded_bytes: row.get(13)?,
        downloaded_bytes: row.get(14)?,
        download_duration_secs: row.get(15)?,
        avg_upload_speed: row.get(16)?,
        ratio: row.get(17)?,
        last_stats_at: row.get(18)?,
    })
}

fn row_to_tag_rule(row: &rusqlite::Row<'_>) -> rusqlite::Result<crate::tag_rule::TagRuleRecord> {
    Ok(crate::tag_rule::TagRuleRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        tag_name: row.get(2)?,
        match_rules: row.get(3)?,
        enabled: row.get::<_, i32>(4)? != 0,
        downloader_ids: row.get(5)?,
        tagged_torrent_count: row.get(6)?,
        tagged_total_size: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn open_connection(path: &Path) -> Result<Connection, AppError> {
    let conn = Connection::open(path).map_err(sql_error)?;
    // 这些 PRAGMA 是按连接生效的。由于每个操作都新开连接，必须在此逐一设置，
    // 否则 foreign_keys 会静默关闭、并发写入会立刻返回 SQLITE_BUSY。
    // - busy_timeout: 写锁争用时等待而非立即失败（高频采集 + Web 请求并发写）。
    // - foreign_keys: 启用外键级联（ON DELETE CASCADE / SET NULL）。
    // - journal_mode=WAL + synchronous=NORMAL: 读写并发，且仍有合理的持久性。
    conn.busy_timeout(std::time::Duration::from_secs(5))
        .map_err(sql_error)?;
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(sql_error)?;
    conn.pragma_update(None, "journal_mode", "WAL")
        .map_err(sql_error)?;
    conn.pragma_update(None, "synchronous", "NORMAL")
        .map_err(sql_error)?;
    Ok(conn)
}

fn join_error(error: tokio::task::JoinError) -> AppError {
    AppError::Database {
        message: format!("database task join error: {}", error),
    }
}

fn sql_error(error: rusqlite::Error) -> AppError {
    AppError::Database {
        message: error.to_string(),
    }
}

fn ensure_column(conn: &Connection, table: &str, column: &str, sql: &str) -> Result<(), AppError> {
    let mut stmt = conn
        .prepare(&format!("PRAGMA table_info({table})"))
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .map_err(sql_error)?;
    for row in rows {
        if row.map_err(sql_error)? == column {
            return Ok(());
        }
    }
    conn.execute(sql, []).map_err(sql_error)?;
    Ok(())
}

fn time_unit_name(unit: TimeUnit) -> &'static str {
    match unit {
        TimeUnit::Second => "second",
        TimeUnit::Minute => "minute",
        TimeUnit::Hour => "hour",
    }
}

fn parse_time_unit(value: String) -> TimeUnit {
    match value.as_str() {
        "minute" => TimeUnit::Minute,
        "hour" => TimeUnit::Hour,
        _ => TimeUnit::Second,
    }
}

fn clamp_u64_to_i64(value: u64) -> i64 {
    value.min(i64::MAX as u64) as i64
}

fn final_status_name(status: FinalStatus) -> &'static str {
    match status {
        FinalStatus::Success => "success",
        FinalStatus::SkippedExisting => "skipped_existing",
        FinalStatus::Failed => "failed",
    }
}
