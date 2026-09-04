use chrono::Utc;
use rusqlite::params;
use serde::{Deserialize, Serialize};

use super::{Database, join_error, open_connection, sql_error};
use crate::error::AppError;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PtdBackupConfig {
    pub enabled: bool,
    pub webdav_url: String,
    pub username: String,
    pub password: String,
    pub use_proxy: bool,
    pub backup_interval_hours: u64,
    pub last_backup_at: Option<String>,
    pub last_backup_filename: Option<String>,
    pub last_error: Option<String>,
    pub updated_at: String,
}

impl Database {
    pub async fn get_ptd_backup_config(&self) -> Result<PtdBackupConfig, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT enabled, webdav_url, username, password, use_proxy,
                        backup_interval_hours, last_backup_at,
                        last_backup_filename, last_error, updated_at
                 FROM ptd_backup_settings WHERE id = 1",
                [],
                |row| {
                    Ok(PtdBackupConfig {
                        enabled: row.get::<_, i32>(0).unwrap_or_default() != 0,
                        webdav_url: row.get(1)?,
                        username: row.get(2)?,
                        password: row.get(3)?,
                        use_proxy: row.get::<_, i32>(4).unwrap_or_default() != 0,
                        backup_interval_hours: row.get::<_, i64>(5).unwrap_or(24).max(1) as u64,
                        last_backup_at: row.get(6)?,
                        last_backup_filename: row.get(7)?,
                        last_error: row.get(8)?,
                        updated_at: row.get(9)?,
                    })
                },
            )
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_ptd_backup_config(&self, config: &PtdBackupConfig) -> Result<(), AppError> {
        let path = self.path.clone();
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE ptd_backup_settings SET
                    enabled = ?, webdav_url = ?, username = ?, password = ?, use_proxy = ?,
                    backup_interval_hours = ?, site_mappings_json = '{}', last_error = NULL,
                    updated_at = ?
                 WHERE id = 1",
                params![
                    config.enabled as i32,
                    config.webdav_url,
                    config.username,
                    config.password,
                    config.use_proxy as i32,
                    config.backup_interval_hours.min(i64::MAX as u64) as i64,
                    Utc::now().to_rfc3339(),
                ],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn record_ptd_backup_success(
        &self,
        filename: &str,
        backed_up_at: &str,
    ) -> Result<(), AppError> {
        let path = self.path.clone();
        let filename = filename.to_string();
        let backed_up_at = backed_up_at.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE ptd_backup_settings SET
                    last_backup_at = ?, last_backup_filename = ?, last_error = NULL
                 WHERE id = 1",
                params![backed_up_at, filename],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn record_ptd_backup_error(&self, message: &str) -> Result<(), AppError> {
        let path = self.path.clone();
        let message = message.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE ptd_backup_settings SET last_error = ? WHERE id = 1",
                [message],
            )
            .map_err(sql_error)?;
            Ok(())
        })
        .await
        .map_err(join_error)?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn ptd_backup_config_round_trips_without_losing_status() {
        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let mut config = db.get_ptd_backup_config().await.unwrap();
        assert!(!config.enabled);
        assert_eq!(config.backup_interval_hours, 24);

        config.enabled = true;
        config.webdav_url = "https://dav.example/ptd".to_string();
        config.username = "alice".to_string();
        config.password = "secret".to_string();
        db.update_ptd_backup_config(&config).await.unwrap();
        db.record_ptd_backup_success("PTD_backup_20260904T1200.zip", "2026-09-04T12:00:00Z")
            .await
            .unwrap();

        let saved = db.get_ptd_backup_config().await.unwrap();
        assert!(saved.enabled);
        assert_eq!(saved.password, "secret");
        assert_eq!(
            saved.last_backup_filename.as_deref(),
            Some("PTD_backup_20260904T1200.zip")
        );
        assert!(saved.last_error.is_none());
    }
}
