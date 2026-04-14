use std::path::{Path, PathBuf};

use chrono::{Local, Utc};
use rusqlite::{Connection, OptionalExtension, params};
use serde::Serialize;

use crate::brush::{BrushTaskRecord, BrushTaskRequest, BrushTorrentRecord};
use crate::config::{GlobalConfig, RssConfig, RssSubscription, TimeUnit};
use crate::downloader::DownloaderRecord;
use crate::error::AppError;
use crate::history::{FinalStatus, RunHistory, TorrentRunRecord};
use crate::site::SiteRecord;
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
    pub file_name: Option<String>,
    pub saved_path: Option<String>,
    pub final_status: String,
    pub final_message: Option<String>,
    pub file_deleted: bool,
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

impl Database {
    pub async fn open(base_dir: &Path) -> Result<Self, AppError> {
        let data_dir = base_dir.join("data");
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
                    "SELECT download_rate_limit_requests, download_rate_limit_interval, download_rate_limit_unit, retry_interval_secs, log_level, max_concurrent_downloads, max_concurrent_rss_fetches, throttle_interval_secs FROM global_settings WHERE id = 1",
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
                "UPDATE global_settings SET download_rate_limit_requests = ?, download_rate_limit_interval = ?, download_rate_limit_unit = ?, retry_interval_secs = ?, log_level = ?, max_concurrent_downloads = ?, max_concurrent_rss_fetches = ?, throttle_interval_secs = ? WHERE id = 1",
                params![
                    settings.download_rate_limit.requests,
                    settings.download_rate_limit.interval,
                    time_unit_name(settings.download_rate_limit.unit),
                    settings.retry_interval_secs,
                    settings.log_level,
                    settings.max_concurrent_downloads,
                    settings.max_concurrent_rss_fetches,
                    settings.throttle_interval_secs
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
                    "SELECT id, name, url, enabled, created_at, updated_at FROM rss_subscriptions ORDER BY id DESC",
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
                "SELECT id, name, url, enabled, created_at, updated_at FROM rss_subscriptions WHERE id = ?",
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
            let now = Local::now().to_rfc3339();
            conn.execute(
                "INSERT INTO rss_subscriptions (name, url, enabled, created_at, updated_at) VALUES (?, ?, ?, ?, ?)",
                params![rss.name, rss.url, if enabled { 1 } else { 0 }, now, now],
            )
            .map_err(sql_error)?;
            let id = conn.last_insert_rowid();
            conn.query_row(
                "SELECT id, name, url, enabled, created_at, updated_at FROM rss_subscriptions WHERE id = ?",
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
            let now = Local::now().to_rfc3339();
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
                params![if enabled { 1 } else { 0 }, Local::now().to_rfc3339()],
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

    pub async fn mark_task_records_deleted(&self, task_ids: &[i64]) -> Result<(), AppError> {
        if task_ids.is_empty() {
            return Ok(());
        }

        let path = self.path.clone();
        let task_ids = task_ids.to_vec();
        tokio::task::spawn_blocking(move || -> Result<(), AppError> {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            for task_id in task_ids {
                tx.execute(
                    "UPDATE download_records SET file_deleted = 1 WHERE task_id = ? AND saved_path IS NOT NULL",
                    [task_id],
                )
                .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(())
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
                .prepare("SELECT id, name, site_type, base_url, auth_config, created_at, updated_at FROM sites ORDER BY id")
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([], |row| {
                    Ok(SiteRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
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
                "SELECT id, name, site_type, base_url, auth_config, created_at, updated_at FROM sites WHERE id = ?",
                params![id],
                |row| {
                    Ok(SiteRecord {
                        id: row.get(0)?,
                        name: row.get(1)?,
                        site_type: row.get(2)?,
                        base_url: row.get(3)?,
                        auth_config: row.get(4)?,
                        created_at: row.get(5)?,
                        updated_at: row.get(6)?,
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
                "INSERT INTO sites (name, site_type, base_url, auth_config, created_at, updated_at) VALUES (?, ?, ?, ?, ?, ?)",
                params![name, site_type, base_url, auth_config, now, now],
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
                "UPDATE sites SET name = ?, site_type = ?, base_url = ?, auth_config = ?, updated_at = ? WHERE id = ?",
                params![name, site_type, base_url, auth_config, now, id],
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
        let now = Local::now().to_rfc3339();
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
                     size_ranges, seeder_ranges,
                     delete_mode, min_seed_time_hours, hr_min_seed_time_hours,
                     target_ratio, max_upload_gb, download_timeout_hours,
                     min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                     enabled, created_at, updated_at
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
                 size_ranges, seeder_ranges,
                 delete_mode, min_seed_time_hours, hr_min_seed_time_hours,
                 target_ratio, max_upload_gb, download_timeout_hours,
                 min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                 enabled, created_at, updated_at
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
        let now = Local::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let promotion = req.promotion.unwrap_or_else(|| "all".to_string());
            let skip_hr = req.skip_hit_and_run.unwrap_or(true) as i32;
            let max_concurrent = req.max_concurrent.unwrap_or(100);
            let delete_mode = req.delete_mode.unwrap_or_else(|| "or".to_string());
            conn.execute(
                "INSERT INTO brush_tasks (name, cron_expression, site_id, downloader_id, tag, rss_url,
                 seed_volume_gb, save_dir, active_time_windows,
                 promotion, skip_hit_and_run, max_concurrent,
                 download_speed_limit, upload_speed_limit,
                 size_ranges, seeder_ranges,
                 delete_mode, min_seed_time_hours, hr_min_seed_time_hours,
                 target_ratio, max_upload_gb, download_timeout_hours,
                 min_avg_upload_speed_kbs, max_inactive_hours, min_disk_space_gb,
                 enabled, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 1, ?, ?)",
                params![
                    req.name, req.cron_expression, req.site_id, req.downloader_id, req.tag, req.rss_url,
                    req.seed_volume_gb, req.save_dir, req.active_time_windows,
                    promotion, skip_hr, max_concurrent,
                    req.download_speed_limit, req.upload_speed_limit,
                    req.size_ranges, req.seeder_ranges,
                    delete_mode, req.min_seed_time_hours, req.hr_min_seed_time_hours,
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
        let now = Local::now().to_rfc3339();
        let req = req.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let promotion = req.promotion.unwrap_or_else(|| "all".to_string());
            let skip_hr = req.skip_hit_and_run.unwrap_or(true) as i32;
            let max_concurrent = req.max_concurrent.unwrap_or(100);
            let delete_mode = req.delete_mode.unwrap_or_else(|| "or".to_string());
            conn.execute(
                "UPDATE brush_tasks SET name = ?, cron_expression = ?, site_id = ?, downloader_id = ?, tag = ?, rss_url = ?,
                 seed_volume_gb = ?, save_dir = ?, active_time_windows = ?,
                 promotion = ?, skip_hit_and_run = ?, max_concurrent = ?,
                 download_speed_limit = ?, upload_speed_limit = ?,
                 size_ranges = ?, seeder_ranges = ?,
                 delete_mode = ?, min_seed_time_hours = ?, hr_min_seed_time_hours = ?,
                 target_ratio = ?, max_upload_gb = ?, download_timeout_hours = ?,
                 min_avg_upload_speed_kbs = ?, max_inactive_hours = ?, min_disk_space_gb = ?,
                 updated_at = ? WHERE id = ?",
                params![
                    req.name, req.cron_expression, req.site_id, req.downloader_id, req.tag, req.rss_url,
                    req.seed_volume_gb, req.save_dir, req.active_time_windows,
                    promotion, skip_hr, max_concurrent,
                    req.download_speed_limit, req.upload_speed_limit,
                    req.size_ranges, req.seeder_ranges,
                    delete_mode, req.min_seed_time_hours, req.hr_min_seed_time_hours,
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
        let now = Local::now().to_rfc3339();
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
                "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, status, removed_at, remove_reason,
                        uploaded_bytes, downloaded_bytes, download_duration_secs, avg_upload_speed, ratio, last_stats_at
                 FROM brush_task_torrents
                 WHERE task_id = ?
                   AND (torrent_name LIKE ? OR COALESCE(torrent_id, '') LIKE ?)
                 ORDER BY CASE WHEN removed_at IS NULL THEN 0 ELSE 1 END, added_at DESC, id DESC
                 LIMIT ? OFFSET ?"
            } else {
                "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, status, removed_at, remove_reason,
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
                    "SELECT id, task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, status, removed_at, remove_reason,
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

    pub async fn add_brush_torrent(
        &self,
        task_id: i64,
        torrent_id: Option<&str>,
        torrent_link: Option<&str>,
        hash: &str,
        name: &str,
        size_bytes: Option<i64>,
        is_hr: bool,
    ) -> Result<i64, AppError> {
        let path = self.path.clone();
        let now = Local::now().to_rfc3339();
        let (torrent_id, torrent_link, hash, name) = (
            torrent_id.map(|value| value.to_string()),
            torrent_link.map(|value| value.to_string()),
            hash.to_string(),
            name.to_string(),
        );
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT OR IGNORE INTO brush_task_torrents (task_id, torrent_id, torrent_link, torrent_hash, torrent_name, added_at, size_bytes, is_hr, status) VALUES (?, ?, ?, ?, ?, ?, ?, ?, 'active')",
                params![task_id, torrent_id, torrent_link, hash, name, now, size_bytes, is_hr as i32],
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
        let now = Local::now().to_rfc3339();
        let (hash, status) = (hash.to_string(), status.to_string());
        let reason = reason.map(|s| s.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE brush_task_torrents SET status = ?, removed_at = ?, remove_reason = ? WHERE task_id = ? AND torrent_hash = ?",
                params![status, now, reason, task_id, hash],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    // ========== Stats Snapshots ==========

    pub async fn save_task_stats_snapshot(
        &self,
        task_id: i64,
        total_uploaded: i64,
        total_downloaded: i64,
        torrent_count: i64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Local::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO task_stats_snapshots (task_id, total_uploaded, total_downloaded, torrent_count, recorded_at) VALUES (?, ?, ?, ?, ?)",
                params![task_id, total_uploaded, total_downloaded, torrent_count, now],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn save_downloader_speed_snapshot(
        &self,
        downloader_id: i64,
        upload_speed: i64,
        download_speed: i64,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let now = Local::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "INSERT INTO downloader_speed_snapshots (downloader_id, upload_speed, download_speed, recorded_at) VALUES (?, ?, ?, ?)",
                params![downloader_id, upload_speed, download_speed, now],
            )
            .map_err(sql_error)?;
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
        let now = Local::now().to_rfc3339();
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
    ) -> Result<Vec<DownloaderSpeedSnapshot>, AppError> {
        let path = self.path.clone();
        let (since, until) = (since.to_string(), until.to_string());
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
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
        let since = (Local::now() - chrono::Duration::minutes(minutes)).to_rfc3339();
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
            conn.execute(
                "DELETE FROM torrent_traffic WHERE datetime(recorded_at) < datetime(?)",
                params![cutoff],
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
                    throttle_interval_secs INTEGER NOT NULL
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
                    delete_mode TEXT NOT NULL DEFAULT 'or',
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

                CREATE INDEX IF NOT EXISTS idx_brush_task_torrents_task ON brush_task_torrents(task_id, status);
                CREATE INDEX IF NOT EXISTS idx_task_stats_task ON task_stats_snapshots(task_id, recorded_at);
                CREATE INDEX IF NOT EXISTS idx_torrent_traffic_lookup ON torrent_traffic(task_id, torrent_hash, recorded_at);
                CREATE INDEX IF NOT EXISTS idx_downloader_speed_snapshots_lookup ON downloader_speed_snapshots(downloader_id, recorded_at);
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
                "brush_tasks",
                "site_id",
                "ALTER TABLE brush_tasks ADD COLUMN site_id INTEGER REFERENCES sites(id) ON DELETE SET NULL",
            )?;
            ensure_column(
                &conn,
                "rss_subscriptions",
                "enabled",
                "ALTER TABLE rss_subscriptions ADD COLUMN enabled INTEGER NOT NULL DEFAULT 1",
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

            conn.execute(
                "INSERT OR IGNORE INTO global_settings (id, download_rate_limit_requests, download_rate_limit_interval, download_rate_limit_unit, retry_interval_secs, log_level, max_concurrent_downloads, max_concurrent_rss_fetches, throttle_interval_secs) VALUES (1, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![2, 1, "second", 5, "info", 32, 8, 30],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }
}

fn row_to_brush_task(row: &rusqlite::Row<'_>) -> rusqlite::Result<BrushTaskRecord> {
    Ok(BrushTaskRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        cron_expression: row.get(2)?,
        site_id: row.get(3)?,
        downloader_id: row.get(4)?,
        tag: row.get(5)?,
        rss_url: row.get(6)?,
        seed_volume_gb: row.get(7)?,
        save_dir: row.get(8)?,
        active_time_windows: row.get(9)?,
        promotion: row.get(10)?,
        skip_hit_and_run: row.get::<_, i32>(11)? != 0,
        max_concurrent: row.get(12)?,
        download_speed_limit: row.get(13)?,
        upload_speed_limit: row.get(14)?,
        size_ranges: row.get(15)?,
        seeder_ranges: row.get(16)?,
        delete_mode: row.get(17)?,
        min_seed_time_hours: row.get(18)?,
        hr_min_seed_time_hours: row.get(19)?,
        target_ratio: row.get(20)?,
        max_upload_gb: row.get(21)?,
        download_timeout_hours: row.get(22)?,
        min_avg_upload_speed_kbs: row.get(23)?,
        max_inactive_hours: row.get(24)?,
        min_disk_space_gb: row.get(25)?,
        enabled: row.get::<_, i32>(26)? != 0,
        created_at: row.get(27)?,
        updated_at: row.get(28)?,
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
            record.file_name,
            record.saved_path,
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
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
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
        file_name: row.get(10)?,
        saved_path: row.get(11)?,
        final_status: row.get(12)?,
        final_message: row.get(13)?,
        file_deleted: row.get::<_, i64>(14)? != 0,
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
        status: row.get(9)?,
        removed_at: row.get(10)?,
        remove_reason: row.get(11)?,
        uploaded_bytes: row.get(12)?,
        downloaded_bytes: row.get(13)?,
        download_duration_secs: row.get(14)?,
        avg_upload_speed: row.get(15)?,
        ratio: row.get(16)?,
        last_stats_at: row.get(17)?,
    })
}

fn open_connection(path: &Path) -> Result<Connection, AppError> {
    Connection::open(path).map_err(sql_error)
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

fn final_status_name(status: FinalStatus) -> &'static str {
    match status {
        FinalStatus::Success => "success",
        FinalStatus::SkippedExisting => "skipped_existing",
        FinalStatus::Failed => "failed",
    }
}
