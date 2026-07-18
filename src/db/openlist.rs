use chrono::Utc;
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};

use super::{Database, join_error, open_connection, sql_error};
use crate::error::AppError;

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
    pub media_download_id: i64,
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
                            target_root_folder
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
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let until = (now + chrono::Duration::seconds(lease_secs.max(10))).to_rfc3339();
            tx.execute(
                "UPDATE media_relocation_jobs SET lease_owner = ?, lease_until = ?,
                     version = version + 1, updated_at = ?
                 WHERE id IN (SELECT id FROM media_relocation_jobs
                     WHERE stage NOT IN ('completed', 'cancelled')
                       AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                       AND (lease_until IS NULL OR lease_until < ?)
                     ORDER BY id LIMIT ?)",
                params![
                    owner,
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
                        target_root_folder
                        FROM media_relocation_jobs WHERE lease_owner = ? ORDER BY id",
                )
                .map_err(sql_error)?;
            let jobs = stmt
                .query_map([&owner], map_media_relocation_job)
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
                     source_files_json=?, target_root_folder=?, torrent_name=?, stage=?,
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
                        target_root_folder
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
    })
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
        assert!(columns.iter().any(|name| name == "target_root_folder"));

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
        claimed.target_root_folder = Some(true);
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
        assert_eq!(stored.target_root_folder, Some(true));
    }
}
