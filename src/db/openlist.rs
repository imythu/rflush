use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};

use super::{Database, join_error, open_connection, sql_error};
use crate::error::AppError;

static CLAIM_SEQUENCE: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenListPathMapping {
    pub id: Option<i64>,
    pub downloader_id: i64,
    pub qb_path: String,
    pub openlist_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenListTargetDirectory {
    pub id: Option<i64>,
    pub name: String,
    pub downloader_id: i64,
    #[serde(rename = "openlist_root")]
    pub openlist_path: String,
    #[serde(rename = "qb_root")]
    pub qb_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenListConfig {
    pub base_url: String,
    pub api_key: String,
    pub enabled: bool,
    pub target_directory_id: Option<i64>,
    pub selected_target_index: Option<usize>,
    pub scan_interval_secs: u64,
    pub updated_at: String,
    pub path_mappings: Vec<OpenListPathMapping>,
    pub target_directories: Vec<OpenListTargetDirectory>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaRelocationJob {
    pub id: i64,
    pub media_download_id: Option<i64>,
    pub downloader_id: Option<i64>,
    pub infohash: String,
    pub source_qb_path: String,
    pub source_openlist_path: String,
    pub source_content_openlist_path: String,
    pub target_openlist_path: String,
    pub target_qb_path: String,
    pub target_content_qb_path: String,
    pub target_downloader_id: Option<i64>,
    pub copy_items_json: String,
    pub source_files_json: String,
    pub source_manifest_json: String,
    pub copy_checkpoint_json: Option<String>,
    pub copy_lock_acquired: bool,
    pub manifest_cursor: usize,
    pub target_root_folder: Option<bool>,
    pub torrent_name: String,
    pub stage: String,
    pub openlist_task_id: Option<String>,
    pub torrent_data: Option<Vec<u8>>,
    pub attempts: u32,
    pub next_attempt_at: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<String>,
    pub version: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub completed_at: Option<String>,
}

impl Database {
    pub async fn enqueue_manual_media_relocation_jobs(
        &self,
        downloader_id: i64,
        torrents: &[(String, String)],
    ) -> Result<(usize, usize), AppError> {
        let path = self.path.clone();
        let torrents = torrents.to_vec();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let mut inserted = 0;
            let mut skipped = 0;
            for (infohash, name) in torrents {
                let active = tx
                    .query_row(
                        "SELECT 1 FROM media_relocation_jobs
                         WHERE downloader_id=? AND lower(infohash)=lower(?)
                           AND stage NOT IN ('completed', 'cancelled') LIMIT 1",
                        params![downloader_id, infohash],
                        |_| Ok(()),
                    )
                    .optional()
                    .map_err(sql_error)?
                    .is_some();
                if active {
                    skipped += 1;
                    continue;
                }
                tx.execute(
                    "INSERT INTO media_relocation_jobs
                     (media_download_id, downloader_id, infohash, source_qb_path,
                      source_openlist_path, target_openlist_path, target_qb_path,
                      torrent_name, stage, created_at, updated_at)
                     VALUES (NULL, ?, ?, '', '', '', '', ?, 'waiting_download', ?, ?)",
                    params![downloader_id, infohash, name, now, now],
                )
                .map_err(sql_error)?;
                inserted += 1;
            }
            tx.commit().map_err(sql_error)?;
            Ok((inserted, skipped))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_media_relocation_jobs(
        &self,
        limit: usize,
    ) -> Result<Vec<MediaRelocationJob>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                            source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                            stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                            lease_owner, lease_until, version, last_error, created_at, updated_at,
                            completed_at, source_content_openlist_path, target_content_qb_path,
                            target_downloader_id, copy_items_json, source_files_json,
                            target_root_folder, source_manifest_json, copy_checkpoint_json,
                            copy_lock_acquired, manifest_cursor
                     FROM media_relocation_jobs ORDER BY id DESC LIMIT ?",
                )
                .map_err(sql_error)?;
            stmt.query_map([limit.clamp(1, 500) as i64], map_media_relocation_job)
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn enqueue_submitted_media_relocation_jobs(&self) -> Result<usize, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO media_relocation_jobs
                 (media_download_id, downloader_id, infohash, source_qb_path,
                  source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                  stage, created_at, updated_at)
                 SELECT id, downloader_id, infohash, '', '', '', '', title,
                        'waiting_download', ?, ? FROM media_downloads m
                 WHERE status = 'submitted' AND length(trim(infohash)) = 40
                   AND downloader_id IS NOT NULL
                   AND NOT EXISTS (SELECT 1 FROM media_relocation_jobs j
                                   WHERE j.media_download_id = m.id)",
                params![now, now],
            )
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn claim_due_media_relocation_jobs(
        &self,
        owner: &str,
        lease_secs: i64,
        limit: usize,
    ) -> Result<Vec<MediaRelocationJob>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let until = (now + chrono::Duration::seconds(lease_secs.max(10))).to_rfc3339();
            let claim_owner = format!(
                "{owner}#{}#{}",
                now.timestamp_nanos_opt().unwrap_or_default(),
                CLAIM_SEQUENCE.fetch_add(1, Ordering::Relaxed)
            );
            tx.execute(
                "UPDATE media_relocation_jobs SET lease_owner = ?, lease_until = ?,
                     version = version + 1, updated_at = ?
                 WHERE id IN (SELECT id FROM media_relocation_jobs
                     WHERE stage NOT IN ('completed', 'cancelled', 'copy_manual_review', 'manifest_required')
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND (lease_until IS NULL OR lease_until < ?)
                     ORDER BY id LIMIT ?)",
                params![
                    claim_owner,
                    until,
                    now_text,
                    now_text,
                    now_text,
                    limit.max(1) as i64
                ],
            )
            .map_err(sql_error)?;
            let mut stmt = tx
                .prepare(
                    "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                        source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                        stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                        lease_owner, lease_until, version, last_error, created_at, updated_at,
                        completed_at, source_content_openlist_path, target_content_qb_path,
                        target_downloader_id, copy_items_json, source_files_json,
                        target_root_folder, source_manifest_json, copy_checkpoint_json,
                        copy_lock_acquired, manifest_cursor
                        FROM media_relocation_jobs WHERE lease_owner = ? ORDER BY id",
                )
                .map_err(sql_error)?;
            let jobs = stmt
                .query_map([&claim_owner], map_media_relocation_job)
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;
            drop(stmt);
            tx.commit().map_err(sql_error)?;
            Ok(jobs)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn next_media_relocation_attempt_at(&self) -> Result<Option<String>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.query_row(
                "SELECT COALESCE(next_attempt_at, ?) FROM media_relocation_jobs
                 WHERE stage NOT IN ('completed', 'cancelled', 'copy_manual_review', 'manifest_required')
                 ORDER BY CASE WHEN next_attempt_at IS NULL THEN 0 ELSE 1 END,
                          next_attempt_at
                 LIMIT 1",
                [now],
                |row| row.get(0),
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_media_relocation_job(
        &self,
        job: &MediaRelocationJob,
        expected_version: i64,
        expected_stage: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let job = job.clone();
        let expected_stage = expected_stage.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let changed = conn
                .execute(
                    "UPDATE media_relocation_jobs SET source_qb_path=?, source_openlist_path=?,
                     source_content_openlist_path=?, target_openlist_path=?, target_qb_path=?,
                     target_content_qb_path=?, target_downloader_id=?, copy_items_json=?,
                     source_files_json=?, source_manifest_json=?, copy_checkpoint_json=?,
                     copy_lock_acquired=?, manifest_cursor=?, target_root_folder=?, torrent_name=?, stage=?,
                     openlist_task_id=?, torrent_data=?, attempts=?, next_attempt_at=?,
                     lease_owner=NULL, lease_until=NULL, version=version+1, last_error=?,
                     updated_at=?, completed_at=? WHERE id=? AND version=? AND stage=?
                     AND lease_owner=? AND lease_until >= ?",
                    params![
                        job.source_qb_path,
                        job.source_openlist_path,
                        job.source_content_openlist_path,
                        job.target_openlist_path,
                        job.target_qb_path,
                        job.target_content_qb_path,
                        job.target_downloader_id,
                        job.copy_items_json,
                        job.source_files_json,
                        job.source_manifest_json,
                        job.copy_checkpoint_json,
                        job.copy_lock_acquired,
                        job.manifest_cursor as i64,
                        job.target_root_folder,
                        job.torrent_name,
                        job.stage,
                        job.openlist_task_id,
                        job.torrent_data,
                        job.attempts as i64,
                        job.next_attempt_at,
                        job.last_error,
                        Utc::now().to_rfc3339(),
                        job.completed_at,
                        job.id,
                        expected_version,
                        expected_stage,
                        job.lease_owner,
                        Utc::now().to_rfc3339()
                    ],
                )
                .map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn renew_media_relocation_lease(
        &self,
        id: i64,
        expected_version: i64,
        expected_stage: &str,
        owner: &str,
        lease_secs: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_stage = expected_stage.to_string();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + chrono::Duration::seconds(lease_secs.max(10))).to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs SET lease_until=?, updated_at=?
                 WHERE id=? AND version=? AND stage=? AND lease_owner=?
                   AND lease_until IS NOT NULL AND lease_until >= ?",
                params![
                    lease_until,
                    now_text,
                    id,
                    expected_version,
                    expected_stage,
                    owner,
                    now_text
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn checkpoint_media_relocation_copy_submission(
        &self,
        id: i64,
        expected_version: i64,
        expected_stage: &str,
        owner: &str,
        checkpoint_json: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_stage = expected_stage.to_string();
        let owner = owner.to_string();
        let checkpoint_json = checkpoint_json.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs SET copy_checkpoint_json=?, updated_at=?
                 WHERE id=? AND version=? AND stage=? AND lease_owner=?
                   AND lease_until IS NOT NULL AND lease_until >= ?",
                params![
                    checkpoint_json,
                    now,
                    id,
                    expected_version,
                    expected_stage,
                    owner,
                    now
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn try_acquire_media_relocation_target_lock(
        &self,
        id: i64,
        expected_version: i64,
        expected_stage: &str,
        owner: &str,
        target_path: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_stage = expected_stage.to_string();
        let owner = owner.to_string();
        let target_path = target_path.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let owned = tx
                .query_row(
                    "SELECT copy_lock_acquired FROM media_relocation_jobs
                     WHERE id=? AND version=? AND stage=? AND lease_owner=?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![id, expected_version, expected_stage, owner, now],
                    |row| row.get::<_, bool>(0),
                )
                .optional()
                .map_err(sql_error)?;
            let Some(owned) = owned else {
                tx.commit().map_err(sql_error)?;
                return Ok(false);
            };
            if owned {
                tx.commit().map_err(sql_error)?;
                return Ok(true);
            }

            let mut statement = tx
                .prepare(
                    "SELECT target_openlist_path FROM media_relocation_jobs
                     WHERE id != ? AND copy_lock_acquired = 1
                       AND stage IN ('copy_reconcile', 'copy_legacy_reconcile', 'copy_submitting',
                                     'copying', 'copy_succeeded', 'copy_manual_review')",
                )
                .map_err(sql_error)?;
            let locked_paths = statement
                .query_map([id], |row| row.get::<_, String>(0))
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;
            drop(statement);
            if locked_paths
                .iter()
                .any(|locked| remote_paths_overlap(locked, &target_path))
            {
                tx.commit().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_relocation_jobs SET copy_lock_acquired=1, updated_at=?
                     WHERE id=? AND version=? AND stage=? AND lease_owner=?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![now, id, expected_version, expected_stage, owner, now],
                )
                .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn record_media_relocation_retry(
        &self,
        job: &MediaRelocationJob,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let job = job.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE media_relocation_jobs SET attempts=?, next_attempt_at=?, last_error=?,
                     lease_owner=NULL, lease_until=NULL, version=version+1, updated_at=?
                 WHERE id=? AND version=? AND stage=? AND lease_owner=? AND lease_until >= ?",
                params![
                    job.attempts as i64,
                    job.next_attempt_at,
                    job.last_error,
                    now,
                    job.id,
                    job.version,
                    job.stage,
                    job.lease_owner,
                    now
                ],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn resolve_media_relocation_copy(
        &self,
        id: i64,
        resolution: &str,
        expected_version: i64,
        confirm_task_terminated: bool,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let resolution = resolution.to_string();
        tokio::task::spawn_blocking(move || {
            if matches!(resolution.as_str(), "force_retry" | "cancel") && !confirm_task_terminated {
                return Ok(false);
            }
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let (clear_checkpoint, release_lock) = match resolution.as_str() {
                "force_retry" => (true, false),
                "recheck" => (false, false),
                "cancel" => (false, true),
                _ => return Ok(false),
            };
            let changed = conn
                .execute(
                    "UPDATE media_relocation_jobs
                     SET stage=CASE
                           WHEN ?='force_retry' THEN 'copy_reconcile'
                           WHEN ?='recheck' AND copy_checkpoint_json IS NULL
                             THEN 'copy_legacy_reconcile'
                           WHEN ?='recheck' THEN 'copy_submitting'
                           ELSE 'cancelled' END,
                         openlist_task_id=CASE WHEN ? THEN NULL ELSE openlist_task_id END,
                         copy_checkpoint_json=CASE WHEN ? THEN NULL ELSE copy_checkpoint_json END,
                         copy_lock_acquired=CASE WHEN ? THEN 0 ELSE copy_lock_acquired END,
                         manifest_cursor=CASE WHEN ? THEN 0 ELSE manifest_cursor END,
                         attempts=0, next_attempt_at=CASE WHEN ? = 'cancel' THEN NULL ELSE ? END,
                         lease_owner=NULL, lease_until=NULL, version=version+1,
                         last_error=NULL, updated_at=?
                     WHERE id=? AND version=?
                       AND (stage='copy_manual_review'
                            OR (stage='copying' AND copy_checkpoint_json IS NULL)
                            OR (stage='manifest_required' AND ?='cancel'))
                       AND (lease_until IS NULL OR lease_until < ?)
                       AND (? != 'recheck' OR stage != 'manifest_required')",
                    params![
                        resolution,
                        resolution,
                        resolution,
                        clear_checkpoint,
                        clear_checkpoint,
                        release_lock,
                        clear_checkpoint,
                        resolution,
                        now,
                        now,
                        id,
                        expected_version,
                        resolution,
                        now,
                        resolution
                    ],
                )
                .map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_openlist_config(&self) -> Result<OpenListConfig, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let (base_url, api_key, enabled, target_directory_id, scan_interval_secs, updated_at) =
                conn.query_row(
                    "SELECT base_url, api_key, enabled, target_directory_id,
                            scan_interval_secs, updated_at
                     FROM openlist_settings WHERE id = 1",
                    [],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get::<_, i64>(2)? != 0,
                            row.get::<_, Option<i64>>(3)?,
                            row.get::<_, i64>(4)?.max(1) as u64,
                            row.get(5)?,
                        ))
                    },
                )
                .map_err(sql_error)?;

            let mut mapping_stmt = conn
                .prepare(
                    "SELECT id, downloader_id, qb_path, openlist_path
                     FROM openlist_path_mappings ORDER BY downloader_id, qb_path",
                )
                .map_err(sql_error)?;
            let path_mappings = mapping_stmt
                .query_map([], |row| {
                    Ok(OpenListPathMapping {
                        id: Some(row.get(0)?),
                        downloader_id: row.get(1)?,
                        qb_path: row.get(2)?,
                        openlist_path: row.get(3)?,
                    })
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;

            let mut target_stmt = conn
                .prepare(
                    "SELECT id, name, downloader_id, openlist_path, qb_path
                     FROM openlist_target_directories ORDER BY name, id",
                )
                .map_err(sql_error)?;
            let target_directories = target_stmt
                .query_map([], |row| {
                    Ok(OpenListTargetDirectory {
                        id: Some(row.get(0)?),
                        name: row.get(1)?,
                        downloader_id: row.get(2)?,
                        openlist_path: row.get(3)?,
                        qb_path: row.get(4)?,
                    })
                })
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)?;

            let selected_target_index = target_directory_id.and_then(|id| {
                target_directories
                    .iter()
                    .position(|target| target.id == Some(id))
            });
            Ok(OpenListConfig {
                base_url,
                api_key,
                enabled,
                target_directory_id,
                selected_target_index,
                scan_interval_secs,
                updated_at,
                path_mappings,
                target_directories,
            })
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_openlist_config(
        &self,
        config: &OpenListConfig,
    ) -> Result<OpenListConfig, AppError> {
        let path = self.path.clone();
        let mut config = config.clone();
        config.updated_at = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            tx.execute("DELETE FROM openlist_path_mappings", [])
                .map_err(sql_error)?;
            tx.execute("DELETE FROM openlist_target_directories", [])
                .map_err(sql_error)?;

            for mapping in &mut config.path_mappings {
                tx.execute(
                    "INSERT INTO openlist_path_mappings
                     (id, downloader_id, qb_path, openlist_path, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?)",
                    params![
                        mapping.id,
                        mapping.downloader_id,
                        mapping.qb_path,
                        mapping.openlist_path,
                        config.updated_at,
                        config.updated_at
                    ],
                )
                .map_err(sql_error)?;
                mapping.id = Some(tx.last_insert_rowid());
            }
            for target in &mut config.target_directories {
                tx.execute(
                    "INSERT INTO openlist_target_directories
                     (id, name, downloader_id, openlist_path, qb_path, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?)",
                    params![
                        target.id,
                        target.name,
                        target.downloader_id,
                        target.openlist_path,
                        target.qb_path,
                        config.updated_at,
                        config.updated_at
                    ],
                )
                .map_err(sql_error)?;
                target.id = Some(tx.last_insert_rowid());
            }

            let target_directory_id = config
                .selected_target_index
                .and_then(|index| config.target_directories.get(index))
                .and_then(|target| target.id)
                .or_else(|| {
                    config.target_directory_id.filter(|requested| {
                        config
                            .target_directories
                            .iter()
                            .any(|target| target.id == Some(*requested))
                    })
                });
            config.target_directory_id = target_directory_id;
            tx.execute(
                "UPDATE openlist_settings
                 SET base_url = ?, api_key = ?, enabled = ?, target_directory_id = ?,
                     scan_interval_secs = ?, updated_at = ? WHERE id = 1",
                params![
                    config.base_url,
                    config.api_key,
                    config.enabled as i32,
                    config.target_directory_id,
                    config.scan_interval_secs as i64,
                    config.updated_at
                ],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            Ok(config)
        })
        .await
        .map_err(join_error)?
    }

    #[allow(dead_code)]
    pub async fn get_media_relocation_job(
        &self,
        id: i64,
    ) -> Result<Option<MediaRelocationJob>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, media_download_id, downloader_id, infohash, source_qb_path,
                        source_openlist_path, target_openlist_path, target_qb_path, torrent_name,
                        stage, openlist_task_id, torrent_data, attempts, next_attempt_at,
                        lease_owner, lease_until, version, last_error, created_at, updated_at,
                        completed_at, source_content_openlist_path, target_content_qb_path,
                        target_downloader_id, copy_items_json, source_files_json,
                        target_root_folder, source_manifest_json, copy_checkpoint_json,
                        copy_lock_acquired, manifest_cursor
                 FROM media_relocation_jobs WHERE id = ?",
                [id],
                map_media_relocation_job,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }
}

fn map_media_relocation_job(row: &rusqlite::Row<'_>) -> rusqlite::Result<MediaRelocationJob> {
    Ok(MediaRelocationJob {
        id: row.get(0)?,
        media_download_id: row.get(1)?,
        downloader_id: row.get(2)?,
        infohash: row.get(3)?,
        source_qb_path: row.get(4)?,
        source_openlist_path: row.get(5)?,
        target_openlist_path: row.get(6)?,
        target_qb_path: row.get(7)?,
        torrent_name: row.get(8)?,
        stage: row.get(9)?,
        openlist_task_id: row.get(10)?,
        torrent_data: row.get(11)?,
        attempts: row.get::<_, i64>(12)?.max(0) as u32,
        next_attempt_at: row.get(13)?,
        lease_owner: row.get(14)?,
        lease_until: row.get(15)?,
        version: row.get(16)?,
        last_error: row.get(17)?,
        created_at: row.get(18)?,
        updated_at: row.get(19)?,
        completed_at: row.get(20)?,
        source_content_openlist_path: row.get(21)?,
        target_content_qb_path: row.get(22)?,
        target_downloader_id: row.get(23)?,
        copy_items_json: row.get(24)?,
        source_files_json: row.get(25)?,
        target_root_folder: row.get(26)?,
        source_manifest_json: row.get(27)?,
        copy_checkpoint_json: row.get(28)?,
        copy_lock_acquired: row.get(29)?,
        manifest_cursor: row.get::<_, i64>(30)?.max(0) as usize,
    })
}

fn remote_paths_overlap(left: &str, right: &str) -> bool {
    left == "/"
        || right == "/"
        || left == right
        || right
            .strip_prefix(left)
            .is_some_and(|suffix| suffix.starts_with('/'))
        || left
            .strip_prefix(right)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

#[cfg(test)]
mod tests {
    use super::Database;

    #[tokio::test]
    async fn relocation_schema_includes_safety_path_snapshots() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        assert!(db.list_media_relocation_jobs(10).await.unwrap().is_empty());

        let conn = super::open_connection(&dir.path().join("rflush.db")).unwrap();
        let mut statement = conn
            .prepare("PRAGMA table_info(media_relocation_jobs)")
            .unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert!(
            columns
                .iter()
                .any(|name| name == "source_content_openlist_path")
        );
        assert!(columns.iter().any(|name| name == "target_content_qb_path"));
        assert!(columns.iter().any(|name| name == "target_downloader_id"));
        assert!(columns.iter().any(|name| name == "copy_items_json"));
        assert!(columns.iter().any(|name| name == "source_files_json"));
        assert!(columns.iter().any(|name| name == "source_manifest_json"));
        assert!(columns.iter().any(|name| name == "copy_checkpoint_json"));
        assert!(columns.iter().any(|name| name == "copy_lock_acquired"));
        assert!(columns.iter().any(|name| name == "manifest_cursor"));
        assert!(columns.iter().any(|name| name == "target_root_folder"));
        let media_download_not_null: i64 = conn
            .query_row(
                "SELECT \"notnull\" FROM pragma_table_info('media_relocation_jobs')
                 WHERE name='media_download_id'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(media_download_not_null, 0);

        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, infohash, source_qb_path, source_openlist_path,
              target_openlist_path, target_qb_path, torrent_name, stage, created_at, updated_at)
             VALUES (999, '0123456789012345678901234567890123456789', '', '', '', '',
                     'schema test', 'waiting_download', ?, ?)",
            rusqlite::params![now, now],
        )
        .unwrap();
        drop(statement);
        drop(conn);

        let mut claimed = db
            .claim_due_media_relocation_jobs("schema-test", 120, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let expected_version = claimed.version;
        let expected_stage = claimed.stage.clone();
        claimed.copy_items_json = "[\"Show\"]".to_string();
        claimed.source_files_json = "[\"Show/E01.mkv\"]".to_string();
        claimed.source_manifest_json = "[{\"path\":\"Show/E01.mkv\",\"size\":123}]".to_string();
        claimed.copy_checkpoint_json =
            Some("{\"path\":\"Show/E01.mkv\",\"size\":123,\"phase\":\"prepared\"}".to_string());
        claimed.copy_lock_acquired = true;
        claimed.manifest_cursor = 1;
        claimed.target_root_folder = Some(true);
        claimed.attempts = 1;
        claimed.last_error = Some("目标 qB 提交失败: test".to_string());
        let next_attempt_at = (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339();
        claimed.next_attempt_at = Some(next_attempt_at.clone());
        assert!(
            db.update_media_relocation_job(&claimed, expected_version, &expected_stage)
                .await
                .unwrap()
        );
        let stored = db
            .get_media_relocation_job(claimed.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.copy_items_json, "[\"Show\"]");
        assert_eq!(stored.source_files_json, "[\"Show/E01.mkv\"]");
        assert_eq!(
            stored.source_manifest_json,
            "[{\"path\":\"Show/E01.mkv\",\"size\":123}]"
        );
        assert!(stored.copy_checkpoint_json.is_some());
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.manifest_cursor, 1);
        assert_eq!(stored.target_root_folder, Some(true));
        assert_eq!(stored.attempts, 1);
        assert_eq!(stored.last_error.as_deref(), Some("目标 qB 提交失败: test"));
        assert_eq!(
            stored.next_attempt_at.as_deref(),
            Some(next_attempt_at.as_str())
        );
        assert_eq!(
            db.next_media_relocation_attempt_at().await.unwrap(),
            Some(next_attempt_at)
        );
    }

    #[tokio::test]
    async fn relocation_claim_heartbeat_checkpoint_and_target_lock_are_durable() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let conn = super::open_connection(&dir.path().join("rflush.db")).unwrap();
        conn.execute_batch("PRAGMA foreign_keys=OFF;").unwrap();
        let now = chrono::Utc::now().to_rfc3339();
        for (media_download_id, target) in [(901, "/archive/show"), (902, "/archive/show/season")] {
            conn.execute(
                "INSERT INTO media_relocation_jobs
                 (media_download_id, infohash, source_qb_path, source_openlist_path,
                  target_openlist_path, target_qb_path, torrent_name, stage,
                  source_files_json, source_manifest_json, created_at, updated_at)
                 VALUES (?, '0123456789012345678901234567890123456789', '/src', '/src',
                         ?, '/target', 'lock test', 'copy_reconcile',
                         '[\"Show/E01.mkv\"]',
                         '[{\"path\":\"Show/E01.mkv\",\"size\":123}]', ?, ?)",
                rusqlite::params![media_download_id, target, now, now],
            )
            .unwrap();
        }
        drop(conn);

        let first = db
            .claim_due_media_relocation_jobs("same-worker", 120, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let second_claim = db
            .claim_due_media_relocation_jobs("same-worker", 120, 1)
            .await
            .unwrap();
        assert_eq!(
            second_claim.len(),
            1,
            "an old lease must not be selected again"
        );
        let second = &second_claim[0];
        assert_ne!(first.id, second.id);
        assert_ne!(first.lease_owner, second.lease_owner);

        let first_owner = first.lease_owner.as_deref().unwrap();
        assert!(
            db.renew_media_relocation_lease(
                first.id,
                first.version,
                &first.stage,
                first_owner,
                120,
            )
            .await
            .unwrap()
        );
        assert!(
            !db.renew_media_relocation_lease(
                first.id,
                first.version,
                &first.stage,
                "wrong-owner",
                120,
            )
            .await
            .unwrap()
        );

        assert!(
            db.try_acquire_media_relocation_target_lock(
                first.id,
                first.version,
                &first.stage,
                first_owner,
                &first.target_openlist_path,
            )
            .await
            .unwrap()
        );
        assert!(
            !db.try_acquire_media_relocation_target_lock(
                second.id,
                second.version,
                &second.stage,
                second.lease_owner.as_deref().unwrap(),
                &second.target_openlist_path,
            )
            .await
            .unwrap()
        );

        let checkpoint = "{\"path\":\"Show/E01.mkv\",\"size\":123,\"phase\":\"uncertain\",\"submitted_at\":\"2026-01-01T00:00:00Z\"}";
        assert!(
            db.checkpoint_media_relocation_copy_submission(
                first.id,
                first.version,
                &first.stage,
                first_owner,
                checkpoint,
            )
            .await
            .unwrap()
        );
        let mut retry = first.clone();
        retry.attempts = 1;
        retry.next_attempt_at = Some(now.clone());
        retry.last_error = Some("ambiguous response".to_string());
        assert!(db.record_media_relocation_retry(&retry).await.unwrap());
        let stored = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(stored.copy_checkpoint_json.as_deref(), Some(checkpoint));
        assert!(stored.copy_lock_acquired);
        assert_eq!(stored.last_error.as_deref(), Some("ambiguous response"));

        let conn = super::open_connection(&dir.path().join("rflush.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='copy_manual_review', copy_checkpoint_json=NULL,
                 source_manifest_json='[]' WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.claim_due_media_relocation_jobs("other-worker", 120, 1)
                .await
                .unwrap()
                .is_empty(),
            "manual review jobs must not be claimed"
        );
        assert!(
            !db.resolve_media_relocation_copy(first.id, "force_retry", stored.version, false)
                .await
                .unwrap(),
            "force retry requires explicit task termination confirmation"
        );
        assert!(
            !db.resolve_media_relocation_copy(first.id, "force_retry", stored.version - 1, true)
                .await
                .unwrap(),
            "force retry must reject a stale expected_version"
        );
        assert!(
            db.resolve_media_relocation_copy(first.id, "force_retry", stored.version, true)
                .await
                .unwrap()
        );
        let retried = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(retried.stage, "copy_reconcile");
        assert_eq!(retried.copy_checkpoint_json, None);
        assert!(retried.copy_lock_acquired);
        assert_eq!(retried.manifest_cursor, 0);

        let conn = super::open_connection(&dir.path().join("rflush.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='copying', copy_checkpoint_json=NULL,
                 copy_lock_acquired=1 WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.resolve_media_relocation_copy(first.id, "recheck", retried.version, false)
                .await
                .unwrap()
        );
        let legacy_recheck = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(legacy_recheck.stage, "copy_legacy_reconcile");

        let conn = super::open_connection(&dir.path().join("rflush.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage='copy_manual_review', copy_checkpoint_json=?, copy_lock_acquired=1
             WHERE id=?",
            rusqlite::params![checkpoint, first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.resolve_media_relocation_copy(first.id, "recheck", legacy_recheck.version, false)
                .await
                .unwrap()
        );
        let rechecking = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rechecking.stage, "copy_submitting");
        assert_eq!(rechecking.copy_checkpoint_json.as_deref(), Some(checkpoint));

        let conn = super::open_connection(&dir.path().join("rflush.db")).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET stage='copy_manual_review' WHERE id=?",
            [first.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            !db.resolve_media_relocation_copy(first.id, "cancel", rechecking.version, false)
                .await
                .unwrap()
        );
        assert!(
            db.resolve_media_relocation_copy(first.id, "cancel", rechecking.version, true)
                .await
                .unwrap()
        );
        let cancelled = db
            .get_media_relocation_job(first.id)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(cancelled.stage, "cancelled");
        assert!(!cancelled.copy_lock_acquired);
    }

    #[tokio::test]
    async fn manual_relocation_jobs_are_idempotent_while_active() {
        let dir = tempfile::tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let downloader_id = db
            .create_downloader("qb", "qbittorrent", "http://127.0.0.1:8080", "", "")
            .await
            .unwrap();
        let torrents = vec![(
            "eadb91a4769b1fad89e0dd3a930523e7fc5814b8".to_string(),
            "manual torrent".to_string(),
        )];

        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(downloader_id, &torrents)
                .await
                .unwrap(),
            (1, 0)
        );
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(downloader_id, &torrents)
                .await
                .unwrap(),
            (0, 1)
        );
        let jobs = db.list_media_relocation_jobs(10).await.unwrap();
        assert_eq!(jobs.len(), 1);
        assert_eq!(jobs[0].media_download_id, None);
        assert_eq!(jobs[0].torrent_name, "manual torrent");
    }
}
