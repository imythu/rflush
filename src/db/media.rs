use std::collections::HashSet;

use chrono::{Duration, Utc};
use rusqlite::{Connection, OptionalExtension, Row, TransactionBehavior, params};

use super::{Database, join_error, open_connection, sql_error};
use crate::error::AppError;
use crate::media::domain::QualityProfile;
use crate::media::models::{
    MEDIA_DOWNLOAD_MAX_ATTEMPTS, MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS,
    MediaDownloadDeleteOutcome, MediaDownloadRecord, MediaSettings, NewMediaDownload,
    NewSubscription, QualityProfileRecord, QualityProfileRequest, SubscriptionRecord,
    SubscriptionTargetRecord, UpdateSubscription, target_key,
};
use crate::media::progression::{
    SubscriptionTargetSeed, SubscriptionTargetSeedStatus, TargetSyncResult, air_date_eligible_at,
};

impl Database {
    pub async fn get_media_settings(&self) -> Result<MediaSettings, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT tmdb_token, tmdb_language, scan_interval_mins,
                        max_search_queries, search_concurrency, updated_at
                 FROM media_settings WHERE id = 1",
                [],
                |row| {
                    Ok(MediaSettings {
                        tmdb_token: row.get(0)?,
                        tmdb_language: row.get(1)?,
                        scan_interval_mins: row.get::<_, i64>(2)?.max(1) as u64,
                        max_search_queries: row.get::<_, i64>(3)?.max(2) as usize,
                        search_concurrency: row.get::<_, i64>(4)?.max(1) as usize,
                        updated_at: row.get(5)?,
                    })
                },
            )
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_media_settings(
        &self,
        settings: &MediaSettings,
    ) -> Result<MediaSettings, AppError> {
        let path = self.path.clone();
        let mut settings = settings.clone();
        settings.tmdb_token = clean_optional(settings.tmdb_token);
        settings.tmdb_language = settings.tmdb_language.trim().to_string();
        if settings.tmdb_language.is_empty() {
            settings.tmdb_language = "zh-CN".to_string();
        }
        settings.scan_interval_mins = settings.scan_interval_mins.max(1);
        settings.max_search_queries = settings.max_search_queries.clamp(2, 32);
        settings.search_concurrency = settings.search_concurrency.clamp(1, 16);
        settings.updated_at = Utc::now().to_rfc3339();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE media_settings
                 SET tmdb_token = ?, tmdb_language = ?, scan_interval_mins = ?,
                     max_search_queries = ?, search_concurrency = ?, updated_at = ?
                 WHERE id = 1",
                params![
                    settings.tmdb_token,
                    settings.tmdb_language,
                    settings.scan_interval_mins as i64,
                    settings.max_search_queries as i64,
                    settings.search_concurrency as i64,
                    settings.updated_at,
                ],
            )
            .map_err(sql_error)?;
            Ok(settings)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_quality_profiles(&self) -> Result<Vec<QualityProfileRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let mut stmt = conn
                .prepare(
                    "SELECT id, name, resolution_order, allowed_resolutions,
                            blocked_resolutions, source_order, allowed_sources,
                            codec_order, blocked_codecs, allow_unknown_quality,
                            minimum_score, min_seeders, created_at, updated_at
                     FROM quality_profiles ORDER BY id",
                )
                .map_err(sql_error)?;
            let rows = stmt.query_map([], map_quality_profile).map_err(sql_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_quality_profile(
        &self,
        id: i64,
    ) -> Result<Option<QualityProfileRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT id, name, resolution_order, allowed_resolutions,
                        blocked_resolutions, source_order, allowed_sources,
                        codec_order, blocked_codecs, allow_unknown_quality,
                        minimum_score, min_seeders, created_at, updated_at
                 FROM quality_profiles WHERE id = ?",
                [id],
                map_quality_profile,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_quality_profile(
        &self,
        request: &QualityProfileRequest,
    ) -> Result<QualityProfileRecord, AppError> {
        validate_quality_profile(request)?;
        let path = self.path.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "INSERT INTO quality_profiles
                 (name, resolution_order, allowed_resolutions, blocked_resolutions,
                  source_order, allowed_sources, codec_order, blocked_codecs,
                  allow_unknown_quality, minimum_score, min_seeders, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    request.name.trim(),
                    json_string(&request.resolution_order)?,
                    json_string(&request.allowed_resolutions)?,
                    json_string(&request.blocked_resolutions)?,
                    json_string(&request.source_order)?,
                    json_string(&request.allowed_sources)?,
                    json_string(&request.codec_order)?,
                    json_string(&request.blocked_codecs)?,
                    request.allow_unknown_quality as i32,
                    request.minimum_score,
                    request.min_seeders,
                    now,
                    now,
                ],
            )
            .map_err(sql_error)?;
            load_quality_profile(&conn, conn.last_insert_rowid())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_quality_profile(
        &self,
        id: i64,
        request: &QualityProfileRequest,
    ) -> Result<Option<QualityProfileRecord>, AppError> {
        validate_quality_profile(request)?;
        let path = self.path.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let changed = conn
                .execute(
                    "UPDATE quality_profiles
                     SET name = ?, resolution_order = ?, allowed_resolutions = ?,
                         blocked_resolutions = ?, source_order = ?, allowed_sources = ?,
                         codec_order = ?, blocked_codecs = ?, allow_unknown_quality = ?,
                         minimum_score = ?, min_seeders = ?, updated_at = ?
                     WHERE id = ?",
                    params![
                        request.name.trim(),
                        json_string(&request.resolution_order)?,
                        json_string(&request.allowed_resolutions)?,
                        json_string(&request.blocked_resolutions)?,
                        json_string(&request.source_order)?,
                        json_string(&request.allowed_sources)?,
                        json_string(&request.codec_order)?,
                        json_string(&request.blocked_codecs)?,
                        request.allow_unknown_quality as i32,
                        request.minimum_score,
                        request.min_seeders,
                        Utc::now().to_rfc3339(),
                        id,
                    ],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                Ok(None)
            } else {
                load_quality_profile(&conn, id).map(Some)
            }
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_quality_profile(&self, id: i64) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute("DELETE FROM quality_profiles WHERE id = ?", [id])
                .map(|changed| changed > 0)
                .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn reset_quality_profiles(&self) -> Result<Vec<QualityProfileRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            tx.execute(
                "INSERT INTO quality_profiles
                 (id, name, resolution_order, allowed_resolutions, blocked_resolutions,
                  source_order, allowed_sources, codec_order, blocked_codecs,
                  allow_unknown_quality, minimum_score, min_seeders, created_at, updated_at)
                 VALUES (1, '电视剧 · 日常', '[\"1080p\",\"2160p\",\"720p\"]',
                         '[\"2160p\",\"1080p\",\"720p\"]', '[\"480p\"]',
                         '[\"WEB-DL\",\"BluRay\",\"WEBRip\"]', '[\"WEB-DL\",\"BluRay\",\"WEBRip\"]',
                         '[\"H265\",\"H264\",\"AV1\"]', '[]', 0, 65, 1, ?, ?)
                 ON CONFLICT(id) DO UPDATE SET
                   name = excluded.name, resolution_order = excluded.resolution_order,
                   allowed_resolutions = excluded.allowed_resolutions,
                   blocked_resolutions = excluded.blocked_resolutions,
                   source_order = excluded.source_order, allowed_sources = excluded.allowed_sources,
                   codec_order = excluded.codec_order, blocked_codecs = excluded.blocked_codecs,
                   allow_unknown_quality = excluded.allow_unknown_quality,
                   minimum_score = excluded.minimum_score, min_seeders = excluded.min_seeders,
                   updated_at = excluded.updated_at",
                params![now, now],
            )
            .map_err(sql_error)?;
            tx.execute("UPDATE subscriptions SET quality_profile_id = 1", [])
                .map_err(sql_error)?;
            tx.execute("DELETE FROM quality_profiles WHERE id <> 1", [])
                .map_err(sql_error)?;
            for preset in [
                (
                    "电视剧 · 4K",
                    "[\"2160p\",\"1080p\"]",
                    "[\"2160p\",\"1080p\"]",
                    "[\"720p\",\"480p\"]",
                    "[\"WEB-DL\",\"BluRay\",\"WEBRip\"]",
                    "[\"WEB-DL\",\"BluRay\",\"WEBRip\"]",
                    "[\"H265\",\"AV1\",\"H264\"]",
                    0,
                    65,
                ),
                (
                    "电影 · 收藏",
                    "[\"2160p\",\"1080p\"]",
                    "[\"2160p\",\"1080p\"]",
                    "[\"720p\",\"480p\"]",
                    "[\"REMUX\",\"BluRay\",\"WEB-DL\"]",
                    "[\"REMUX\",\"BluRay\",\"WEB-DL\"]",
                    "[\"H265\",\"AV1\",\"H264\"]",
                    0,
                    70,
                ),
                (
                    "电影 · 均衡",
                    "[\"1080p\",\"2160p\",\"720p\"]",
                    "[\"2160p\",\"1080p\",\"720p\"]",
                    "[\"480p\"]",
                    "[\"BluRay\",\"WEB-DL\",\"WEBRip\"]",
                    "[\"BluRay\",\"WEB-DL\",\"WEBRip\"]",
                    "[\"H265\",\"H264\",\"AV1\"]",
                    0,
                    65,
                ),
                (
                    "动漫 · 日常",
                    "[\"2160p\",\"1080p\",\"720p\"]",
                    "[\"2160p\",\"1080p\",\"720p\"]",
                    "[\"480p\"]",
                    "[\"BluRay\",\"WEB-DL\",\"WEBRip\"]",
                    "[\"BluRay\",\"WEB-DL\",\"WEBRip\"]",
                    "[\"H265\",\"H264\",\"AV1\"]",
                    1,
                    60,
                ),
                (
                    "动漫 · 省空间",
                    "[\"1080p\",\"720p\"]",
                    "[\"1080p\",\"720p\"]",
                    "[\"2160p\",\"480p\"]",
                    "[\"WEB-DL\",\"WEBRip\",\"BluRay\"]",
                    "[\"WEB-DL\",\"WEBRip\",\"BluRay\"]",
                    "[\"H265\",\"AV1\",\"H264\"]",
                    1,
                    55,
                ),
            ] {
                tx.execute(
                    "INSERT INTO quality_profiles
                     (name, resolution_order, allowed_resolutions, blocked_resolutions,
                      source_order, allowed_sources, codec_order, blocked_codecs,
                      allow_unknown_quality, minimum_score, min_seeders, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, '[]', ?, ?, 1, ?, ?)",
                    params![
                        preset.0, preset.1, preset.2, preset.3, preset.4, preset.5, preset.6,
                        preset.7, preset.8, now, now
                    ],
                )
                .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;

            let mut stmt = conn
                .prepare(
                    "SELECT id, name, resolution_order, allowed_resolutions,
                            blocked_resolutions, source_order, allowed_sources,
                            codec_order, blocked_codecs, allow_unknown_quality,
                            minimum_score, min_seeders, created_at, updated_at
                     FROM quality_profiles ORDER BY id",
                )
                .map_err(sql_error)?;
            stmt.query_map([], map_quality_profile)
                .map_err(sql_error)?
                .collect::<Result<Vec<_>, _>>()
                .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_subscriptions(&self) -> Result<Vec<SubscriptionRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            load_subscriptions(&conn, None)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_subscription(&self, id: i64) -> Result<Option<SubscriptionRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            load_subscription(&conn, id)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_subscription_last_run_info(
        &self,
        id: i64,
    ) -> Result<Option<String>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                "SELECT last_run_info FROM subscriptions WHERE id = ?",
                [id],
                |row| row.get(0),
            )
            .optional()
            .map(|value| value.flatten())
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn save_claimed_subscription_last_run_info(
        &self,
        id: i64,
        owner: &str,
        info: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let info = info.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE subscriptions SET last_run_info = ?
                 WHERE id = ? AND lease_owner = ?
                   AND lease_until IS NOT NULL AND lease_until >= ?",
                params![info, id, owner, now],
            )
            .map(|changed| changed > 0)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn create_subscription(
        &self,
        request: &NewSubscription,
    ) -> Result<SubscriptionRecord, AppError> {
        self.create_subscription_with_targets(request, &[], None, None)
            .await
    }

    pub async fn create_subscription_with_targets(
        &self,
        request: &NewSubscription,
        targets: &[SubscriptionTargetSeed],
        next_run_at: Option<&str>,
        initial_status: Option<&str>,
    ) -> Result<SubscriptionRecord, AppError> {
        validate_subscription(request)?;
        validate_target_seeds(request, targets)?;
        let path = self.path.clone();
        let request = request.clone();
        let targets = targets.to_vec();
        let next_run_at = next_run_at.map(str::to_string);
        let initial_status = initial_status.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let current_target = targets
                .iter()
                .find(|target| target.status != SubscriptionTargetSeedStatus::Skipped);
            let next_episode = match (request.media_type.as_str(), current_target) {
                ("movie", _) => None,
                (_, Some(target)) => Some(target.episode),
                (_, None) if targets.is_empty() => Some(request.start_episode.unwrap_or(1)),
                _ => return Err(invalid("TV target plan has no actionable target")),
            };
            let current_absolute = current_target
                .and_then(|target| target.absolute_episode)
                .or(request.absolute_episode);
            let scheduled_at = next_run_at.unwrap_or_else(|| now.clone());
            tx.execute(
                "INSERT INTO subscriptions
                 (tmdb_id, media_type, tmdb_is_animation, tmdb_genres_json, title, original_title, aliases_json, year,
                  poster_path, season, next_episode, start_episode, absolute_episode,
                  quality_profile_id, downloader_id, save_path, enabled, next_run_at,
                  last_status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
                params![
                    request.tmdb_id,
                    request.media_type,
                    request.tmdb_is_animation as i32,
                    json_string(&request.tmdb_genres)?,
                    request.title,
                    request.original_title,
                    json_string(&request.aliases)?,
                    request.year,
                    request.poster_path,
                    request.season,
                    next_episode,
                    request.start_episode,
                    current_absolute,
                    request.quality_profile_id,
                    request.downloader_id,
                    clean_optional(request.save_path),
                    request.enabled as i32,
                    scheduled_at,
                    initial_status,
                    now,
                    now,
                ],
            )
            .map_err(sql_error)?;
            let id = tx.last_insert_rowid();
            replace_subscription_sites(&tx, id, &request.site_ids)?;
            if targets.is_empty() {
                let key = target_key(
                    &request.media_type,
                    request.tmdb_id,
                    request.season,
                    next_episode,
                    current_absolute,
                );
                tx.execute(
                    "INSERT INTO subscription_targets
                     (subscription_id, target_key, season, episode, absolute_episode,
                      status, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)",
                    params![
                        id,
                        key,
                        request.season,
                        next_episode,
                        current_absolute,
                        now,
                        now,
                    ],
                )
                .map_err(sql_error)?;
            } else {
                for target in &targets {
                    tx.execute(
                        "INSERT INTO subscription_targets
                         (subscription_id, target_key, season, episode, absolute_episode,
                          air_date, status, created_at, updated_at)
                         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                        params![
                            id,
                            target.target_key,
                            target.season,
                            target.episode,
                            target.absolute_episode,
                            target.air_date,
                            target.status.as_str(),
                            now,
                            now,
                        ],
                    )
                    .map_err(sql_error)?;
                }
            }
            tx.commit().map_err(sql_error)?;
            load_subscription(&conn, id)?.ok_or_else(|| AppError::Database {
                message: "created subscription could not be loaded".to_string(),
            })
        })
        .await
        .map_err(join_error)?
    }

    pub async fn update_subscription(
        &self,
        id: i64,
        expected_version: i64,
        request: &UpdateSubscription,
    ) -> Result<Option<SubscriptionRecord>, AppError> {
        if request.site_ids.is_empty() {
            return Err(invalid("at least one site is required"));
        }
        let path = self.path.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let Some(current) = load_subscription(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            if current.version != expected_version {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            if current.media_type == "movie" && current.last_status.as_deref() == Some("completed")
            {
                return Err(invalid(
                    "a completed movie subscription cannot be reopened by editing rules",
                ));
            }
            if current.media_type != "tv" && request.reset_download_history {
                return Err(invalid(
                    "download history can only be reset while editing a TV subscription",
                ));
            }
            let key = target_key(
                &current.media_type,
                current.tmdb_id,
                request.season,
                request.next_episode,
                request.absolute_episode,
            );
            let current_key = target_key(
                &current.media_type,
                current.tmdb_id,
                current.season,
                current.next_episode,
                current.absolute_episode,
            );
            let mut existing_target_status = None;
            if current.media_type == "tv" {
                existing_target_status = tx
                    .query_row(
                        "SELECT status FROM subscription_targets
                         WHERE subscription_id = ? AND target_key = ?",
                        params![id, key],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(sql_error)?;
                let current_target_status = if key == current_key {
                    existing_target_status.clone()
                } else {
                    tx.query_row(
                        "SELECT status FROM subscription_targets
                         WHERE subscription_id = ? AND target_key = ?",
                        params![id, current_key],
                        |row| row.get::<_, String>(0),
                    )
                    .optional()
                    .map_err(sql_error)?
                };
                if key != current_key
                    && (existing_target_status.as_deref() == Some("queued")
                        || current_target_status.as_deref() == Some("queued"))
                {
                    return Err(invalid(
                        "cannot move a subscription cursor while the source or destination target is active",
                    ));
                }
                if request.reset_download_history {
                    reset_subscription_download_history(
                        &tx,
                        id,
                        request.season,
                        request.next_episode,
                        &now,
                    )?;
                } else if existing_target_status.as_deref() == Some("submitted") {
                    return Err(invalid(
                        "the selected episode was already submitted; set reset_download_history to reopen it",
                    ));
                }
            }
            let new_start_episode = if current.media_type == "tv" {
                request.next_episode
            } else {
                current.start_episode
            };
            let initial_status =
                if key == current_key && existing_target_status.as_deref() == Some("queued") {
                    "queued"
                } else if current.media_type == "tv" {
                    "awaiting_metadata"
                } else {
                    "waiting"
                };
            let changed = tx
                .execute(
                    "UPDATE subscriptions
                     SET season = ?, next_episode = ?, start_episode = ?, absolute_episode = ?,
                         quality_profile_id = ?, downloader_id = ?, save_path = ?,
                         enabled = ?, next_run_at = ?, lease_owner = NULL,
                         lease_until = NULL, last_status = ?, last_error = NULL,
                         version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ?
                       AND (lease_until IS NULL OR lease_until < ?)",
                    params![
                        request.season,
                        request.next_episode,
                        new_start_episode,
                        request.absolute_episode,
                        request.quality_profile_id,
                        request.downloader_id,
                        clean_optional(request.save_path),
                        request.enabled as i32,
                        now,
                        initial_status,
                        now,
                        id,
                        expected_version,
                        now,
                    ],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            replace_subscription_sites(&tx, id, &request.site_ids)?;
            let target_status = if current.media_type == "tv" {
                "metadata_pending"
            } else {
                "pending"
            };
            tx.execute(
                "INSERT INTO subscription_targets
                 (subscription_id, target_key, season, episode, absolute_episode,
                  status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT(subscription_id, target_key) DO UPDATE SET
                    season = excluded.season,
                    episode = excluded.episode,
                    absolute_episode = excluded.absolute_episode,
                    air_date = NULL,
                    status = CASE
                        WHEN subscription_targets.status = 'queued' THEN 'queued'
                        ELSE excluded.status
                    END,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    key,
                    request.season,
                    request.next_episode,
                    request.absolute_episode,
                    target_status,
                    now,
                    now,
                ],
            )
            .map_err(sql_error)?;
            tx.commit().map_err(sql_error)?;
            load_subscription(&conn, id)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_subscription(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "DELETE FROM subscriptions
                 WHERE id = ?1 AND version = ?2
                   AND (lease_until IS NULL OR lease_until < ?3)
                   AND NOT EXISTS(
                       SELECT 1
                       FROM media_downloads AS download
                       WHERE download.subscription_id = subscriptions.id
                         AND (
                             download.status NOT IN ('submitted', 'failed', 'cancelled')
                             OR (download.lease_until IS NOT NULL
                                 AND download.lease_until >= ?3)
                             OR EXISTS(
                                 SELECT 1 FROM subscription_targets AS target
                                 WHERE target.subscription_id = download.subscription_id
                                   AND target.target_key = download.target_key
                                   AND target.status = 'queued'
                             )
                             OR EXISTS(
                                 SELECT 1 FROM media_relocation_jobs AS relocation
                                 WHERE (
                                     relocation.media_download_id = download.id
                                     OR (
                                         download.downloader_id IS NOT NULL
                                         AND length(trim(download.infohash)) > 0
                                         AND (relocation.downloader_id = download.downloader_id
                                              OR relocation.target_downloader_id = download.downloader_id)
                                         AND lower(relocation.infohash) = lower(download.infohash)
                                     )
                                 )
                                   AND (relocation.stage NOT IN ('completed', 'cancelled')
                                        OR (relocation.lease_until IS NOT NULL
                                            AND relocation.lease_until >= ?3))
                             )
                         )
                   )",
                params![id, expected_version, Utc::now().to_rfc3339()],
            )
            .map(|changed| changed > 0)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn set_subscription_enabled(
        &self,
        id: i64,
        expected_version: i64,
        enabled: bool,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE subscriptions
                 SET enabled = ?, next_run_at = ?, lease_owner = NULL,
                     lease_until = NULL, version = version + 1, updated_at = ?
                 WHERE id = ? AND version = ?
                   AND (lease_until IS NULL OR lease_until < ?)
                   AND (? = 0 OR COALESCE(last_status, '') != 'completed')",
                params![
                    enabled as i32,
                    now,
                    now,
                    id,
                    expected_version,
                    now,
                    enabled as i32,
                ],
            )
            .map(|changed| changed > 0)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn claim_due_subscriptions(
        &self,
        owner: &str,
        lease_seconds: i64,
        limit: usize,
    ) -> Result<Vec<SubscriptionRecord>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let ids = {
                let mut stmt = tx
                    .prepare(
                        "SELECT id FROM subscriptions
                         WHERE enabled = 1 AND next_run_at <= ?
                           AND COALESCE(last_status, '') != 'completed'
                           AND (lease_until IS NULL OR lease_until < ?)
                           AND NOT EXISTS (
                               SELECT 1 FROM media_downloads
                               WHERE media_downloads.subscription_id = subscriptions.id
                                 AND media_downloads.status IN ('fetching', 'submitting', 'reconciling')
                                 AND media_downloads.lease_until IS NOT NULL
                                 AND media_downloads.lease_until >= ?
                           )
                         ORDER BY next_run_at, id LIMIT ?",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(
                        params![now_text, now_text, now_text, limit.max(1) as i64],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(sql_error)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
            };
            let mut claimed = Vec::new();
            for id in ids {
                let changed = tx
                    .execute(
                        "UPDATE subscriptions
                         SET lease_owner = ?, lease_until = ?, version = version + 1
                         WHERE id = ? AND enabled = 1
                           AND COALESCE(last_status, '') != 'completed'
                           AND (lease_until IS NULL OR lease_until < ?)
                           AND NOT EXISTS (
                               SELECT 1 FROM media_downloads
                               WHERE media_downloads.subscription_id = subscriptions.id
                                 AND media_downloads.status IN ('fetching', 'submitting', 'reconciling')
                                 AND media_downloads.lease_until IS NOT NULL
                                 AND media_downloads.lease_until >= ?
                           )",
                        params![owner, lease_until, id, now_text, now_text],
                    )
                    .map_err(sql_error)?;
                if changed > 0 {
                    if let Some(record) = load_subscription(&tx, id)? {
                        claimed.push(record);
                    }
                }
            }
            tx.commit().map_err(sql_error)?;
            Ok(claimed)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn claim_subscription(
        &self,
        id: i64,
        owner: &str,
        lease_seconds: i64,
    ) -> Result<Option<SubscriptionRecord>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let changed = tx
                .execute(
                    "UPDATE subscriptions
                     SET lease_owner = ?, lease_until = ?, next_run_at = ?,
                         version = version + 1
                     WHERE id = ? AND COALESCE(last_status, '') != 'completed'
                       AND (lease_until IS NULL OR lease_until < ?)
                       AND NOT EXISTS (
                           SELECT 1 FROM media_downloads
                           WHERE media_downloads.subscription_id = subscriptions.id
                             AND media_downloads.status IN ('fetching', 'submitting', 'reconciling')
                             AND media_downloads.lease_until IS NOT NULL
                             AND media_downloads.lease_until >= ?
                       )",
                    params![owner, lease_until, now_text, id, now_text, now_text],
                )
                .map_err(sql_error)?;
            let record = if changed > 0 {
                load_subscription(&tx, id)?
            } else {
                None
            };
            tx.commit().map_err(sql_error)?;
            Ok(record)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_active_subscription_lease_owners(&self) -> Result<Vec<String>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT lease_owner FROM subscriptions
                     WHERE lease_owner IS NOT NULL AND lease_until IS NOT NULL
                       AND lease_until >= ?
                     ORDER BY lease_owner",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([now], |row| row.get::<_, String>(0))
                .map_err(sql_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn recover_subscription_leases_for_owners(
        &self,
        owners: &[String],
    ) -> Result<usize, AppError> {
        let owners: HashSet<_> = owners.iter().cloned().collect();
        if owners.is_empty() {
            return Ok(0);
        }
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let mut recovered = 0;
            for owner in owners {
                recovered += tx
                    .execute(
                        "UPDATE subscriptions
                         SET next_run_at = CASE
                                 WHEN COALESCE(last_status, '') = 'completed' THEN next_run_at
                                 ELSE ?
                             END,
                             lease_owner = NULL, lease_until = NULL,
                             last_status = CASE
                                 WHEN COALESCE(last_status, '') = 'completed' THEN last_status
                                 ELSE 'interrupted'
                             END,
                             last_error = CASE
                                 WHEN COALESCE(last_status, '') = 'completed' THEN last_error
                                 ELSE 'scan interrupted before completion; scheduled to retry'
                             END,
                             version = version + 1, updated_at = ?
                         WHERE lease_owner = ? AND lease_until IS NOT NULL",
                        params![now, now, owner],
                    )
                    .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(recovered)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_active_media_download_lease_owners(&self) -> Result<Vec<String>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let mut stmt = conn
                .prepare(
                    "SELECT DISTINCT lease_owner FROM media_downloads
                     WHERE status IN ('fetching', 'submitting', 'reconciling')
                       AND lease_owner IS NOT NULL AND lease_until IS NOT NULL
                       AND lease_until >= ?
                     ORDER BY lease_owner",
                )
                .map_err(sql_error)?;
            let rows = stmt
                .query_map([now], |row| row.get::<_, String>(0))
                .map_err(sql_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn recover_media_download_leases_for_owners(
        &self,
        owners: &[String],
    ) -> Result<usize, AppError> {
        let owners: HashSet<_> = owners.iter().cloned().collect();
        if owners.is_empty() {
            return Ok(0);
        }
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let reconciliation_at =
                (now + Duration::seconds(MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS)).to_rfc3339();
            let now = now.to_rfc3339();
            let mut downloads = Vec::new();
            for owner in owners {
                let mut stmt = tx
                    .prepare(&format!(
                        "{} WHERE status IN ('fetching', 'submitting', 'reconciling')
                            AND lease_owner = ? AND lease_until IS NOT NULL",
                        MEDIA_DOWNLOAD_SELECT
                    ))
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map([owner], map_media_download)
                    .map_err(sql_error)?;
                downloads.extend(rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?);
            }
            let recovered =
                recover_media_download_records(&tx, &downloads, &now, &reconciliation_at)?;
            tx.commit().map_err(sql_error)?;
            Ok(recovered)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn renew_subscription_lease(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        lease_seconds: i64,
    ) -> Result<Option<i64>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let changed = conn
                .execute(
                    "UPDATE subscriptions
                     SET lease_until = ?, version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ? AND lease_owner = ?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![lease_until, now_text, id, expected_version, owner, now_text,],
                )
                .map_err(sql_error)?;
            Ok((changed > 0).then_some(expected_version.saturating_add(1)))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn refresh_claimed_subscription_aliases(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        aliases: &[String],
    ) -> Result<Option<i64>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let aliases = json_string(&aliases)?;
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            let changed = conn
                .execute(
                    "UPDATE subscriptions
                     SET aliases_json = ?, version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ? AND lease_owner = ?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![aliases, now, id, expected_version, owner, now],
                )
                .map_err(sql_error)?;
            Ok((changed > 0).then_some(expected_version.saturating_add(1)))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn finish_subscription_scan(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        next_run_at: &str,
        status: &str,
        error: Option<&str>,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let next_run_at = next_run_at.to_string();
        let status = status.to_string();
        let error = error.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            conn.execute(
                "UPDATE subscriptions
                 SET next_run_at = ?, lease_owner = NULL, lease_until = NULL,
                     last_status = ?, last_error = ?, last_run_at = ?,
                     updated_at = ?, version = version + 1
                 WHERE id = ? AND version = ? AND lease_owner = ?
                   AND lease_until IS NOT NULL AND lease_until >= ?",
                params![
                    next_run_at,
                    status,
                    error,
                    now,
                    now,
                    id,
                    expected_version,
                    owner,
                    now,
                ],
            )
            .map(|changed| changed > 0)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn release_claimed_subscription_after_error(
        &self,
        id: i64,
        owner: &str,
        claimed_target_key: &str,
        next_run_at: &str,
        error: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let claimed_target_key = claimed_target_key.to_string();
        let next_run_at = next_run_at.to_string();
        let error = error.chars().take(2_000).collect::<String>();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let Some(subscription) = load_subscription(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            };
            let now = Utc::now().to_rfc3339();
            let current_target_key = target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                subscription.next_episode,
                subscription.absolute_episode,
            );
            let business_state_advanced = subscription.last_status.as_deref() == Some("completed")
                || current_target_key != claimed_target_key;
            if business_state_advanced {
                let changed = tx
                    .execute(
                        "UPDATE subscriptions
                         SET lease_owner = NULL, lease_until = NULL,
                             updated_at = ?, version = version + 1
                         WHERE id = ? AND version = ? AND lease_owner = ?",
                        params![now, id, subscription.version, owner],
                    )
                    .map_err(sql_error)?;
                if changed == 0 {
                    tx.rollback().map_err(sql_error)?;
                    return Ok(false);
                }
                tx.commit().map_err(sql_error)?;
                return Ok(true);
            }
            let changed = tx
                .execute(
                    "UPDATE subscriptions
                     SET next_run_at = ?, lease_owner = NULL, lease_until = NULL,
                         last_status = 'error', last_error = ?, last_run_at = ?,
                         updated_at = ?, version = version + 1
                     WHERE id = ? AND version = ? AND lease_owner = ?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![
                        next_run_at,
                        error,
                        now,
                        now,
                        id,
                        subscription.version,
                        owner,
                        now,
                    ],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn sync_claimed_subscription_targets(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        targets: &[SubscriptionTargetSeed],
        tmdb_is_animation: bool,
        tmdb_genres: &[crate::media::tmdb::TmdbGenre],
    ) -> Result<Option<TargetSyncResult>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let targets = targets.to_vec();
        let tmdb_genres = json_string(&tmdb_genres.to_vec())?;
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let changed = tx
                .execute(
                    "UPDATE subscriptions
                     SET tmdb_is_animation = ?, tmdb_genres_json = ?, version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ? AND lease_owner = ?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![
                        tmdb_is_animation as i32,
                        tmdb_genres,
                        now,
                        id,
                        expected_version,
                        owner,
                        now
                    ],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            let subscription = load_subscription(&tx, id)?.ok_or_else(|| AppError::Database {
                message: format!("subscription {id} disappeared while syncing targets"),
            })?;
            validate_target_seeds_for_subscription(&subscription, &targets)?;
            if targets.is_empty() {
                return Err(invalid("TMDB target sync cannot be empty"));
            }
            for target in &targets {
                tx.execute(
                    "INSERT INTO subscription_targets
                     (subscription_id, target_key, season, episode, absolute_episode,
                      air_date, status, created_at, updated_at)
                     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)
                     ON CONFLICT(subscription_id, target_key) DO UPDATE SET
                        season = excluded.season,
                        episode = excluded.episode,
                        absolute_episode = excluded.absolute_episode,
                        air_date = excluded.air_date,
                        status = CASE
                            WHEN subscription_targets.status IN ('queued', 'submitted')
                                THEN subscription_targets.status
                            ELSE excluded.status
                        END,
                        updated_at = excluded.updated_at",
                    params![
                        id,
                        target.target_key,
                        target.season,
                        target.episode,
                        target.absolute_episode,
                        target.air_date,
                        target.status.as_str(),
                        now,
                        now,
                    ],
                )
                .map_err(sql_error)?;
            }

            let planned_keys: HashSet<_> = targets
                .iter()
                .map(|target| target.target_key.as_str())
                .collect();
            let stale_targets = {
                let mut stmt = tx
                    .prepare(
                        "SELECT target_key FROM subscription_targets
                         WHERE subscription_id = ?
                           AND status IN ('pending', 'metadata_pending')",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map([id], |row| row.get::<_, String>(0))
                    .map_err(sql_error)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
            };
            for stale_key in stale_targets {
                if !planned_keys.contains(stale_key.as_str()) {
                    tx.execute(
                        "UPDATE subscription_targets
                         SET status = 'skipped', updated_at = ?
                         WHERE subscription_id = ? AND target_key = ?
                           AND status IN ('pending', 'metadata_pending')",
                        params![now, id, stale_key],
                    )
                    .map_err(sql_error)?;
                }
            }

            let current_key = target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                subscription.next_episode,
                subscription.absolute_episode,
            );
            let current = load_subscription_target(&tx, id, &current_key)?;
            let result = TargetSyncResult {
                version: subscription.version,
                current,
            };
            tx.commit().map_err(sql_error)?;
            Ok(Some(result))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn complete_claimed_subscription(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        frontier_key: Option<&str>,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let frontier_key = frontier_key.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            if let Some(frontier_key) = frontier_key {
                tx.execute(
                    "UPDATE subscription_targets
                     SET status = 'skipped', updated_at = ?
                     WHERE subscription_id = ? AND target_key = ?
                       AND status = 'metadata_pending'",
                    params![now, id, frontier_key],
                )
                .map_err(sql_error)?;
            }
            let changed = tx
                .execute(
                    "UPDATE subscriptions
                     SET enabled = 0, next_episode = NULL, absolute_episode = NULL,
                         next_run_at = ?, lease_owner = NULL, lease_until = NULL,
                         last_status = 'completed', last_error = NULL, last_run_at = ?,
                         updated_at = ?, version = version + 1
                     WHERE id = ? AND version = ? AND lease_owner = ?
                       AND lease_until IS NOT NULL AND lease_until >= ?",
                    params![now, now, now, id, expected_version, owner, now],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_subscription_target(
        &self,
        subscription_id: i64,
        key: &str,
    ) -> Result<Option<SubscriptionTargetRecord>, AppError> {
        let path = self.path.clone();
        let key = key.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            load_subscription_target(&conn, subscription_id, &key)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn enqueue_media_download(
        &self,
        request: &NewMediaDownload,
    ) -> Result<MediaDownloadRecord, AppError> {
        let path = self.path.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let now = Utc::now().to_rfc3339();
            enqueue_media_download_row(&conn, &request, &now)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn requeue_missing_manual_media_download(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                     SET status = 'queued', attempts = 0, next_attempt_at = ?, infohash = NULL,
                         submitted_at = NULL, lease_owner = NULL, lease_until = NULL,
                         last_error = NULL, version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ? AND subscription_id IS NULL
                       AND status = 'submitted'",
                    params![now, now, id, expected_version],
                )
                .map_err(sql_error)?;
            if changed == 1 {
                tx.execute(
                    "DELETE FROM media_relocation_jobs WHERE media_download_id = ?",
                    [id],
                )
                .map_err(sql_error)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(changed == 1)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn enqueue_subscription_media_download(
        &self,
        expected_subscription_version: i64,
        request: &NewMediaDownload,
    ) -> Result<Option<MediaDownloadRecord>, AppError> {
        let subscription_id = request
            .subscription_id
            .ok_or_else(|| invalid("a linked media download requires subscription_id"))?;
        let path = self.path.clone();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let Some(subscription) = load_subscription(&tx, subscription_id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            let active_lease = subscription
                .lease_until
                .as_deref()
                .is_some_and(|lease_until| {
                    chrono::DateTime::parse_from_rfc3339(lease_until)
                        .map(|lease_until| lease_until >= now)
                        .unwrap_or(true)
                });
            let current_key = target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                subscription.next_episode,
                subscription.absolute_episode,
            );
            if subscription.version != expected_subscription_version
                || subscription.last_status.as_deref() == Some("completed")
                || active_lease
                || current_key != request.target_key
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            let Some(target) = load_subscription_target(&tx, subscription_id, &request.target_key)?
            else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            if target.status != "pending"
                || !subscription_target_is_ready(&tx, &subscription, &target, now)?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            let download = enqueue_media_download_row(&tx, &request, &now_text)?;
            if download.subscription_id != Some(subscription_id)
                || download.target_key != request.target_key
                || download.status != "queued"
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            let target_changed = tx
                .execute(
                    "UPDATE subscription_targets
                     SET status = 'queued', updated_at = ?
                     WHERE subscription_id = ? AND target_key = ? AND status = 'pending'",
                    params![now_text, subscription_id, request.target_key],
                )
                .map_err(sql_error)?;
            let subscription_changed = tx
                .execute(
                    "UPDATE subscriptions
                     SET last_status = 'queued', last_error = NULL,
                         version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ?
                       AND (lease_until IS NULL OR lease_until < ?)
                       AND COALESCE(last_status, '') != 'completed'",
                    params![
                        now_text,
                        subscription_id,
                        expected_subscription_version,
                        now_text,
                    ],
                )
                .map_err(sql_error)?;
            if target_changed != 1 || subscription_changed != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            tx.commit().map_err(sql_error)?;
            Ok(Some(download))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn enqueue_claimed_subscription_media_download(
        &self,
        expected_subscription_version: i64,
        owner: &str,
        request: &NewMediaDownload,
    ) -> Result<Option<MediaDownloadRecord>, AppError> {
        let subscription_id = request
            .subscription_id
            .ok_or_else(|| invalid("a claimed media download requires subscription_id"))?;
        let path = self.path.clone();
        let owner = owner.to_string();
        let request = request.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let Some(subscription) = load_subscription(&tx, subscription_id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            let lease_active = subscription.lease_owner.as_deref() == Some(owner.as_str())
                && subscription
                    .lease_until
                    .as_deref()
                    .and_then(|lease_until| chrono::DateTime::parse_from_rfc3339(lease_until).ok())
                    .is_some_and(|lease_until| lease_until >= now);
            let current_key = target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                subscription.next_episode,
                subscription.absolute_episode,
            );
            if subscription.version != expected_subscription_version
                || subscription.last_status.as_deref() == Some("completed")
                || !lease_active
                || current_key != request.target_key
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            let Some(target) = load_subscription_target(&tx, subscription_id, &request.target_key)?
            else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            if target.status != "pending"
                || !subscription_target_is_ready(&tx, &subscription, &target, now)?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            let download = enqueue_media_download_row(&tx, &request, &now_text)?;
            if download.subscription_id != Some(subscription_id)
                || download.target_key != request.target_key
                || download.status != "queued"
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            let target_changed = tx
                .execute(
                    "UPDATE subscription_targets
                     SET status = 'queued', updated_at = ?
                     WHERE subscription_id = ? AND target_key = ? AND status = 'pending'",
                    params![now_text, subscription_id, request.target_key],
                )
                .map_err(sql_error)?;
            if target_changed != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            tx.commit().map_err(sql_error)?;
            Ok(Some(download))
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_media_download(
        &self,
        id: i64,
    ) -> Result<Option<MediaDownloadRecord>, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            load_media_download(&conn, id)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn media_download_failed_reconciliation_allowed(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let Some(download) = load_media_download(&conn, id)? else {
                return Ok(false);
            };
            if download.version != expected_version {
                return Ok(false);
            }
            failed_reconciliation_allowed_row(&conn, &download, Utc::now())
        })
        .await
        .map_err(join_error)?
    }

    pub async fn media_download_identity_has_active_relocation(
        &self,
        id: i64,
        downloader_id: i64,
        infohash: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let infohash = infohash.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            media_download_identity_has_active_relocation_row(
                &conn,
                id,
                Some(downloader_id),
                Some(&infohash),
                &Utc::now().to_rfc3339(),
            )
        })
        .await
        .map_err(join_error)?
    }

    pub async fn reserve_media_download_redelivery(
        &self,
        id: i64,
        expected_version: i64,
        downloader_id: i64,
        infohash: &str,
        owner: &str,
        lease_seconds: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let infohash = infohash.to_ascii_lowercase();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let Some(download) = load_media_download(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            };
            if download.version != expected_version
                || download.status != "submitted"
                || download.downloader_id != Some(downloader_id)
                || !download
                    .infohash
                    .as_deref()
                    .is_some_and(|stored| stored.eq_ignore_ascii_case(&infohash))
                || lease_is_active(download.lease_until.as_deref(), now)
                || media_download_identity_has_active_relocation_row(
                    &tx,
                    download.id,
                    download.downloader_id,
                    Some(&infohash),
                    &now_text,
                )?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                     SET lease_owner = ?, lease_until = ?
                     WHERE id = ? AND version = ? AND status = 'submitted'
                       AND downloader_id = ? AND lower(infohash) = lower(?)
                       AND (lease_until IS NULL OR lease_until < ?)",
                    params![
                        owner,
                        lease_until,
                        id,
                        expected_version,
                        downloader_id,
                        infohash,
                        now_text,
                    ],
                )
                .map_err(sql_error)?;
            if changed != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn release_media_download_redelivery(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE media_downloads
                 SET lease_owner = NULL, lease_until = NULL
                 WHERE id = ? AND version = ? AND status = 'submitted'
                   AND lease_owner = ?",
                params![id, expected_version, owner],
            )
            .map(|changed| changed == 1)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn renew_media_download_redelivery(
        &self,
        id: i64,
        expected_version: i64,
        downloader_id: i64,
        infohash: &str,
        owner: &str,
        lease_seconds: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let infohash = infohash.to_ascii_lowercase();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let Some(download) = load_media_download(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            };
            if download.version != expected_version
                || download.status != "submitted"
                || download.downloader_id != Some(downloader_id)
                || download.lease_owner.as_deref() != Some(owner.as_str())
                || !lease_is_active(download.lease_until.as_deref(), now)
                || !download
                    .infohash
                    .as_deref()
                    .is_some_and(|stored| stored.eq_ignore_ascii_case(&infohash))
                || media_download_identity_has_active_relocation_row(
                    &tx,
                    download.id,
                    download.downloader_id,
                    Some(&infohash),
                    &now_text,
                )?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_downloads SET lease_until = ?
                     WHERE id = ? AND version = ? AND status = 'submitted'
                       AND downloader_id = ? AND lower(infohash) = lower(?)
                       AND lease_owner = ? AND lease_until IS NOT NULL
                       AND lease_until >= ?",
                    params![
                        lease_until,
                        id,
                        expected_version,
                        downloader_id,
                        infohash,
                        owner,
                        now_text,
                    ],
                )
                .map_err(sql_error)?;
            if changed != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn get_media_download_by_infohash(
        &self,
        downloader_id: i64,
        infohash: &str,
    ) -> Result<Option<MediaDownloadRecord>, AppError> {
        let path = self.path.clone();
        let infohash = infohash.to_ascii_lowercase();
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.query_row(
                &format!(
                    "{} WHERE downloader_id = ? AND lower(infohash) = ?",
                    MEDIA_DOWNLOAD_SELECT
                ),
                params![downloader_id, infohash],
                map_media_download,
            )
            .optional()
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_media_downloads(
        &self,
        subscription_id: Option<i64>,
        status: Option<&str>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<MediaDownloadRecord>, AppError> {
        let path = self.path.clone();
        let status = status.map(str::to_string);
        let limit = i64::try_from(limit).map_err(|_| invalid("download limit is too large"))?;
        let offset = i64::try_from(offset).map_err(|_| invalid("download offset is too large"))?;
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let sql = format!(
                "{} WHERE (?1 IS NULL OR subscription_id = ?1)
                    AND (?2 IS NULL OR status = ?2)
                 ORDER BY id DESC LIMIT ?3 OFFSET ?4",
                MEDIA_DOWNLOAD_SELECT
            );
            let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
            let rows = stmt
                .query_map(
                    params![subscription_id, status, limit, offset],
                    map_media_download,
                )
                .map_err(sql_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn list_media_downloads_before(
        &self,
        subscription_id: Option<i64>,
        status: Option<&str>,
        limit: usize,
        before_id: i64,
    ) -> Result<Vec<MediaDownloadRecord>, AppError> {
        let path = self.path.clone();
        let status = status.map(str::to_string);
        let limit = i64::try_from(limit).map_err(|_| invalid("download limit is too large"))?;
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            let sql = format!(
                "{} WHERE (?1 IS NULL OR subscription_id = ?1)
                    AND (?2 IS NULL OR status = ?2)
                    AND id < ?3
                 ORDER BY id DESC LIMIT ?4",
                MEDIA_DOWNLOAD_SELECT
            );
            let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
            let rows = stmt
                .query_map(
                    params![subscription_id, status, before_id, limit],
                    map_media_download,
                )
                .map_err(sql_error)?;
            rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn delete_media_download_record(
        &self,
        id: i64,
        expected_version: i64,
    ) -> Result<MediaDownloadDeleteOutcome, AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let Some(download) = load_media_download(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(MediaDownloadDeleteOutcome::NotFound);
            };
            if download.version != expected_version {
                tx.rollback().map_err(sql_error)?;
                return Ok(MediaDownloadDeleteOutcome::VersionChanged);
            }
            if !matches!(download.status.as_str(), "submitted" | "failed" | "cancelled")
                || lease_is_active(download.lease_until.as_deref(), now)
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(MediaDownloadDeleteOutcome::DownloadActive);
            }

            let relocation_active =
                media_download_has_active_relocation_row(&tx, &download, &now_text)?;
            if relocation_active {
                tx.rollback().map_err(sql_error)?;
                return Ok(MediaDownloadDeleteOutcome::RelocationActive);
            }

            let mut target_reopened = false;
            let mut reopen = None;
            if let Some(subscription_id) = download.subscription_id {
                let canonical_key = format!("subscription:{subscription_id}:{}", download.target_key);
                if let Some(target) =
                    load_subscription_target(&tx, subscription_id, &download.target_key)?
                {
                    if target.status == "queued" {
                        tx.rollback().map_err(sql_error)?;
                        return Ok(MediaDownloadDeleteOutcome::DownloadActive);
                    }
                    if download.dedupe_key == canonical_key && target.status == "submitted" {
                            let target_relocation_active = tx
                                .query_row(
                                    "SELECT EXISTS(
                                         SELECT 1
                                         FROM media_downloads AS history
                                         WHERE history.subscription_id = ?1
                                           AND history.target_key = ?2
                                           AND EXISTS(
                                               SELECT 1
                                               FROM media_relocation_jobs AS relocation
                                               WHERE (
                                                   relocation.media_download_id = history.id
                                                   OR (
                                                       history.downloader_id IS NOT NULL
                                                       AND length(trim(history.infohash)) > 0
                                                       AND (relocation.downloader_id = history.downloader_id
                                                            OR relocation.target_downloader_id = history.downloader_id)
                                                       AND lower(relocation.infohash) = lower(history.infohash)
                                                   )
                                               )
                                                 AND (relocation.stage NOT IN ('completed', 'cancelled')
                                                      OR (relocation.lease_until IS NOT NULL
                                                          AND relocation.lease_until >= ?3))
                                           )
                                     )",
                                    params![subscription_id, download.target_key, now_text],
                                    |row| row.get::<_, bool>(0),
                                )
                                .map_err(sql_error)?;
                            if target_relocation_active {
                                tx.rollback().map_err(sql_error)?;
                                return Ok(MediaDownloadDeleteOutcome::RelocationActive);
                            }
                            let Some(subscription) = load_subscription(&tx, subscription_id)? else {
                                tx.rollback().map_err(sql_error)?;
                                return Ok(MediaDownloadDeleteOutcome::VersionChanged);
                            };
                            let other_work_active = tx
                                .query_row(
                                    "SELECT EXISTS(
                                         SELECT 1 FROM media_downloads
                                         WHERE subscription_id = ? AND id != ?
                                           AND (status NOT IN ('submitted', 'failed', 'cancelled')
                                                OR (lease_until IS NOT NULL AND lease_until >= ?))
                                     )",
                                    params![subscription_id, id, now_text],
                                    |row| row.get::<_, bool>(0),
                                )
                                .map_err(sql_error)?;
                            if lease_is_active(subscription.lease_until.as_deref(), now)
                                || other_work_active
                            {
                                tx.rollback().map_err(sql_error)?;
                                return Ok(MediaDownloadDeleteOutcome::SubscriptionActive);
                            }
                            reopen = Some((subscription, target));
                    }
                }
            }

            let deleted = tx
                .execute(
                    "DELETE FROM media_downloads
                     WHERE id = ? AND version = ?
                       AND status IN ('submitted', 'failed', 'cancelled')
                       AND (lease_until IS NULL OR lease_until < ?)",
                    params![id, expected_version, now_text],
                )
                .map_err(sql_error)?;
            if deleted != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(MediaDownloadDeleteOutcome::VersionChanged);
            }

            if let Some((subscription, target)) = reopen {
                let target_status = if subscription.media_type == "tv" {
                    "metadata_pending"
                } else {
                    "pending"
                };
                let target_changed = tx
                    .execute(
                        "UPDATE subscription_targets
                         SET status = ?, air_date = CASE WHEN ? = 'tv' THEN NULL ELSE air_date END,
                             updated_at = ?
                         WHERE subscription_id = ? AND target_key = ? AND status = 'submitted'",
                        params![
                            target_status,
                            subscription.media_type,
                            now_text,
                            subscription.id,
                            target.target_key,
                        ],
                    )
                    .map_err(sql_error)?;
                let subscription_changed = tx
                    .execute(
                        "UPDATE subscriptions
                         SET season = ?, next_episode = ?,
                             start_episode = CASE WHEN media_type = 'tv' THEN ? ELSE start_episode END,
                             absolute_episode = ?, next_run_at = ?,
                             lease_owner = NULL, lease_until = NULL,
                             last_status = ?, last_error = NULL, last_run_info = NULL,
                             version = version + 1, updated_at = ?
                         WHERE id = ? AND version = ?
                           AND (lease_until IS NULL OR lease_until < ?)",
                        params![
                            target.season,
                            target.episode,
                            target.episode,
                            target.absolute_episode,
                            now_text,
                            if subscription.media_type == "tv" {
                                "awaiting_metadata"
                            } else {
                                "waiting"
                            },
                            now_text,
                            subscription.id,
                            subscription.version,
                            now_text,
                        ],
                    )
                    .map_err(sql_error)?;
                if target_changed != 1 || subscription_changed != 1 {
                    tx.rollback().map_err(sql_error)?;
                    return Ok(MediaDownloadDeleteOutcome::VersionChanged);
                }
                target_reopened = true;
            }

            tx.commit().map_err(sql_error)?;
            Ok(MediaDownloadDeleteOutcome::Deleted {
                download,
                target_reopened,
            })
        })
        .await
        .map_err(join_error)?
    }

    pub async fn resolve_verified_failed_media_download(
        &self,
        id: i64,
        expected_version: i64,
        expected_downloader_id: i64,
        expected_downloader_updated_at: &str,
        torrent_present: bool,
    ) -> Result<Option<MediaDownloadRecord>, AppError> {
        let path = self.path.clone();
        let expected_downloader_updated_at = expected_downloader_updated_at.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let Some(download) = load_media_download(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            let Some(subscription_id) = download.subscription_id else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            if download.version != expected_version
                || download.status != "failed"
                || download.infohash.is_none()
                || download.downloader_id != Some(expected_downloader_id)
                || lease_is_active(download.lease_until.as_deref(), now)
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            let downloader_unchanged = tx
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM downloaders
                         WHERE id = ? AND updated_at = ?
                     )",
                    params![expected_downloader_id, expected_downloader_updated_at],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_error)?;
            if !downloader_unchanged {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }
            let Some(target) =
                load_subscription_target(&tx, subscription_id, &download.target_key)?
            else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            let Some(subscription) = load_subscription(&tx, subscription_id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            };
            let current_key = target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                subscription.next_episode,
                subscription.absolute_episode,
            );
            let other_work_active = tx
                .query_row(
                    "SELECT EXISTS(
                         SELECT 1 FROM media_downloads
                         WHERE subscription_id = ? AND id != ?
                           AND (status NOT IN ('submitted', 'failed', 'cancelled')
                                OR (lease_until IS NOT NULL AND lease_until >= ?))
                     )",
                    params![subscription_id, id, now_text],
                    |row| row.get::<_, bool>(0),
                )
                .map_err(sql_error)?;
            let relocation_active =
                media_download_has_active_relocation_row(&tx, &download, &now_text)?;
            if target.status != "queued"
                || current_key != download.target_key
                || lease_is_active(subscription.lease_until.as_deref(), now)
                || other_work_active
                || relocation_active
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            let message = if torrent_present {
                "qB verification confirmed the torrent is present"
            } else {
                "qB verification confirmed the torrent is absent; ready to retry"
            };
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                     SET status = CASE WHEN ? THEN 'submitted' ELSE status END,
                         next_attempt_at = NULL, lease_owner = NULL, lease_until = NULL,
                         last_error = ?, submitted_at = CASE WHEN ? THEN ? ELSE submitted_at END,
                         version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ? AND status = 'failed'
                       AND (lease_until IS NULL OR lease_until < ?)",
                    params![
                        torrent_present,
                        message,
                        torrent_present,
                        now_text,
                        now_text,
                        id,
                        expected_version,
                        now_text,
                    ],
                )
                .map_err(sql_error)?;
            if changed != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(None);
            }

            if torrent_present {
                let target_changed = tx
                    .execute(
                        "UPDATE subscription_targets SET status = 'submitted', updated_at = ?
                         WHERE subscription_id = ? AND target_key = ? AND status = 'queued'",
                        params![now_text, subscription_id, download.target_key],
                    )
                    .map_err(sql_error)?;
                if target_changed != 1 {
                    tx.rollback().map_err(sql_error)?;
                    return Ok(None);
                }
                advance_subscription_after_submit(
                    &tx,
                    subscription_id,
                    &download.target_key,
                    &now_text,
                )?;
            } else {
                reset_subscription_after_failed_download(&tx, &download, message, &now_text)?;
                if subscription_target_has_status(
                    &tx,
                    subscription_id,
                    &download.target_key,
                    "queued",
                )? {
                    tx.rollback().map_err(sql_error)?;
                    return Ok(None);
                }
            }

            let resolved = load_media_download(&tx, id)?;
            tx.commit().map_err(sql_error)?;
            Ok(resolved)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn claim_due_media_downloads(
        &self,
        owner: &str,
        lease_seconds: i64,
        limit: usize,
    ) -> Result<Vec<MediaDownloadRecord>, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let ids = {
                let mut stmt = tx
                    .prepare(
                        "SELECT media_downloads.id FROM media_downloads
                         WHERE ((status IN ('queued', 'retry_wait') AND attempts < ?)
                                OR (status = 'reconciling' AND attempts <= ?))
                           AND (next_attempt_at IS NULL OR next_attempt_at <= ?)
                           AND (lease_until IS NULL OR lease_until < ?)
                           AND (
                               media_downloads.subscription_id IS NULL OR EXISTS (
                                   SELECT 1 FROM subscription_targets
                                   WHERE subscription_targets.subscription_id = media_downloads.subscription_id
                                     AND subscription_targets.target_key = media_downloads.target_key
                                     AND subscription_targets.status = 'queued'
                               )
                           )
                           AND NOT EXISTS (
                               SELECT 1 FROM subscriptions
                               WHERE subscriptions.id = media_downloads.subscription_id
                                 AND subscriptions.lease_until IS NOT NULL
                                 AND subscriptions.lease_until >= ?
                           )
                         ORDER BY media_downloads.id LIMIT ?",
                    )
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map(
                        params![
                            MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                            MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                            now_text,
                            now_text,
                            now_text,
                            limit.max(1) as i64,
                        ],
                        |row| row.get::<_, i64>(0),
                    )
                    .map_err(sql_error)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
            };
            let mut claimed = Vec::new();
            for id in ids {
                let changed = tx
                    .execute(
                        "UPDATE media_downloads
                         SET status = CASE WHEN status = 'reconciling' THEN status ELSE 'fetching' END,
                             attempts = CASE
                                 WHEN status = 'reconciling' AND attempts >= ? THEN attempts
                                 ELSE attempts + 1
                             END,
                             lease_owner = ?, lease_until = ?, version = version + 1,
                             updated_at = ?
                         WHERE id = ?
                            AND ((status IN ('queued', 'retry_wait') AND attempts < ?)
                                 OR (status = 'reconciling' AND attempts <= ?))
                            AND (lease_until IS NULL OR lease_until < ?)
                            AND (
                                media_downloads.subscription_id IS NULL OR EXISTS (
                                    SELECT 1 FROM subscription_targets
                                    WHERE subscription_targets.subscription_id = media_downloads.subscription_id
                                      AND subscription_targets.target_key = media_downloads.target_key
                                      AND subscription_targets.status = 'queued'
                                )
                            )
                            AND NOT EXISTS (
                               SELECT 1 FROM subscriptions
                               WHERE subscriptions.id = media_downloads.subscription_id
                                 AND subscriptions.lease_until IS NOT NULL
                                 AND subscriptions.lease_until >= ?
                           )",
                        params![
                            MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                            owner,
                            lease_until,
                            now_text,
                            id,
                            MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                            MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                            now_text,
                            now_text,
                        ],
                    )
                    .map_err(sql_error)?;
                if changed > 0 {
                    if let Some(record) = load_media_download(&tx, id)? {
                        claimed.push(record);
                    }
                }
            }
            tx.commit().map_err(sql_error)?;
            Ok(claimed)
        })
        .await
        .map_err(join_error)?
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn transition_media_download(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        expected_status: &str,
        new_status: &str,
        infohash: Option<&str>,
        last_error: Option<&str>,
        next_attempt_at: Option<&str>,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let expected_status = expected_status.to_string();
        let owner = owner.to_string();
        let new_status = new_status.to_string();
        let infohash = infohash.map(|value| value.to_ascii_lowercase());
        let last_error = last_error.map(str::to_string);
        let next_attempt_at = next_attempt_at.map(str::to_string);
        tokio::task::spawn_blocking(move || {
            let conn = open_connection(&path)?;
            conn.execute(
                "UPDATE media_downloads
                 SET status = ?, infohash = COALESCE(?, infohash), last_error = ?,
                     next_attempt_at = ?, lease_owner = NULL, lease_until = NULL,
                     version = version + 1, updated_at = ?
                 WHERE id = ? AND version = ? AND status = ? AND lease_owner = ?",
                params![
                    new_status,
                    infohash,
                    last_error,
                    next_attempt_at,
                    Utc::now().to_rfc3339(),
                    id,
                    expected_version,
                    expected_status,
                    owner,
                ],
            )
            .map(|changed| changed > 0)
            .map_err(sql_error)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn release_media_download_after_error(
        &self,
        id: i64,
        owner: &str,
        error: &str,
        submission_confirmed_absent: bool,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let error = error.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn.transaction().map_err(sql_error)?;
            let Some(download) = load_media_download(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            };
            if download.lease_owner.as_deref() != Some(owner.as_str())
                || !matches!(
                    download.status.as_str(),
                    "fetching" | "submitting" | "reconciling"
                )
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }

            let preserve_reconciliation = !submission_confirmed_absent
                && matches!(download.status.as_str(), "submitting" | "reconciling");
            let status = if download.status == "submitting" && preserve_reconciliation {
                "reconciling"
            } else if download.attempts >= MEDIA_DOWNLOAD_MAX_ATTEMPTS {
                "failed"
            } else if preserve_reconciliation {
                "reconciling"
            } else {
                "retry_wait"
            };
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let delay_seconds = 30_i64.saturating_mul(1_i64 << download.attempts.min(6));
            let next_attempt_at = (status != "failed")
                .then(|| (now + Duration::seconds(delay_seconds.min(3600))).to_rfc3339());
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                     SET status = ?, last_error = ?, next_attempt_at = ?,
                         lease_owner = NULL, lease_until = NULL,
                         version = version + 1, updated_at = ?
                     WHERE id = ? AND version = ? AND status = ? AND lease_owner = ?",
                    params![
                        status,
                        error,
                        next_attempt_at,
                        now_text,
                        id,
                        download.version,
                        download.status,
                        owner,
                    ],
                )
                .map_err(sql_error)?;
            let submission_known_absent =
                submission_confirmed_absent || download.status == "fetching";
            if changed > 0 && status == "failed" && submission_known_absent {
                reset_subscription_after_failed_download(&tx, &download, &error, &now_text)?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(changed > 0)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn mark_media_download_submitting(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        infohash: &str,
        lease_seconds: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let infohash = infohash.to_ascii_lowercase();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now();
            let now_text = now.to_rfc3339();
            let lease_until = (now + Duration::seconds(lease_seconds.max(10))).to_rfc3339();
            let Some(download) = load_media_download(&tx, id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            };
            if download.version != expected_version
                || download.status != "fetching"
                || download.lease_owner.as_deref() != Some(owner.as_str())
                || download.downloader_id.is_none()
                || media_download_identity_has_active_relocation_row(
                    &tx,
                    download.id,
                    download.downloader_id,
                    Some(&infohash),
                    &now_text,
                )?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                 SET status = 'submitting', infohash = ?, lease_until = ?,
                     version = version + 1, updated_at = ?
                 WHERE id = ? AND version = ? AND status = 'fetching'
                   AND lease_owner = ?",
                    params![infohash, lease_until, now_text, id, expected_version, owner,],
                )
                .map_err(sql_error)?;
            if changed != 1 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn mark_media_download_submitted(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        infohash: &str,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        let infohash = infohash.to_ascii_lowercase();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let download = load_media_download(&tx, id)?.ok_or_else(|| AppError::Database {
                message: format!("media download {id} not found"),
            })?;
            if download.version != expected_version
                || !matches!(download.status.as_str(), "submitting" | "reconciling")
                || download.lease_owner.as_deref() != Some(owner.as_str())
                || media_download_identity_has_active_relocation_row(
                    &tx,
                    download.id,
                    download.downloader_id,
                    Some(&infohash),
                    &now,
                )?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            if let Some(subscription_id) = download.subscription_id
                && !subscription_target_has_status(
                    &tx,
                    subscription_id,
                    &download.target_key,
                    "queued",
                )?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                 SET status = 'submitted', infohash = ?, lease_owner = NULL,
                     lease_until = NULL, version = version + 1, last_error = NULL,
                     updated_at = ?, submitted_at = ?
                 WHERE id = ? AND version = ? AND lease_owner = ?
                   AND status IN ('submitting', 'reconciling')",
                    params![infohash, now, now, id, expected_version, owner],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            if let Some(subscription_id) = download.subscription_id {
                let target_changed = tx
                    .execute(
                        "UPDATE subscription_targets SET status = 'submitted', updated_at = ?
                     WHERE subscription_id = ? AND target_key = ? AND status = 'queued'",
                        params![now, subscription_id, download.target_key],
                    )
                    .map_err(sql_error)?;
                if target_changed != 1 {
                    tx.rollback().map_err(sql_error)?;
                    return Ok(false);
                }
                advance_subscription_after_submit(
                    &tx,
                    subscription_id,
                    &download.target_key,
                    &now,
                )?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn mark_media_download_duplicate_submitted(
        &self,
        id: i64,
        expected_version: i64,
        owner: &str,
        existing_download_id: i64,
    ) -> Result<bool, AppError> {
        let path = self.path.clone();
        let owner = owner.to_string();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let download = load_media_download(&tx, id)?.ok_or_else(|| AppError::Database {
                message: format!("media download {id} not found"),
            })?;
            if download.version != expected_version
                || download.status != "fetching"
                || download.lease_owner.as_deref() != Some(owner.as_str())
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            let Some(existing) = load_media_download(&tx, existing_download_id)? else {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            };
            if existing.status != "submitted"
                || existing.downloader_id.is_none()
                || existing.downloader_id != download.downloader_id
                || existing.infohash.is_none()
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            if media_download_identity_has_active_relocation_row(
                &tx,
                download.id,
                download.downloader_id,
                existing.infohash.as_deref(),
                &now,
            )? {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            if let Some(subscription_id) = download.subscription_id
                && !subscription_target_has_status(
                    &tx,
                    subscription_id,
                    &download.target_key,
                    "queued",
                )?
            {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            let changed = tx
                .execute(
                    "UPDATE media_downloads
                     SET status = 'submitted', lease_owner = NULL, lease_until = NULL,
                         version = version + 1, last_error = ?, updated_at = ?, submitted_at = ?
                     WHERE id = ? AND version = ? AND status = 'fetching'
                       AND lease_owner = ?",
                    params![
                        format!(
                            "torrent already submitted by media download {existing_download_id}"
                        ),
                        now,
                        now,
                        id,
                        expected_version,
                        owner,
                    ],
                )
                .map_err(sql_error)?;
            if changed == 0 {
                tx.rollback().map_err(sql_error)?;
                return Ok(false);
            }
            if let Some(subscription_id) = download.subscription_id {
                let target_changed = tx
                    .execute(
                        "UPDATE subscription_targets SET status = 'submitted', updated_at = ?
                     WHERE subscription_id = ? AND target_key = ? AND status = 'queued'",
                        params![now, subscription_id, download.target_key],
                    )
                    .map_err(sql_error)?;
                if target_changed != 1 {
                    tx.rollback().map_err(sql_error)?;
                    return Ok(false);
                }
                advance_subscription_after_submit(
                    &tx,
                    subscription_id,
                    &download.target_key,
                    &now,
                )?;
            }
            tx.commit().map_err(sql_error)?;
            Ok(true)
        })
        .await
        .map_err(join_error)?
    }

    pub async fn recover_expired_media_leases(&self) -> Result<(usize, usize), AppError> {
        let path = self.path.clone();
        tokio::task::spawn_blocking(move || {
            let mut conn = open_connection(&path)?;
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(sql_error)?;
            let now = Utc::now().to_rfc3339();
            let expired_downloads = {
                let mut stmt = tx
                    .prepare(&format!(
                        "{} WHERE status IN ('fetching', 'submitting', 'reconciling')
                            AND lease_until IS NOT NULL AND lease_until < ?",
                        MEDIA_DOWNLOAD_SELECT
                    ))
                    .map_err(sql_error)?;
                let rows = stmt
                    .query_map([now.as_str()], map_media_download)
                    .map_err(sql_error)?;
                rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
            };
            let subscriptions = tx
                .execute(
                    "UPDATE subscriptions
                     SET lease_owner = NULL, lease_until = NULL, version = version + 1,
                         updated_at = ?
                     WHERE lease_until IS NOT NULL AND lease_until < ?",
                    params![now, now],
                )
                .map_err(sql_error)?;
            let downloads = recover_media_download_records(&tx, &expired_downloads, &now, &now)?;
            tx.commit().map_err(sql_error)?;
            Ok((subscriptions, downloads))
        })
        .await
        .map_err(join_error)?
    }
}

fn recover_media_download_records(
    conn: &Connection,
    downloads: &[MediaDownloadRecord],
    now: &str,
    reconciliation_at: &str,
) -> Result<usize, AppError> {
    let mut recovered = 0;
    for download in downloads {
        let changed = conn
            .execute(
                "UPDATE media_downloads
                 SET status = CASE
                         WHEN status = 'submitting' THEN 'reconciling'
                         WHEN attempts >= ? THEN 'failed'
                         WHEN status = 'reconciling' THEN 'reconciling'
                         ELSE 'retry_wait'
                     END,
                     lease_owner = NULL, lease_until = NULL,
                     next_attempt_at = CASE
                         WHEN status = 'submitting' THEN ?
                         WHEN status != 'submitting' AND attempts >= ? THEN NULL
                         ELSE ?
                     END,
                     version = version + 1, updated_at = ?
                 WHERE id = ? AND version = ? AND lease_owner = ?
                   AND status = ? AND lease_until IS NOT NULL",
                params![
                    MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                    reconciliation_at,
                    MEDIA_DOWNLOAD_MAX_ATTEMPTS,
                    now,
                    now,
                    download.id,
                    download.version,
                    download.lease_owner,
                    download.status,
                ],
            )
            .map_err(sql_error)?;
        if changed == 0 {
            continue;
        }
        recovered += changed;
        if download.status == "fetching" && download.attempts >= MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            reset_subscription_after_failed_download(
                conn,
                download,
                "download delivery failed after lease recovery",
                now,
            )?;
        }
    }
    Ok(recovered)
}

const SUBSCRIPTION_SELECT: &str =
    "SELECT id, tmdb_id, media_type, tmdb_is_animation, tmdb_genres_json, title, original_title, aliases_json, year,
            poster_path, season, next_episode, start_episode, absolute_episode,
            quality_profile_id, downloader_id, save_path, enabled, next_run_at,
            lease_owner, lease_until, version, last_status, last_error, last_run_at,
            created_at, updated_at FROM subscriptions";

const MEDIA_DOWNLOAD_SELECT: &str =
    "SELECT id, subscription_id, target_key, dedupe_key, site_id, downloader_id,
            source_site, downloader_name, torrent_id, title, size, release_json,
            decision_json, profile_snapshot_json, infohash, status, attempts,
            next_attempt_at, lease_owner, lease_until, version, last_error,
            created_at, updated_at, submitted_at FROM media_downloads";

fn load_quality_profile(conn: &Connection, id: i64) -> Result<QualityProfileRecord, AppError> {
    conn.query_row(
        "SELECT id, name, resolution_order, allowed_resolutions,
                blocked_resolutions, source_order, allowed_sources, codec_order,
                blocked_codecs, allow_unknown_quality, minimum_score, min_seeders,
                created_at, updated_at FROM quality_profiles WHERE id = ?",
        [id],
        map_quality_profile,
    )
    .map_err(sql_error)
}

fn map_quality_profile(row: &Row<'_>) -> rusqlite::Result<QualityProfileRecord> {
    Ok(QualityProfileRecord {
        id: row.get(0)?,
        name: row.get(1)?,
        resolution_order: parse_json_column(row.get(2)?, 2)?,
        allowed_resolutions: parse_json_column(row.get(3)?, 3)?,
        blocked_resolutions: parse_json_column(row.get(4)?, 4)?,
        source_order: parse_json_column(row.get(5)?, 5)?,
        allowed_sources: parse_json_column(row.get(6)?, 6)?,
        codec_order: parse_json_column(row.get(7)?, 7)?,
        blocked_codecs: parse_json_column(row.get(8)?, 8)?,
        allow_unknown_quality: row.get::<_, i32>(9)? != 0,
        minimum_score: row.get(10)?,
        min_seeders: row.get(11)?,
        created_at: row.get(12)?,
        updated_at: row.get(13)?,
    })
}

fn load_subscriptions(
    conn: &Connection,
    only_id: Option<i64>,
) -> Result<Vec<SubscriptionRecord>, AppError> {
    let sql = match only_id {
        Some(_) => format!("{} WHERE id = ?", SUBSCRIPTION_SELECT),
        None => format!("{} ORDER BY id DESC", SUBSCRIPTION_SELECT),
    };
    let mut records = {
        let mut stmt = conn.prepare(&sql).map_err(sql_error)?;
        let rows = match only_id {
            Some(id) => stmt.query_map([id], map_subscription).map_err(sql_error)?,
            None => stmt.query_map([], map_subscription).map_err(sql_error)?,
        };
        rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)?
    };
    for record in &mut records {
        record.site_ids = load_subscription_site_ids(conn, record.id)?;
    }
    Ok(records)
}

fn load_subscription(conn: &Connection, id: i64) -> Result<Option<SubscriptionRecord>, AppError> {
    Ok(load_subscriptions(conn, Some(id))?.into_iter().next())
}

fn map_subscription(row: &Row<'_>) -> rusqlite::Result<SubscriptionRecord> {
    Ok(SubscriptionRecord {
        id: row.get(0)?,
        tmdb_id: row.get(1)?,
        media_type: row.get(2)?,
        tmdb_is_animation: row.get::<_, i32>(3)? != 0,
        tmdb_genres: parse_json_column(row.get(4)?, 4)?,
        title: row.get(5)?,
        original_title: row.get(6)?,
        aliases: parse_json_column(row.get(7)?, 7)?,
        year: row.get(8)?,
        poster_path: row.get(9)?,
        season: row.get(10)?,
        next_episode: row.get(11)?,
        start_episode: row.get(12)?,
        absolute_episode: row.get(13)?,
        quality_profile_id: row.get(14)?,
        downloader_id: row.get(15)?,
        save_path: row.get(16)?,
        enabled: row.get::<_, i32>(17)? != 0,
        next_run_at: row.get(18)?,
        lease_owner: row.get(19)?,
        lease_until: row.get(20)?,
        version: row.get(21)?,
        last_status: row.get(22)?,
        last_error: row.get(23)?,
        last_run_at: row.get(24)?,
        created_at: row.get(25)?,
        updated_at: row.get(26)?,
        site_ids: Vec::new(),
    })
}

fn load_subscription_site_ids(conn: &Connection, id: i64) -> Result<Vec<i64>, AppError> {
    let mut stmt = conn
        .prepare(
            "SELECT site_id FROM subscription_sites
             WHERE subscription_id = ? ORDER BY priority, site_id",
        )
        .map_err(sql_error)?;
    let rows = stmt
        .query_map([id], |row| row.get::<_, i64>(0))
        .map_err(sql_error)?;
    rows.collect::<Result<Vec<_>, _>>().map_err(sql_error)
}

fn replace_subscription_sites(
    conn: &Connection,
    subscription_id: i64,
    site_ids: &[i64],
) -> Result<(), AppError> {
    conn.execute(
        "DELETE FROM subscription_sites WHERE subscription_id = ?",
        [subscription_id],
    )
    .map_err(sql_error)?;
    for (priority, site_id) in site_ids.iter().copied().enumerate() {
        conn.execute(
            "INSERT INTO subscription_sites (subscription_id, site_id, priority)
             VALUES (?, ?, ?)",
            params![subscription_id, site_id, priority as i64],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

fn map_subscription_target(row: &Row<'_>) -> rusqlite::Result<SubscriptionTargetRecord> {
    Ok(SubscriptionTargetRecord {
        id: row.get(0)?,
        subscription_id: row.get(1)?,
        target_key: row.get(2)?,
        season: row.get(3)?,
        episode: row.get(4)?,
        absolute_episode: row.get(5)?,
        air_date: row.get(6)?,
        status: row.get(7)?,
        created_at: row.get(8)?,
        updated_at: row.get(9)?,
    })
}

fn load_subscription_target(
    conn: &Connection,
    subscription_id: i64,
    key: &str,
) -> Result<Option<SubscriptionTargetRecord>, AppError> {
    conn.query_row(
        "SELECT id, subscription_id, target_key, season, episode,
                absolute_episode, air_date, status, created_at, updated_at
         FROM subscription_targets
         WHERE subscription_id = ? AND target_key = ?",
        params![subscription_id, key],
        map_subscription_target,
    )
    .optional()
    .map_err(sql_error)
}

fn subscription_target_has_status(
    conn: &Connection,
    subscription_id: i64,
    key: &str,
    status: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM subscription_targets
             WHERE subscription_id = ? AND target_key = ? AND status = ?
         )",
        params![subscription_id, key, status],
        |row| row.get(0),
    )
    .map_err(sql_error)
}

fn subscription_target_is_ready(
    conn: &Connection,
    subscription: &SubscriptionRecord,
    target: &SubscriptionTargetRecord,
    now: chrono::DateTime<Utc>,
) -> Result<bool, AppError> {
    if let Some(air_date) = target.air_date.as_deref() {
        return Ok(air_date_eligible_at(air_date).is_some_and(|eligible_at| eligible_at <= now));
    }
    if subscription.media_type == "movie" {
        return Ok(true);
    }
    let Some(episode) = target.episode else {
        return Ok(false);
    };
    if episode == u32::MAX {
        return Ok(true);
    }
    let (skipped_frontier, metadata_frontier) = conn
        .query_row(
            "SELECT
                 EXISTS(
                     SELECT 1 FROM subscription_targets
                     WHERE subscription_id = ?1 AND season IS ?2 AND episode > ?3
                       AND status = 'skipped'
                 ),
                 EXISTS(
                     SELECT 1 FROM subscription_targets
                     WHERE subscription_id = ?1 AND season IS ?2 AND episode > ?3
                       AND status = 'metadata_pending'
                 )",
            params![subscription.id, target.season, episode],
            |row| Ok((row.get::<_, bool>(0)?, row.get::<_, bool>(1)?)),
        )
        .map_err(sql_error)?;
    Ok(skipped_frontier && !metadata_frontier)
}

fn media_download_has_active_relocation_row(
    conn: &Connection,
    download: &MediaDownloadRecord,
    now: &str,
) -> Result<bool, AppError> {
    media_download_identity_has_active_relocation_row(
        conn,
        download.id,
        download.downloader_id,
        download.infohash.as_deref(),
        now,
    )
}

fn failed_reconciliation_allowed_row(
    conn: &Connection,
    download: &MediaDownloadRecord,
    now: chrono::DateTime<Utc>,
) -> Result<bool, AppError> {
    let Some(subscription_id) = download.subscription_id else {
        return Ok(false);
    };
    let Some(downloader_id) = download.downloader_id else {
        return Ok(false);
    };
    let Some(infohash) = download.infohash.as_deref() else {
        return Ok(false);
    };
    if download.status != "failed"
        || !matches!(infohash.len(), 40 | 64)
        || !infohash.bytes().all(|byte| byte.is_ascii_hexdigit())
        || lease_is_active(download.lease_until.as_deref(), now)
    {
        return Ok(false);
    }
    let Some(subscription) = load_subscription(conn, subscription_id)? else {
        return Ok(false);
    };
    let Some(target) = load_subscription_target(conn, subscription_id, &download.target_key)?
    else {
        return Ok(false);
    };
    let current_key = target_key(
        &subscription.media_type,
        subscription.tmdb_id,
        subscription.season,
        subscription.next_episode,
        subscription.absolute_episode,
    );
    if target.status != "queued"
        || current_key != download.target_key
        || lease_is_active(subscription.lease_until.as_deref(), now)
    {
        return Ok(false);
    }
    let now_text = now.to_rfc3339();
    let downloader_exists = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM downloaders WHERE id = ?)",
            [downloader_id],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;
    let other_work_active = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM media_downloads
                 WHERE subscription_id = ? AND id != ?
                   AND (status NOT IN ('submitted', 'failed', 'cancelled')
                        OR (lease_until IS NOT NULL AND lease_until >= ?))
             )",
            params![subscription_id, download.id, now_text],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;
    Ok(downloader_exists
        && !other_work_active
        && !media_download_has_active_relocation_row(conn, download, &now_text)?)
}

fn media_download_identity_has_active_relocation_row(
    conn: &Connection,
    download_id: i64,
    downloader_id: Option<i64>,
    infohash: Option<&str>,
    now: &str,
) -> Result<bool, AppError> {
    conn.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM media_relocation_jobs AS relocation
             WHERE (
                 relocation.media_download_id = ?1
                 OR (
                     ?2 IS NOT NULL
                     AND length(trim(?3)) > 0
                     AND (relocation.downloader_id = ?2
                          OR relocation.target_downloader_id = ?2)
                     AND lower(relocation.infohash) = lower(?3)
                 )
             )
               AND (relocation.stage NOT IN ('completed', 'cancelled')
                    OR (relocation.lease_until IS NOT NULL AND relocation.lease_until >= ?4))
         )",
        params![download_id, downloader_id, infohash, now],
        |row| row.get::<_, bool>(0),
    )
    .map_err(sql_error)
}

fn reset_subscription_download_history(
    conn: &Connection,
    subscription_id: i64,
    season: Option<u32>,
    start_episode: Option<u32>,
    now: &str,
) -> Result<(), AppError> {
    let (Some(season), Some(start_episode)) = (season, start_episode) else {
        return Err(invalid(
            "season and next episode are required to reset TV download history",
        ));
    };
    let unresolved_target = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM subscription_targets
                 WHERE subscription_id = ? AND season IS ? AND episode >= ?
                   AND status = 'queued'
             )",
            params![subscription_id, season, start_episode],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;
    if unresolved_target {
        return Err(invalid(
            "cannot reset download history while download work is active or a target submission is unresolved",
        ));
    }
    let active_download = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM media_downloads AS download
                 JOIN subscription_targets AS target
                   ON target.subscription_id = download.subscription_id
                  AND target.target_key = download.target_key
                 WHERE download.subscription_id = ?
                   AND target.season IS ? AND target.episode >= ?
                   AND (download.status NOT IN ('submitted', 'failed', 'cancelled')
                        OR (download.lease_until IS NOT NULL AND download.lease_until >= ?))
             )",
            params![subscription_id, season, start_episode, now],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;
    if active_download {
        return Err(invalid(
            "cannot reset download history while download work is active",
        ));
    }

    let active_relocation = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1
                 FROM media_downloads AS download
                 JOIN subscription_targets AS target
                   ON target.subscription_id = download.subscription_id
                  AND target.target_key = download.target_key
                 WHERE download.subscription_id = ?1
                   AND target.season IS ?2 AND target.episode >= ?3
                   AND EXISTS(
                       SELECT 1
                       FROM media_relocation_jobs AS relocation
                       WHERE (
                           relocation.media_download_id = download.id
                           OR (
                               download.downloader_id IS NOT NULL
                               AND length(trim(download.infohash)) > 0
                               AND (relocation.downloader_id = download.downloader_id
                                    OR relocation.target_downloader_id = download.downloader_id)
                               AND lower(relocation.infohash) = lower(download.infohash)
                           )
                       )
                         AND (relocation.stage NOT IN ('completed', 'cancelled')
                              OR (relocation.lease_until IS NOT NULL
                                  AND relocation.lease_until >= ?4))
                   )
             )",
            params![subscription_id, season, start_episode, now],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;
    if active_relocation {
        return Err(invalid(
            "cannot reset download history while relocation jobs are active",
        ));
    }

    conn.execute(
        "DELETE FROM media_downloads
         WHERE subscription_id = ?
           AND target_key IN (
               SELECT target_key FROM subscription_targets
               WHERE subscription_id = ? AND season IS ? AND episode >= ?
           )",
        params![subscription_id, subscription_id, season, start_episode],
    )
    .map_err(sql_error)?;
    conn.execute(
        "UPDATE subscription_targets
         SET status = CASE WHEN air_date IS NULL THEN 'metadata_pending' ELSE 'pending' END,
             updated_at = ?
         WHERE subscription_id = ? AND season IS ? AND episode >= ?
           AND status = 'submitted'",
        params![now, subscription_id, season, start_episode],
    )
    .map_err(sql_error)?;
    Ok(())
}

fn lease_is_active(lease_until: Option<&str>, now: chrono::DateTime<Utc>) -> bool {
    lease_until.is_some_and(|lease_until| {
        chrono::DateTime::parse_from_rfc3339(lease_until)
            .map(|lease_until| lease_until >= now)
            .unwrap_or(true)
    })
}

fn enqueue_media_download_row(
    conn: &Connection,
    request: &NewMediaDownload,
    now: &str,
) -> Result<MediaDownloadRecord, AppError> {
    conn.execute(
        "INSERT INTO media_downloads
         (subscription_id, target_key, dedupe_key, site_id, downloader_id,
          source_site, downloader_name, torrent_id, title, size, release_json,
          decision_json, profile_snapshot_json, status, next_attempt_at,
          created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, 'queued', ?, ?, ?)
         ON CONFLICT(dedupe_key) DO UPDATE SET
            site_id = excluded.site_id,
            downloader_id = excluded.downloader_id,
            source_site = excluded.source_site,
            downloader_name = excluded.downloader_name,
            torrent_id = excluded.torrent_id,
            title = excluded.title,
            size = excluded.size,
            release_json = excluded.release_json,
            decision_json = excluded.decision_json,
            profile_snapshot_json = excluded.profile_snapshot_json,
            status = 'queued', attempts = 0, next_attempt_at = excluded.next_attempt_at,
            lease_owner = NULL, lease_until = NULL, last_error = NULL,
            version = media_downloads.version + 1, updated_at = excluded.updated_at
         WHERE media_downloads.status IN ('failed', 'cancelled')",
        params![
            request.subscription_id,
            request.target_key,
            request.dedupe_key,
            request.site_id,
            request.downloader_id,
            request.source_site,
            request.downloader_name,
            request.torrent_id,
            request.title,
            request.size,
            request.release_json,
            request.decision_json,
            request.profile_snapshot_json,
            now,
            now,
            now,
        ],
    )
    .map_err(sql_error)?;
    load_media_download_by_dedupe(conn, &request.dedupe_key)?.ok_or_else(|| AppError::Database {
        message: "enqueued media download could not be loaded".to_string(),
    })
}

fn reset_subscription_after_failed_download(
    conn: &Connection,
    download: &MediaDownloadRecord,
    error: &str,
    now: &str,
) -> Result<(), AppError> {
    let Some(subscription_id) = download.subscription_id else {
        return Ok(());
    };
    let Some(subscription) = load_subscription(conn, subscription_id)? else {
        return Ok(());
    };
    let current_key = target_key(
        &subscription.media_type,
        subscription.tmdb_id,
        subscription.season,
        subscription.next_episode,
        subscription.absolute_episode,
    );
    if current_key != download.target_key
        || subscription.last_status.as_deref() == Some("completed")
        || !subscription_target_has_status(conn, subscription_id, &download.target_key, "queued")?
    {
        return Ok(());
    }
    let error = error.chars().take(2_000).collect::<String>();
    let subscription_changed = conn
        .execute(
            "UPDATE subscriptions
         SET next_run_at = ?, lease_owner = NULL, lease_until = NULL,
             last_status = 'error', last_error = ?, updated_at = ?,
             version = version + 1
         WHERE id = ? AND version = ?
           AND COALESCE(last_status, '') != 'completed'",
            params![now, error, now, subscription_id, subscription.version],
        )
        .map_err(sql_error)?;
    if subscription_changed == 0 {
        return Ok(());
    }
    let target_changed = conn
        .execute(
            "UPDATE subscription_targets
             SET status = 'pending', updated_at = ?
             WHERE subscription_id = ? AND target_key = ? AND status = 'queued'",
            params![now, subscription_id, download.target_key],
        )
        .map_err(sql_error)?;
    if target_changed != 1 {
        return Err(AppError::Database {
            message: format!(
                "subscription target {} changed while restoring failed download {}",
                download.target_key, download.id
            ),
        });
    }
    Ok(())
}

fn load_media_download(
    conn: &Connection,
    id: i64,
) -> Result<Option<MediaDownloadRecord>, AppError> {
    conn.query_row(
        &format!("{} WHERE id = ?", MEDIA_DOWNLOAD_SELECT),
        [id],
        map_media_download,
    )
    .optional()
    .map_err(sql_error)
}

fn load_media_download_by_dedupe(
    conn: &Connection,
    key: &str,
) -> Result<Option<MediaDownloadRecord>, AppError> {
    conn.query_row(
        &format!("{} WHERE dedupe_key = ?", MEDIA_DOWNLOAD_SELECT),
        [key],
        map_media_download,
    )
    .optional()
    .map_err(sql_error)
}

fn map_media_download(row: &Row<'_>) -> rusqlite::Result<MediaDownloadRecord> {
    Ok(MediaDownloadRecord {
        id: row.get(0)?,
        subscription_id: row.get(1)?,
        target_key: row.get(2)?,
        dedupe_key: row.get(3)?,
        site_id: row.get(4)?,
        downloader_id: row.get(5)?,
        source_site: row.get(6)?,
        downloader_name: row.get(7)?,
        torrent_id: row.get(8)?,
        title: row.get(9)?,
        size: row.get(10)?,
        release_json: row.get(11)?,
        decision_json: row.get(12)?,
        profile_snapshot_json: row.get(13)?,
        infohash: row.get(14)?,
        status: row.get(15)?,
        attempts: row.get(16)?,
        next_attempt_at: row.get(17)?,
        lease_owner: row.get(18)?,
        lease_until: row.get(19)?,
        version: row.get(20)?,
        last_error: row.get(21)?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
        submitted_at: row.get(24)?,
    })
}

fn advance_subscription_after_submit(
    conn: &Connection,
    subscription_id: i64,
    submitted_target_key: &str,
    now: &str,
) -> Result<(), AppError> {
    let subscription =
        load_subscription(conn, subscription_id)?.ok_or_else(|| AppError::Database {
            message: format!("subscription {subscription_id} not found while advancing"),
        })?;
    let current_key = target_key(
        &subscription.media_type,
        subscription.tmdb_id,
        subscription.season,
        subscription.next_episode,
        subscription.absolute_episode,
    );
    if current_key != submitted_target_key {
        return Ok(());
    }
    if subscription.media_type == "movie" {
        let changed = conn
            .execute(
                "UPDATE subscriptions
                 SET enabled = 0, next_episode = NULL, absolute_episode = NULL,
                     last_status = 'completed', last_error = NULL,
                     last_run_at = ?, updated_at = ?, version = version + 1
                 WHERE id = ? AND version = ?",
                params![now, now, subscription_id, subscription.version],
            )
            .map_err(sql_error)?;
        if changed == 0 {
            return Err(AppError::Database {
                message: format!(
                    "subscription {subscription_id} changed while advancing submitted target"
                ),
            });
        }
        return Ok(());
    }

    let submitted_target = load_subscription_target(conn, subscription_id, submitted_target_key)?
        .ok_or_else(|| AppError::Database {
        message: format!("subscription target {submitted_target_key} disappeared while advancing"),
    })?;
    let current_episode = submitted_target
        .episode
        .or(subscription.next_episode)
        .ok_or_else(|| invalid("TV subscription target has no episode"))?;
    let next_target = conn
        .query_row(
            "SELECT id, subscription_id, target_key, season, episode,
                    absolute_episode, air_date, status, created_at, updated_at
             FROM subscription_targets
             WHERE subscription_id = ? AND season IS ? AND episode > ?
               AND status IN ('pending', 'metadata_pending', 'queued')
             ORDER BY episode, id LIMIT 1",
            params![subscription_id, subscription.season, current_episode],
            map_subscription_target,
        )
        .optional()
        .map_err(sql_error)?;

    let terminal_frontier_exists = conn
        .query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM subscription_targets
                 WHERE subscription_id = ? AND season IS ? AND episode > ?
                   AND status = 'skipped'
             )",
            params![subscription_id, subscription.season, current_episode],
            |row| row.get::<_, bool>(0),
        )
        .map_err(sql_error)?;

    if next_target.is_none() && (terminal_frontier_exists || current_episode == u32::MAX) {
        let changed = conn
            .execute(
                "UPDATE subscriptions
                 SET enabled = 0, next_episode = NULL, absolute_episode = NULL,
                     next_run_at = ?, last_status = 'completed', last_error = NULL,
                     last_run_at = ?, updated_at = ?, version = version + 1
                 WHERE id = ? AND version = ?",
                params![now, now, now, subscription_id, subscription.version],
            )
            .map_err(sql_error)?;
        if changed == 0 {
            return Err(AppError::Database {
                message: format!(
                    "subscription {subscription_id} changed while completing its final target"
                ),
            });
        }
        return Ok(());
    }

    let (next_episode, next_absolute, next_run_at, next_key, create_frontier) =
        if let Some(target) = next_target {
            let next_run_at = target
                .air_date
                .as_deref()
                .and_then(air_date_eligible_at)
                .filter(|eligible_at| *eligible_at > Utc::now())
                .map(|eligible_at| eligible_at.to_rfc3339())
                .unwrap_or_else(|| now.to_string());
            (
                target.episode,
                target.absolute_episode,
                next_run_at,
                target.target_key,
                false,
            )
        } else {
            let next_episode = current_episode
                .checked_add(1)
                .ok_or_else(|| invalid("TV episode cursor overflowed"))?;
            let next_absolute = match submitted_target.absolute_episode {
                Some(value) => Some(value.checked_add(1).ok_or_else(|| {
                    invalid("absolute episode cursor overflowed; subscription was not advanced")
                })?),
                None => None,
            };
            let next_key = target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                Some(next_episode),
                next_absolute,
            );
            (
                Some(next_episode),
                next_absolute,
                now.to_string(),
                next_key,
                true,
            )
        };
    let changed = conn
        .execute(
            "UPDATE subscriptions
             SET next_episode = ?, absolute_episode = ?, next_run_at = ?,
                 last_status = 'submitted', last_error = NULL, last_run_at = ?,
                 updated_at = ?, version = version + 1
             WHERE id = ? AND version = ?",
            params![
                next_episode,
                next_absolute,
                next_run_at,
                now,
                now,
                subscription_id,
                subscription.version,
            ],
        )
        .map_err(sql_error)?;
    if changed == 0 {
        return Err(AppError::Database {
            message: format!(
                "subscription {subscription_id} changed while advancing submitted target"
            ),
        });
    }
    if create_frontier {
        conn.execute(
            "INSERT OR IGNORE INTO subscription_targets
             (subscription_id, target_key, season, episode, absolute_episode,
              status, created_at, updated_at)
             VALUES (?, ?, ?, ?, ?, 'metadata_pending', ?, ?)",
            params![
                subscription_id,
                next_key,
                subscription.season,
                next_episode,
                next_absolute,
                now,
                now,
            ],
        )
        .map_err(sql_error)?;
    }
    Ok(())
}

fn validate_quality_profile(request: &QualityProfileRequest) -> Result<(), AppError> {
    if request.name.trim().is_empty() {
        return Err(invalid("quality profile name is required"));
    }
    if !(0..=100).contains(&request.minimum_score) {
        return Err(invalid("minimum_score must be between 0 and 100"));
    }
    QualityProfile {
        id: None,
        name: request.name.trim().to_string(),
        resolution_order: request.resolution_order.clone(),
        allowed_resolutions: request.allowed_resolutions.clone(),
        blocked_resolutions: request.blocked_resolutions.clone(),
        source_order: request.source_order.clone(),
        allowed_sources: request.allowed_sources.clone(),
        codec_order: request.codec_order.clone(),
        blocked_codecs: request.blocked_codecs.clone(),
        allow_unknown_quality: request.allow_unknown_quality,
        minimum_score: request.minimum_score as u32,
        min_seeders: request.min_seeders,
    }
    .validate()
    .map_err(|error| invalid(&error.to_string()))
}

fn validate_target_seeds(
    request: &NewSubscription,
    targets: &[SubscriptionTargetSeed],
) -> Result<(), AppError> {
    validate_target_seed_values(
        &request.media_type,
        request.tmdb_id,
        request.season,
        targets,
    )
}

fn validate_target_seeds_for_subscription(
    subscription: &SubscriptionRecord,
    targets: &[SubscriptionTargetSeed],
) -> Result<(), AppError> {
    validate_target_seed_values(
        &subscription.media_type,
        subscription.tmdb_id,
        subscription.season,
        targets,
    )
}

fn validate_target_seed_values(
    media_type: &str,
    tmdb_id: i64,
    season: Option<u32>,
    targets: &[SubscriptionTargetSeed],
) -> Result<(), AppError> {
    if targets.is_empty() {
        return Ok(());
    }
    if media_type != "tv" {
        return Err(invalid("only TV subscriptions can have a TMDB target plan"));
    }
    let season = season.ok_or_else(|| invalid("TV target plan requires a season"))?;
    let mut keys = HashSet::with_capacity(targets.len());
    for target in targets {
        if target.season != season || target.episode == 0 {
            return Err(invalid(
                "TMDB target does not belong to the subscription season",
            ));
        }
        let expected_key = target_key(
            media_type,
            tmdb_id,
            Some(target.season),
            Some(target.episode),
            target.absolute_episode,
        );
        if target.target_key != expected_key || !keys.insert(target.target_key.clone()) {
            return Err(invalid(
                "TMDB target plan contains an invalid or duplicate key",
            ));
        }
    }
    Ok(())
}

fn validate_subscription(request: &NewSubscription) -> Result<(), AppError> {
    if request.tmdb_id <= 0 {
        return Err(invalid("tmdb_id must be positive"));
    }
    if !matches!(request.media_type.as_str(), "tv" | "movie") {
        return Err(invalid("media_type must be tv or movie"));
    }
    if request.title.trim().is_empty() {
        return Err(invalid("title is required"));
    }
    if request.site_ids.is_empty() {
        return Err(invalid("at least one site is required"));
    }
    if request.media_type == "tv" && request.season.is_none() {
        return Err(invalid("season is required for tv subscriptions"));
    }
    Ok(())
}

fn json_string<T: serde::Serialize>(value: &T) -> Result<String, AppError> {
    serde_json::to_string(value).map_err(|error| AppError::Database {
        message: format!("failed to serialize media data: {error}"),
    })
}

fn parse_json_column<T: serde::de::DeserializeOwned>(
    value: String,
    column: usize,
) -> rusqlite::Result<T> {
    serde_json::from_str(&value).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            column,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim().to_string();
        (!value.is_empty()).then_some(value)
    })
}

fn invalid(message: &str) -> AppError {
    AppError::InvalidConfig {
        message: message.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use tempfile::tempdir;

    use super::*;

    #[tokio::test]
    async fn resetting_quality_profiles_rebuilds_all_six_defaults() {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();

        let profiles = db.reset_quality_profiles().await.unwrap();
        let names: Vec<&str> = profiles
            .iter()
            .map(|profile| profile.name.as_str())
            .collect();

        assert_eq!(
            names,
            vec![
                "电视剧 · 日常",
                "电视剧 · 4K",
                "电影 · 收藏",
                "电影 · 均衡",
                "动漫 · 日常",
                "动漫 · 省空间",
            ]
        );
    }

    async fn database_with_media_references() -> (tempfile::TempDir, Database, i64, i64) {
        let dir = tempdir().unwrap();
        let db = Database::open(dir.path()).await.unwrap();
        let site_id = db
            .create_site(
                "media-test-site",
                "nexusphp",
                "https://tracker.example",
                r#"{"auth_type":"cookie","cookie":"test=1"}"#,
                false,
            )
            .await
            .unwrap();
        let downloader_id = db
            .create_downloader(
                "media-test-downloader",
                "qbittorrent",
                "http://127.0.0.1:8080",
                "",
                "",
            )
            .await
            .unwrap();
        (dir, db, site_id, downloader_id)
    }

    async fn create_tv_subscription(
        db: &Database,
        site_id: i64,
        downloader_id: i64,
    ) -> SubscriptionRecord {
        db.create_subscription(&NewSubscription {
            tmdb_id: 42,
            media_type: "tv".to_string(),
            tmdb_is_animation: false,
            tmdb_genres: Vec::new(),
            title: "Example Show".to_string(),
            original_title: None,
            aliases: vec!["Example".to_string()],
            year: Some(2026),
            poster_path: None,
            season: Some(1),
            start_episode: Some(3),
            absolute_episode: None,
            quality_profile_id: 1,
            downloader_id,
            site_ids: vec![site_id],
            save_path: Some(" /media/tv ".to_string()),
            enabled: true,
        })
        .await
        .unwrap()
    }

    fn test_tv_subscription_request(
        tmdb_id: i64,
        season: u32,
        start_episode: u32,
        site_id: i64,
        downloader_id: i64,
    ) -> NewSubscription {
        NewSubscription {
            tmdb_id,
            media_type: "tv".to_string(),
            tmdb_is_animation: false,
            tmdb_genres: Vec::new(),
            title: format!("Test Show {tmdb_id}"),
            original_title: None,
            aliases: Vec::new(),
            year: Some(2026),
            poster_path: None,
            season: Some(season),
            start_episode: Some(start_episode),
            absolute_episode: None,
            quality_profile_id: 1,
            downloader_id,
            site_ids: vec![site_id],
            save_path: None,
            enabled: true,
        }
    }

    fn test_target_seed(
        tmdb_id: i64,
        season: u32,
        episode: u32,
        air_date: Option<&str>,
        status: SubscriptionTargetSeedStatus,
    ) -> SubscriptionTargetSeed {
        SubscriptionTargetSeed {
            target_key: target_key("tv", tmdb_id, Some(season), Some(episode), None),
            season,
            episode,
            absolute_episode: None,
            air_date: air_date.map(str::to_string),
            status,
        }
    }

    fn new_download(
        subscription: &SubscriptionRecord,
        site_id: i64,
        downloader_id: i64,
    ) -> NewMediaDownload {
        NewMediaDownload {
            subscription_id: Some(subscription.id),
            target_key: target_key(
                &subscription.media_type,
                subscription.tmdb_id,
                subscription.season,
                subscription.next_episode,
                subscription.absolute_episode,
            ),
            dedupe_key: format!("subscription:{}:episode-3", subscription.id),
            site_id: Some(site_id),
            downloader_id: Some(downloader_id),
            source_site: "media-test-site".to_string(),
            downloader_name: "media-test-downloader".to_string(),
            torrent_id: "torrent-3".to_string(),
            title: "Example.Show.S01E03.1080p.WEB-DL".to_string(),
            size: 1_024,
            release_json: "{}".to_string(),
            decision_json: "{}".to_string(),
            profile_snapshot_json: "{}".to_string(),
        }
    }

    async fn enqueue_linked_download(
        db: &Database,
        request: &NewMediaDownload,
    ) -> MediaDownloadRecord {
        let queued = db.enqueue_media_download(request).await.unwrap();
        set_test_target_status(
            db,
            request.subscription_id.expect("linked test download"),
            &request.target_key,
            "queued",
        );
        queued
    }

    fn set_test_target_status(db: &Database, subscription_id: i64, key: &str, status: &str) {
        let conn = open_connection(&db.path).unwrap();
        assert_eq!(
            conn.execute(
                "UPDATE subscription_targets SET status = ?, updated_at = ?
                 WHERE subscription_id = ? AND target_key = ?",
                params![status, Utc::now().to_rfc3339(), subscription_id, key],
            )
            .unwrap(),
            1
        );
    }

    fn make_download_due(db: &Database, id: i64) {
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_downloads
             SET next_attempt_at = '1970-01-01T00:00:00Z'
             WHERE id = ?",
            [id],
        )
        .unwrap();
    }

    #[tokio::test]
    async fn subscription_claim_and_finish_use_owner_and_version_cas() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;

        let claimed = db
            .claim_subscription(created.id, "worker-a", 60)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(claimed.version, created.version + 1);
        assert_eq!(claimed.lease_owner.as_deref(), Some("worker-a"));
        assert!(
            db.claim_subscription(created.id, "worker-b", 60)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !db.finish_subscription_scan(
                created.id,
                claimed.version,
                "worker-b",
                &Utc::now().to_rfc3339(),
                "waiting",
                None,
            )
            .await
            .unwrap()
        );
        assert!(
            db.finish_subscription_scan(
                created.id,
                claimed.version,
                "worker-a",
                &Utc::now().to_rfc3339(),
                "waiting",
                None,
            )
            .await
            .unwrap()
        );

        let finished = db.get_subscription(created.id).await.unwrap().unwrap();
        assert!(finished.lease_owner.is_none());
        assert!(finished.lease_until.is_none());
        assert_eq!(finished.last_status.as_deref(), Some("waiting"));
    }

    #[tokio::test]
    async fn subscription_run_info_is_saved_only_by_the_active_lease_owner() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        db.claim_subscription(created.id, "snapshot-owner", 60)
            .await
            .unwrap()
            .unwrap();

        assert!(
            !db.save_claimed_subscription_last_run_info(
                created.id,
                "other-owner",
                r#"{"queries":["wrong"]}"#,
            )
            .await
            .unwrap()
        );
        let expected = r#"{"queries":["Example Show S01E03"]}"#;
        assert!(
            db.save_claimed_subscription_last_run_info(created.id, "snapshot-owner", expected,)
                .await
                .unwrap()
        );
        assert_eq!(
            db.get_subscription_last_run_info(created.id)
                .await
                .unwrap()
                .as_deref(),
            Some(expected)
        );
    }

    #[tokio::test]
    async fn subscription_alias_refresh_uses_active_owner_and_version_cas() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let claimed = db
            .claim_subscription(created.id, "alias-owner", 60)
            .await
            .unwrap()
            .unwrap();
        let aliases = vec![
            "Example".to_string(),
            "Crowned in a Hundred Days".to_string(),
        ];

        assert!(
            db.refresh_claimed_subscription_aliases(
                created.id,
                claimed.version,
                "other-owner",
                &aliases,
            )
            .await
            .unwrap()
            .is_none()
        );
        let refreshed_version = db
            .refresh_claimed_subscription_aliases(
                created.id,
                claimed.version,
                "alias-owner",
                &aliases,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(refreshed_version, claimed.version + 1);
        assert_eq!(
            db.get_subscription(created.id)
                .await
                .unwrap()
                .unwrap()
                .aliases,
            aliases
        );
    }

    #[tokio::test]
    async fn abandoned_owner_recovery_releases_future_lease_and_makes_scan_due() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let interrupted = create_tv_subscription(&db, site_id, downloader_id).await;
        let foreign = db
            .create_subscription(&test_tv_subscription_request(
                43,
                1,
                1,
                site_id,
                downloader_id,
            ))
            .await
            .unwrap();
        let interrupted_owner = "manual-subscription-999999-1000";
        let foreign_owner = "media-subscriptions-42-deadbeef-1";
        let interrupted_claim = db
            .claim_subscription(interrupted.id, interrupted_owner, 600)
            .await
            .unwrap()
            .unwrap();
        let foreign_claim = db
            .claim_subscription(foreign.id, foreign_owner, 600)
            .await
            .unwrap()
            .unwrap();
        assert!(
            chrono::DateTime::parse_from_rfc3339(&interrupted_claim.next_run_at).unwrap()
                <= Utc::now()
        );
        assert_eq!(
            db.list_active_subscription_lease_owners().await.unwrap(),
            vec![interrupted_owner.to_string(), foreign_owner.to_string()]
        );

        assert_eq!(
            db.recover_subscription_leases_for_owners(&[interrupted_owner.to_string()])
                .await
                .unwrap(),
            1
        );
        let recovered = db.get_subscription(interrupted.id).await.unwrap().unwrap();
        assert_eq!(recovered.lease_owner, None);
        assert_eq!(recovered.lease_until, None);
        assert_eq!(recovered.version, interrupted_claim.version + 1);
        assert_eq!(recovered.last_status.as_deref(), Some("interrupted"));
        assert!(
            recovered
                .last_error
                .as_deref()
                .is_some_and(|error| { error.contains("scheduled to retry") })
        );
        assert!(
            chrono::DateTime::parse_from_rfc3339(&recovered.next_run_at).unwrap() <= Utc::now()
        );
        assert!(
            !db.finish_subscription_scan(
                interrupted.id,
                interrupted_claim.version,
                interrupted_owner,
                &Utc::now().to_rfc3339(),
                "waiting",
                None,
            )
            .await
            .unwrap()
        );
        assert_eq!(
            db.renew_subscription_lease(
                interrupted.id,
                interrupted_claim.version,
                interrupted_owner,
                60,
            )
            .await
            .unwrap(),
            None
        );
        let due = db
            .claim_due_subscriptions("replacement-owner", 60, 10)
            .await
            .unwrap();
        let replacement = due
            .iter()
            .find(|record| record.id == interrupted.id)
            .expect("the interrupted subscription should be immediately claimable");
        assert_eq!(
            replacement.lease_owner.as_deref(),
            Some("replacement-owner")
        );

        let unchanged_foreign = db.get_subscription(foreign.id).await.unwrap().unwrap();
        assert_eq!(unchanged_foreign.version, foreign_claim.version);
        assert_eq!(
            unchanged_foreign.lease_owner.as_deref(),
            Some(foreign_owner)
        );
    }

    #[tokio::test]
    async fn abandoned_scan_recovery_preserves_an_already_queued_download() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(44, 1, 3, site_id, downloader_id),
                &[
                    test_target_seed(
                        44,
                        1,
                        3,
                        Some("2020-01-01"),
                        SubscriptionTargetSeedStatus::Pending,
                    ),
                    test_target_seed(
                        44,
                        1,
                        4,
                        None,
                        SubscriptionTargetSeedStatus::MetadataPending,
                    ),
                ],
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let owner = "manual-subscription-999998-1000";
        let claimed = db
            .claim_subscription(subscription.id, owner, 600)
            .await
            .unwrap()
            .unwrap();
        let request = new_download(&claimed, site_id, downloader_id);
        let queued = db
            .enqueue_claimed_subscription_media_download(claimed.version, owner, &request)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            db.recover_subscription_leases_for_owners(&[owner.to_string()])
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        assert_eq!(
            load_media_download_by_dedupe(
                &open_connection(&db.path).unwrap(),
                &request.dedupe_key,
            )
            .unwrap()
            .unwrap()
            .id,
            queued.id
        );

        let downloads = db
            .claim_due_media_downloads("replacement-download-worker", 60, 10)
            .await
            .unwrap();
        assert_eq!(downloads.len(), 1);
        assert_eq!(downloads[0].id, queued.id);
        assert!(
            db.claim_subscription(subscription.id, "replacement-scan-worker", 60)
                .await
                .unwrap()
                .is_none(),
            "an active queued download must fence a replacement scan"
        );
    }

    #[tokio::test]
    async fn owner_scoped_download_recovery_retries_without_waiting_for_lease_expiry() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let queued =
            enqueue_linked_download(&db, &new_download(&subscription, site_id, downloader_id))
                .await;
        let owner = "media-downloads-v2-999997-1000-1";
        let claimed = db
            .claim_due_media_downloads(owner, 600, 10)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claimed.id, queued.id);
        assert_eq!(
            db.list_active_media_download_lease_owners().await.unwrap(),
            vec![owner.to_string()]
        );
        assert_eq!(
            db.recover_media_download_leases_for_owners(&["another-owner".to_string()])
                .await
                .unwrap(),
            0
        );

        assert_eq!(
            db.recover_media_download_leases_for_owners(&[owner.to_string()])
                .await
                .unwrap(),
            1
        );
        let recovered = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, "retry_wait");
        assert_eq!(recovered.lease_owner, None);
        assert_eq!(recovered.lease_until, None);
        assert_eq!(recovered.version, claimed.version + 1);
        assert!(
            !db.transition_media_download(
                claimed.id,
                claimed.version,
                owner,
                "fetching",
                "retry_wait",
                None,
                Some("stale worker"),
                None,
            )
            .await
            .unwrap()
        );

        let replacement = db
            .claim_due_media_downloads("replacement-download-worker", 60, 10)
            .await
            .unwrap();
        assert_eq!(replacement.len(), 1);
        assert_eq!(replacement[0].id, queued.id);
    }

    #[tokio::test]
    async fn owner_scoped_submitting_recovery_waits_before_final_reconciliation() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&subscription, site_id, downloader_id);
        request.subscription_id = None;
        request.dedupe_key = "owner-recovery-final-reconciliation".to_string();
        let queued = db.enqueue_media_download(&request).await.unwrap();

        for expected_attempt in 1..MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            let owner = format!("pre-recovery-worker-{expected_attempt}");
            let claimed = db
                .claim_due_media_downloads(&owner, 60, 1)
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(claimed.attempts, expected_attempt);
            assert!(
                db.release_media_download_after_error(claimed.id, &owner, "fetch failed", false)
                    .await
                    .unwrap()
            );
            make_download_due(&db, queued.id);
        }

        let owner = "media-downloads-v2-999996-1000-1";
        let claimed = db
            .claim_due_media_downloads(owner, 600, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claimed.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        assert!(
            db.mark_media_download_submitting(
                claimed.id,
                claimed.version,
                owner,
                "2222222222222222222222222222222222222222",
                600,
            )
            .await
            .unwrap()
        );

        let recovered_at = Utc::now();
        assert_eq!(
            db.recover_media_download_leases_for_owners(&[owner.to_string()])
                .await
                .unwrap(),
            1
        );
        let recovered = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(recovered.status, "reconciling");
        assert_eq!(recovered.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        let next_attempt_at =
            chrono::DateTime::parse_from_rfc3339(recovered.next_attempt_at.as_deref().unwrap())
                .unwrap();
        assert!(
            next_attempt_at
                >= recovered_at + Duration::seconds(MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS)
        );
        assert!(
            db.claim_due_media_downloads("early-reconciler", 60, 1)
                .await
                .unwrap()
                .is_empty()
        );

        make_download_due(&db, queued.id);
        let reconciliation = db
            .claim_due_media_downloads("ready-reconciler", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(reconciliation.id, queued.id);
        assert_eq!(reconciliation.status, "reconciling");
        assert_eq!(reconciliation.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn media_download_deduplicates_and_retries_with_a_new_lease() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let request = new_download(&subscription, site_id, downloader_id);

        let first = enqueue_linked_download(&db, &request).await;
        let duplicate = db.enqueue_media_download(&request).await.unwrap();
        assert_eq!(duplicate.id, first.id);

        let claimed = db
            .claim_due_media_downloads("download-worker-a", 60, 10)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].status, "fetching");
        assert_eq!(claimed[0].attempts, 1);
        assert!(
            db.claim_due_media_downloads("download-worker-b", 60, 10)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            !db.transition_media_download(
                first.id,
                claimed[0].version - 1,
                "download-worker-a",
                "fetching",
                "retry_wait",
                None,
                Some("stale update"),
                None,
            )
            .await
            .unwrap()
        );
        assert!(
            db.transition_media_download(
                first.id,
                claimed[0].version,
                "download-worker-a",
                "fetching",
                "retry_wait",
                None,
                Some("temporary failure"),
                None,
            )
            .await
            .unwrap()
        );

        let reclaimed = db
            .claim_due_media_downloads("download-worker-b", 60, 10)
            .await
            .unwrap();
        assert_eq!(reclaimed.len(), 1);
        assert_eq!(reclaimed[0].id, first.id);
        assert_eq!(reclaimed[0].attempts, 2);
        assert_eq!(
            reclaimed[0].lease_owner.as_deref(),
            Some("download-worker-b")
        );
    }

    #[tokio::test]
    async fn submitted_download_advances_subscription_and_target_atomically() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let request = new_download(&subscription, site_id, downloader_id);
        let queued = enqueue_linked_download(&db, &request).await;
        let fetching = db
            .claim_due_media_downloads("download-worker", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let infohash = "0123456789abcdef0123456789abcdef01234567";
        assert!(
            db.mark_media_download_submitting(
                queued.id,
                fetching.version,
                "download-worker",
                infohash,
                60,
            )
            .await
            .unwrap()
        );
        let submitting = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert!(
            db.mark_media_download_submitted(
                submitting.id,
                submitting.version,
                "download-worker",
                infohash,
            )
            .await
            .unwrap()
        );

        let completed_target = db
            .get_subscription_target(subscription.id, &request.target_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(completed_target.status, "submitted");
        let advanced = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert_eq!(advanced.next_episode, Some(4));
        assert_eq!(advanced.last_status.as_deref(), Some("submitted"));
        let next_key = target_key("tv", advanced.tmdb_id, Some(1), Some(4), None);
        let next_target = db
            .get_subscription_target(advanced.id, &next_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(next_target.status, "metadata_pending");
    }

    #[tokio::test]
    async fn manual_linked_enqueue_checks_version_lease_and_materialized_readiness_atomically() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let future_targets = vec![
            test_target_seed(
                101,
                1,
                3,
                Some("2099-07-01"),
                SubscriptionTargetSeedStatus::Pending,
            ),
            test_target_seed(
                101,
                1,
                4,
                None,
                SubscriptionTargetSeedStatus::MetadataPending,
            ),
        ];
        let future = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(101, 1, 3, site_id, downloader_id),
                &future_targets,
                None,
                Some("waiting_air_date"),
            )
            .await
            .unwrap();
        let future_download = new_download(&future, site_id, downloader_id);
        assert!(
            db.enqueue_subscription_media_download(future.version, &future_download)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            load_media_download_by_dedupe(
                &open_connection(&db.path).unwrap(),
                &future_download.dedupe_key,
            )
            .unwrap()
            .is_none()
        );

        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscription_targets SET air_date = '2020-01-01'
             WHERE subscription_id = ? AND target_key = ?",
            params![future.id, future_download.target_key],
        )
        .unwrap();
        drop(conn);
        let claimed = db
            .claim_subscription(future.id, "manual-readiness-scan", 60)
            .await
            .unwrap()
            .unwrap();
        assert!(
            db.enqueue_subscription_media_download(claimed.version, &future_download)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.finish_subscription_scan(
                future.id,
                claimed.version,
                "manual-readiness-scan",
                &Utc::now().to_rfc3339(),
                "waiting",
                None,
            )
            .await
            .unwrap()
        );
        let ready = db.get_subscription(future.id).await.unwrap().unwrap();
        assert!(
            db.enqueue_subscription_media_download(ready.version - 1, &future_download)
                .await
                .unwrap()
                .is_none()
        );
        let queued = db
            .enqueue_subscription_media_download(ready.version, &future_download)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(queued.status, "queued");
        assert_eq!(
            db.get_subscription_target(future.id, &future_download.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        let queued_subscription = db.get_subscription(future.id).await.unwrap().unwrap();
        assert_eq!(queued_subscription.version, ready.version + 1);
        assert_eq!(queued_subscription.last_status.as_deref(), Some("queued"));

        let non_terminal_targets = vec![
            test_target_seed(102, 1, 3, None, SubscriptionTargetSeedStatus::Pending),
            test_target_seed(
                102,
                1,
                4,
                None,
                SubscriptionTargetSeedStatus::MetadataPending,
            ),
        ];
        let non_terminal = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(102, 1, 3, site_id, downloader_id),
                &non_terminal_targets,
                None,
                Some("awaiting_metadata"),
            )
            .await
            .unwrap();
        assert!(
            db.enqueue_subscription_media_download(
                non_terminal.version,
                &new_download(&non_terminal, site_id, downloader_id),
            )
            .await
            .unwrap()
            .is_none()
        );

        let terminal_targets = vec![
            test_target_seed(103, 1, 3, None, SubscriptionTargetSeedStatus::Pending),
            test_target_seed(103, 1, 4, None, SubscriptionTargetSeedStatus::Skipped),
        ];
        let terminal = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(103, 1, 3, site_id, downloader_id),
                &terminal_targets,
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        assert!(
            db.enqueue_subscription_media_download(
                terminal.version,
                &new_download(&terminal, site_id, downloader_id),
            )
            .await
            .unwrap()
            .is_some()
        );
    }

    #[tokio::test]
    async fn claimed_enqueue_is_atomic_against_cursor_edits() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let targets = |tmdb_id| {
            vec![
                test_target_seed(
                    tmdb_id,
                    1,
                    3,
                    Some("2020-01-01"),
                    SubscriptionTargetSeedStatus::Pending,
                ),
                test_target_seed(
                    tmdb_id,
                    1,
                    4,
                    Some("2020-01-08"),
                    SubscriptionTargetSeedStatus::Pending,
                ),
                test_target_seed(
                    tmdb_id,
                    1,
                    5,
                    None,
                    SubscriptionTargetSeedStatus::MetadataPending,
                ),
            ]
        };

        let edit_wins = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(108, 1, 3, site_id, downloader_id),
                &targets(108),
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let stale_claim = db
            .claim_subscription(edit_wins.id, "stale-auto-scan", 60)
            .await
            .unwrap()
            .unwrap();
        let stale_download = new_download(&stale_claim, site_id, downloader_id);
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscriptions SET lease_until = '1970-01-01T00:00:00Z' WHERE id = ?",
            [edit_wins.id],
        )
        .unwrap();
        drop(conn);
        let edited = db
            .update_subscription(
                edit_wins.id,
                stale_claim.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(4),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(edited.next_episode, Some(4));
        assert!(
            db.enqueue_claimed_subscription_media_download(
                stale_claim.version,
                "stale-auto-scan",
                &stale_download,
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(
            load_media_download_by_dedupe(
                &open_connection(&db.path).unwrap(),
                &stale_download.dedupe_key,
            )
            .unwrap()
            .is_none()
        );

        let enqueue_wins = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(109, 1, 3, site_id, downloader_id),
                &targets(109),
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let live_claim = db
            .claim_subscription(enqueue_wins.id, "live-auto-scan", 60)
            .await
            .unwrap()
            .unwrap();
        let live_download = new_download(&live_claim, site_id, downloader_id);
        let queued = db
            .enqueue_claimed_subscription_media_download(
                live_claim.version,
                "live-auto-scan",
                &live_download,
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(queued.status, "queued");
        let after_enqueue = db.get_subscription(enqueue_wins.id).await.unwrap().unwrap();
        assert_eq!(after_enqueue.version, live_claim.version);
        assert_eq!(after_enqueue.lease_owner.as_deref(), Some("live-auto-scan"));
        assert_eq!(
            db.get_subscription_target(enqueue_wins.id, &live_download.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscriptions SET lease_until = '1970-01-01T00:00:00Z' WHERE id = ?",
            [enqueue_wins.id],
        )
        .unwrap();
        drop(conn);
        assert!(
            db.update_subscription(
                enqueue_wins.id,
                live_claim.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(4),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .is_err()
        );
        assert_eq!(
            load_media_download_by_dedupe(
                &open_connection(&db.path).unwrap(),
                &live_download.dedupe_key,
            )
            .unwrap()
            .unwrap()
            .id,
            queued.id
        );
    }

    #[tokio::test]
    async fn linked_downloads_require_a_queued_target_to_claim_or_submit() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let request = new_download(&subscription, site_id, downloader_id);
        let queued = db.enqueue_media_download(&request).await.unwrap();
        assert!(
            db.claim_due_media_downloads("blocked-download", 60, 1)
                .await
                .unwrap()
                .is_empty()
        );

        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_downloads
             SET status = 'submitting', lease_owner = 'forced-submitter',
                 lease_until = ?, version = version + 1
             WHERE id = ?",
            params![(Utc::now() + Duration::minutes(1)).to_rfc3339(), queued.id],
        )
        .unwrap();
        drop(conn);
        let forced = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert!(
            !db.mark_media_download_submitted(
                forced.id,
                forced.version,
                "forced-submitter",
                "abababababababababababababababababababab",
            )
            .await
            .unwrap()
        );
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
    }

    #[tokio::test]
    async fn tmdb_target_sync_is_cas_guarded_and_preserves_monotonic_statuses() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let current_key = target_key("tv", created.tmdb_id, Some(1), Some(3), None);
        set_test_target_status(&db, created.id, &current_key, "queued");
        let claimed = db
            .claim_subscription(created.id, "tmdb-worker", 60)
            .await
            .unwrap()
            .unwrap();
        let targets = vec![
            SubscriptionTargetSeed {
                target_key: current_key.clone(),
                season: 1,
                episode: 3,
                absolute_episode: None,
                air_date: Some("2026-07-01".to_string()),
                status: SubscriptionTargetSeedStatus::Pending,
            },
            SubscriptionTargetSeed {
                target_key: target_key("tv", created.tmdb_id, Some(1), Some(4), None),
                season: 1,
                episode: 4,
                absolute_episode: None,
                air_date: Some("2026-07-08".to_string()),
                status: SubscriptionTargetSeedStatus::Pending,
            },
            SubscriptionTargetSeed {
                target_key: target_key("tv", created.tmdb_id, Some(1), Some(5), None),
                season: 1,
                episode: 5,
                absolute_episode: None,
                air_date: None,
                status: SubscriptionTargetSeedStatus::MetadataPending,
            },
        ];

        assert!(
            db.sync_claimed_subscription_targets(
                created.id,
                claimed.version - 1,
                "tmdb-worker",
                &targets,
                false,
                &[],
            )
            .await
            .unwrap()
            .is_none()
        );
        assert!(
            db.get_subscription_target(created.id, &targets[1].target_key)
                .await
                .unwrap()
                .is_none()
        );

        let synced = db
            .sync_claimed_subscription_targets(
                created.id,
                claimed.version,
                "tmdb-worker",
                &targets,
                false,
                &[],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(synced.version, claimed.version + 1);
        assert_eq!(synced.current.unwrap().status, "queued");
        assert_eq!(
            db.get_subscription_target(created.id, &targets[1].target_key)
                .await
                .unwrap()
                .unwrap()
                .air_date
                .as_deref(),
            Some("2026-07-08")
        );
    }

    #[tokio::test]
    async fn tmdb_retraction_skips_removed_targets_and_completes_at_new_terminal_frontier() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let original_targets = vec![
            test_target_seed(
                104,
                1,
                3,
                Some("2020-01-01"),
                SubscriptionTargetSeedStatus::Pending,
            ),
            test_target_seed(
                104,
                1,
                4,
                Some("2020-01-08"),
                SubscriptionTargetSeedStatus::Pending,
            ),
            test_target_seed(
                104,
                1,
                5,
                None,
                SubscriptionTargetSeedStatus::MetadataPending,
            ),
        ];
        let subscription = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(104, 1, 3, site_id, downloader_id),
                &original_targets,
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let claimed = db
            .claim_subscription(subscription.id, "retraction-worker", 60)
            .await
            .unwrap()
            .unwrap();
        let terminal_targets = vec![
            test_target_seed(
                104,
                1,
                3,
                Some("2020-01-01"),
                SubscriptionTargetSeedStatus::Pending,
            ),
            test_target_seed(104, 1, 4, None, SubscriptionTargetSeedStatus::Skipped),
        ];
        let synced = db
            .sync_claimed_subscription_targets(
                subscription.id,
                claimed.version,
                "retraction-worker",
                &terminal_targets,
                false,
                &[],
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            db.get_subscription_target(subscription.id, &original_targets[1].target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "skipped"
        );
        assert_eq!(
            db.get_subscription_target(subscription.id, &original_targets[2].target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "skipped"
        );
        assert!(
            db.finish_subscription_scan(
                subscription.id,
                synced.version,
                "retraction-worker",
                &Utc::now().to_rfc3339(),
                "waiting",
                None,
            )
            .await
            .unwrap()
        );
        let ready = db.get_subscription(subscription.id).await.unwrap().unwrap();
        let mut download = new_download(&ready, site_id, downloader_id);
        download.dedupe_key = "retracted-terminal-episode-3".to_string();
        submit_test_download(
            &db,
            &download,
            "retracted-terminal-submitter",
            "3333333333333333333333333333333333333333",
        )
        .await;
        let completed = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert!(!completed.enabled);
        assert_eq!(completed.next_episode, None);
        assert_eq!(completed.last_status.as_deref(), Some("completed"));
        assert!(
            !db.set_subscription_enabled(subscription.id, completed.version, true)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn materialized_terminal_season_advances_by_air_date_then_completes() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let request = NewSubscription {
            tmdb_id: 77,
            media_type: "tv".to_string(),
            tmdb_is_animation: false,
            tmdb_genres: Vec::new(),
            title: "Finite Show".to_string(),
            original_title: None,
            aliases: Vec::new(),
            year: Some(2026),
            poster_path: None,
            season: Some(1),
            start_episode: Some(3),
            absolute_episode: None,
            quality_profile_id: 1,
            downloader_id,
            site_ids: vec![site_id],
            save_path: None,
            enabled: true,
        };
        let episode_3_key = target_key("tv", 77, Some(1), Some(3), None);
        let episode_4_key = target_key("tv", 77, Some(1), Some(4), None);
        let targets = vec![
            SubscriptionTargetSeed {
                target_key: episode_3_key.clone(),
                season: 1,
                episode: 3,
                absolute_episode: None,
                air_date: Some("2026-07-01".to_string()),
                status: SubscriptionTargetSeedStatus::Pending,
            },
            SubscriptionTargetSeed {
                target_key: episode_4_key.clone(),
                season: 1,
                episode: 4,
                absolute_episode: None,
                air_date: Some("2099-08-20".to_string()),
                status: SubscriptionTargetSeedStatus::Pending,
            },
            SubscriptionTargetSeed {
                target_key: target_key("tv", 77, Some(1), Some(5), None),
                season: 1,
                episode: 5,
                absolute_episode: None,
                air_date: None,
                status: SubscriptionTargetSeedStatus::Skipped,
            },
        ];
        let subscription = db
            .create_subscription_with_targets(
                &request,
                &targets,
                Some("2026-07-01T00:00:00Z"),
                Some("waiting"),
            )
            .await
            .unwrap();

        let mut first = new_download(&subscription, site_id, downloader_id);
        first.dedupe_key = "terminal-season-episode-3".to_string();
        first.target_key = episode_3_key;
        submit_test_download(
            &db,
            &first,
            "terminal-worker-3",
            "1111111111111111111111111111111111111111",
        )
        .await;
        let advanced = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert!(advanced.enabled);
        assert_eq!(advanced.next_episode, Some(4));
        assert!(advanced.next_run_at.starts_with("2099-08-20T00:00:00"));

        let mut second = new_download(&advanced, site_id, downloader_id);
        second.dedupe_key = "terminal-season-episode-4".to_string();
        second.target_key = episode_4_key;
        submit_test_download(
            &db,
            &second,
            "terminal-worker-4",
            "2222222222222222222222222222222222222222",
        )
        .await;
        let completed = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert!(!completed.enabled);
        assert_eq!(completed.next_episode, None);
        assert_eq!(completed.last_status.as_deref(), Some("completed"));
    }

    #[tokio::test]
    async fn completed_movie_cannot_be_reopened_by_rule_edit_or_resume() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let movie = db
            .create_subscription(&NewSubscription {
                tmdb_id: 107,
                media_type: "movie".to_string(),
                tmdb_is_animation: false,
                tmdb_genres: Vec::new(),
                title: "Finished Movie".to_string(),
                original_title: None,
                aliases: Vec::new(),
                year: Some(2026),
                poster_path: None,
                season: None,
                start_episode: None,
                absolute_episode: None,
                quality_profile_id: 1,
                downloader_id,
                site_ids: vec![site_id],
                save_path: None,
                enabled: true,
            })
            .await
            .unwrap();
        let mut download = new_download(&movie, site_id, downloader_id);
        download.dedupe_key = "completed-movie".to_string();
        submit_test_download(
            &db,
            &download,
            "movie-submitter",
            "4444444444444444444444444444444444444444",
        )
        .await;
        let completed = db.get_subscription(movie.id).await.unwrap().unwrap();
        assert_eq!(completed.last_status.as_deref(), Some("completed"));
        assert!(!completed.enabled);
        assert!(
            db.update_subscription(
                movie.id,
                completed.version,
                &UpdateSubscription {
                    season: None,
                    next_episode: None,
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: Some("/new/movie/path".to_string()),
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .is_err()
        );
        assert!(
            !db.set_subscription_enabled(movie.id, completed.version, true)
                .await
                .unwrap()
        );
        let unchanged = db.get_subscription(movie.id).await.unwrap().unwrap();
        assert_eq!(unchanged.last_status.as_deref(), Some("completed"));
        assert!(!unchanged.enabled);
        assert_eq!(
            db.get_subscription_target(movie.id, &download.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "submitted"
        );
    }

    async fn submit_test_download(
        db: &Database,
        request: &NewMediaDownload,
        owner: &str,
        infohash: &str,
    ) {
        let queued = enqueue_linked_download(db, request).await;
        let fetching = db
            .claim_due_media_downloads(owner, 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(fetching.id, queued.id);
        assert!(
            db.mark_media_download_submitting(queued.id, fetching.version, owner, infohash, 60)
                .await
                .unwrap()
        );
        let submitting = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert!(
            db.mark_media_download_submitted(queued.id, submitting.version, owner, infohash)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn subscription_renewal_and_management_use_owner_version_and_lease_cas() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let claimed = db
            .claim_subscription(created.id, "worker-a", 60)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(
            db.renew_subscription_lease(created.id, claimed.version, "worker-b", 60)
                .await
                .unwrap(),
            None
        );
        assert_eq!(
            db.renew_subscription_lease(created.id, claimed.version - 1, "worker-a", 60)
                .await
                .unwrap(),
            None
        );
        let renewed_version = db
            .renew_subscription_lease(created.id, claimed.version, "worker-a", 60)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(renewed_version, claimed.version + 1);

        let update = UpdateSubscription {
            season: Some(1),
            next_episode: Some(9),
            absolute_episode: None,
            quality_profile_id: 1,
            downloader_id,
            site_ids: vec![site_id],
            save_path: None,
            enabled: true,
            reset_download_history: false,
        };
        assert!(
            db.update_subscription(created.id, renewed_version, &update)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            !db.set_subscription_enabled(created.id, renewed_version, false)
                .await
                .unwrap()
        );
        assert!(
            !db.delete_subscription(created.id, renewed_version)
                .await
                .unwrap()
        );
        assert!(
            db.finish_subscription_scan(
                created.id,
                renewed_version,
                "worker-a",
                &Utc::now().to_rfc3339(),
                "waiting",
                None,
            )
            .await
            .unwrap()
        );

        let finished = db.get_subscription(created.id).await.unwrap().unwrap();
        assert!(
            !db.set_subscription_enabled(created.id, renewed_version, false)
                .await
                .unwrap()
        );
        assert!(
            db.set_subscription_enabled(created.id, finished.version, false)
                .await
                .unwrap()
        );
    }

    #[tokio::test]
    async fn submitted_current_target_does_not_block_moving_to_another_episode() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = db
            .create_subscription(&test_tv_subscription_request(
                205,
                1,
                10,
                site_id,
                downloader_id,
            ))
            .await
            .unwrap();
        let current_key = target_key("tv", 205, Some(1), Some(10), None);
        set_test_target_status(&db, created.id, &current_key, "submitted");

        let moved = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(9),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert_eq!(moved.next_episode, Some(9));
        assert_eq!(moved.start_episode, Some(9));
    }

    #[tokio::test]
    async fn submitted_destination_requires_an_explicit_history_reset() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = db
            .create_subscription(&test_tv_subscription_request(
                206,
                1,
                10,
                site_id,
                downloader_id,
            ))
            .await
            .unwrap();
        let destination_key = target_key("tv", 206, Some(1), Some(9), None);
        let now = Utc::now().to_rfc3339();
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "INSERT INTO subscription_targets
             (subscription_id, target_key, season, episode, status, created_at, updated_at)
             VALUES (?, ?, 1, 9, 'submitted', ?, ?)",
            params![created.id, destination_key, now, now],
        )
        .unwrap();
        let dedupe_key = format!("subscription:{}:{destination_key}", created.id);
        let infohash = "1234567890abcdef1234567890abcdef12345678";
        conn.execute(
            "INSERT INTO media_downloads
             (subscription_id, target_key, dedupe_key, site_id, downloader_id,
              source_site, downloader_name, torrent_id, title, size, release_json,
              decision_json, profile_snapshot_json, infohash, status,
              created_at, updated_at, submitted_at)
             VALUES (?, ?, ?, ?, ?, 'site', 'qB', 'torrent-9', 'Example Show E09', 1024,
                     '{}', '{}', '{}', ?, 'submitted', ?, ?, ?)",
            params![
                created.id,
                destination_key,
                dedupe_key,
                site_id,
                downloader_id,
                infohash,
                now,
                now,
                now,
            ],
        )
        .unwrap();
        let download_id = conn.last_insert_rowid();
        drop(conn);

        let error = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(9),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("reset_download_history"));
        let unchanged = db.get_subscription(created.id).await.unwrap().unwrap();
        assert_eq!(unchanged.next_episode, Some(10));
        assert_eq!(unchanged.version, created.version);
        assert_eq!(
            db.get_subscription_target(created.id, &destination_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "submitted"
        );
        let unchanged_download = db.get_media_download(download_id).await.unwrap().unwrap();
        assert_eq!(unchanged_download.dedupe_key, dedupe_key);
        assert_eq!(unchanged_download.infohash.as_deref(), Some(infohash));

        let moved = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(9),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: true,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved.next_episode, Some(9));
        assert!(db.get_media_download(download_id).await.unwrap().is_none());
        assert_eq!(
            db.get_subscription_target(created.id, &destination_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "metadata_pending"
        );
    }

    #[tokio::test]
    async fn resetting_download_history_releases_keys_and_restores_episode_progression() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let targets = (1..=5)
            .map(|episode| {
                test_target_seed(
                    207,
                    1,
                    episode,
                    Some("2020-01-01"),
                    SubscriptionTargetSeedStatus::Pending,
                )
            })
            .collect::<Vec<_>>();
        let created = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(207, 1, 1, site_id, downloader_id),
                &targets,
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let now = Utc::now().to_rfc3339();
        let first_hash = format!("{:040x}", 1);
        let mut conn = open_connection(&db.path).unwrap();
        let tx = conn.transaction().unwrap();
        tx.execute(
            "UPDATE subscriptions
             SET next_episode = 5, start_episode = 1, last_status = 'submitted'
             WHERE id = ?",
            [created.id],
        )
        .unwrap();
        let mut first_download_id = None;
        for episode in 1..=4 {
            let key = target_key("tv", 207, Some(1), Some(episode), None);
            tx.execute(
                "UPDATE subscription_targets SET status = 'submitted'
                 WHERE subscription_id = ? AND target_key = ?",
                params![created.id, key],
            )
            .unwrap();
            tx.execute(
                "INSERT INTO media_downloads
                 (subscription_id, target_key, dedupe_key, site_id, downloader_id,
                  source_site, downloader_name, torrent_id, title, size, release_json,
                  decision_json, profile_snapshot_json, infohash, status,
                  created_at, updated_at, submitted_at)
                 VALUES (?, ?, ?, ?, ?, 'site', 'qB', ?, ?, 1024, '{}', '{}', '{}', ?,
                         'submitted', ?, ?, ?)",
                params![
                    created.id,
                    key,
                    format!("subscription:{}:{key}", created.id),
                    site_id,
                    downloader_id,
                    format!("torrent-{episode}"),
                    format!("Example.Show.S01E{episode:02}.1080p.WEB-DL"),
                    format!("{episode:040x}"),
                    now,
                    now,
                    now,
                ],
            )
            .unwrap();
            if episode == 1 {
                first_download_id = Some(tx.last_insert_rowid());
            }
        }
        tx.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              torrent_name, stage, created_at, updated_at)
             VALUES (?, ?, ?, '/downloads', '/downloads', '/media', '/media',
                     'Example Show', 'completed', ?, ?)",
            params![
                first_download_id.unwrap(),
                downloader_id,
                first_hash,
                now,
                now
            ],
        )
        .unwrap();
        tx.commit().unwrap();
        drop(conn);

        let current = db.get_subscription(created.id).await.unwrap().unwrap();
        let rewound = db
            .update_subscription(
                created.id,
                current.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(1),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: true,
                },
            )
            .await
            .unwrap()
            .unwrap();

        assert!(
            db.list_media_downloads(Some(created.id), None, 100, 0)
                .await
                .unwrap()
                .is_empty()
        );
        let conn = open_connection(&db.path).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM media_relocation_jobs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        drop(conn);
        for (episode, expected) in [
            (1, "metadata_pending"),
            (2, "pending"),
            (3, "pending"),
            (4, "pending"),
            (5, "pending"),
        ] {
            let key = target_key("tv", 207, Some(1), Some(episode), None);
            assert_eq!(
                db.get_subscription_target(created.id, &key)
                    .await
                    .unwrap()
                    .unwrap()
                    .status,
                expected
            );
        }

        let first_key = target_key("tv", 207, Some(1), Some(1), None);
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscription_targets SET status = 'pending', air_date = '2020-01-01'
             WHERE subscription_id = ? AND target_key = ?",
            params![created.id, first_key],
        )
        .unwrap();
        drop(conn);
        let request = NewMediaDownload {
            subscription_id: Some(created.id),
            target_key: first_key.clone(),
            dedupe_key: format!("subscription:{}:{first_key}", created.id),
            site_id: Some(site_id),
            downloader_id: Some(downloader_id),
            source_site: "site".to_string(),
            downloader_name: "qB".to_string(),
            torrent_id: "torrent-1-redownload".to_string(),
            title: "Example.Show.S01E01.1080p.WEB-DL".to_string(),
            size: 1_024,
            release_json: "{}".to_string(),
            decision_json: "{}".to_string(),
            profile_snapshot_json: "{}".to_string(),
        };
        let queued = db
            .enqueue_subscription_media_download(rewound.version, &request)
            .await
            .unwrap()
            .unwrap();
        let claimed = db
            .claim_due_media_downloads("history-reset-worker", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(claimed.id, queued.id);
        assert!(
            db.mark_media_download_submitting(
                claimed.id,
                claimed.version,
                "history-reset-worker",
                &first_hash,
                60,
            )
            .await
            .unwrap()
        );
        let submitting = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert!(
            db.mark_media_download_submitted(
                submitting.id,
                submitting.version,
                "history-reset-worker",
                &first_hash,
            )
            .await
            .unwrap()
        );
        assert_eq!(
            db.get_subscription(created.id)
                .await
                .unwrap()
                .unwrap()
                .next_episode,
            Some(2),
            "cleared submitted targets must no longer be skipped"
        );
    }

    #[tokio::test]
    async fn deleting_a_submitted_record_waits_for_relocation_and_reopens_its_target() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&created, site_id, downloader_id);
        request.dedupe_key = format!("subscription:{}:{}", created.id, request.target_key);
        let infohash = "0123456789abcdef0123456789abcdef01234567";
        submit_test_download(&db, &request, "single-delete-worker", infohash).await;
        let submitted = db
            .list_media_downloads(Some(created.id), Some("submitted"), 10, 0)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let now = Utc::now().to_rfc3339();
        let future = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              torrent_name, stage, created_at, updated_at)
             VALUES (?, ?, ?, '/downloads', '/downloads', '/media', '/media',
                     'Example Show', 'copying', ?, ?)",
            params![submitted.id, downloader_id, infohash, now, now],
        )
        .unwrap();
        drop(conn);

        assert!(matches!(
            db.delete_media_download_record(submitted.id, submitted.version)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::RelocationActive
        ));
        assert!(db.get_media_download(submitted.id).await.unwrap().is_some());

        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs
             SET stage = 'completed', lease_owner = 'stale-worker', lease_until = ?
             WHERE media_download_id = ?",
            params![future, submitted.id],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            db.delete_media_download_record(submitted.id, submitted.version)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::RelocationActive
        ));

        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET lease_owner = NULL, lease_until = NULL
             WHERE media_download_id = ?",
            [submitted.id],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO media_downloads
             (subscription_id, target_key, dedupe_key, site_id, downloader_id,
              source_site, downloader_name, torrent_id, title, size, release_json,
              decision_json, profile_snapshot_json, status, created_at, updated_at, submitted_at)
             VALUES (?, ?, ?, ?, ?, 'site', 'qB', 'historical-torrent', 'Historical E03',
                     1024, '{}', '{}', '{}', 'submitted', ?, ?, ?)",
            params![
                created.id,
                request.target_key,
                format!("{}:history:test", request.dedupe_key),
                site_id,
                downloader_id,
                now,
                now,
                now,
            ],
        )
        .unwrap();
        let history_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              torrent_name, stage, created_at, updated_at)
             VALUES (?, ?, 'fedcba9876543210fedcba9876543210fedcba98', '/downloads',
                     '/downloads', '/media', '/media', 'Historical E03', 'copying', ?, ?)",
            params![history_id, downloader_id, now, now],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            db.delete_media_download_record(submitted.id, submitted.version)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::RelocationActive
        ));
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_relocation_jobs SET stage = 'cancelled'
             WHERE media_download_id = ?",
            [history_id],
        )
        .unwrap();
        drop(conn);
        assert!(matches!(
            db.delete_media_download_record(history_id, 0)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::Deleted {
                target_reopened: false,
                ..
            }
        ));
        let deleted = db
            .delete_media_download_record(submitted.id, submitted.version)
            .await
            .unwrap();
        assert!(matches!(
            deleted,
            MediaDownloadDeleteOutcome::Deleted {
                target_reopened: true,
                ..
            }
        ));
        assert!(db.get_media_download(submitted.id).await.unwrap().is_none());
        let conn = open_connection(&db.path).unwrap();
        assert_eq!(
            conn.query_row("SELECT count(*) FROM media_relocation_jobs", [], |row| {
                row.get::<_, i64>(0)
            })
            .unwrap(),
            0
        );
        drop(conn);
        let reopened = db.get_subscription(created.id).await.unwrap().unwrap();
        assert_eq!(reopened.next_episode, Some(3));
        assert_eq!(reopened.last_status.as_deref(), Some("awaiting_metadata"));
        assert!(reopened.enabled);
        assert_eq!(
            db.get_subscription_target(created.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "metadata_pending"
        );

        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscription_targets SET status = 'pending', air_date = '2020-01-01'
             WHERE subscription_id = ? AND target_key = ?",
            params![created.id, request.target_key],
        )
        .unwrap();
        drop(conn);
        let requeued = db
            .enqueue_subscription_media_download(reopened.version, &request)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(requeued.target_key, request.target_key);
        assert_eq!(requeued.status, "queued");
    }

    #[tokio::test]
    async fn target_side_manual_relocation_blocks_download_history_deletion() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&created, site_id, downloader_id);
        request.dedupe_key = format!("subscription:{}:{}", created.id, request.target_key);
        let infohash = "3456789abcdef0123456789abcdef0123456789a";
        submit_test_download(&db, &request, "manual-delete-guard", infohash).await;
        let submitted = db
            .list_media_downloads(Some(created.id), Some("submitted"), 10, 0)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let source_id = db
            .create_downloader(
                "manual-source",
                "qbittorrent",
                "http://127.0.0.1:9090",
                "",
                "",
            )
            .await
            .unwrap();
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash.to_string(), "target-side migration".to_string())],
            )
            .await
            .unwrap(),
            (1, 0)
        );

        assert!(matches!(
            db.delete_media_download_record(submitted.id, submitted.version)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::RelocationActive
        ));
        assert!(db.get_media_download(submitted.id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn redelivery_reservation_blocks_manual_migration_until_released() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&created, site_id, downloader_id);
        request.dedupe_key = format!("subscription:{}:{}", created.id, request.target_key);
        let infohash = "6789abcdef0123456789abcdef0123456789abcd";
        submit_test_download(&db, &request, "redelivery-reservation", infohash).await;
        let submitted = db
            .list_media_downloads(Some(created.id), Some("submitted"), 10, 0)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let source_id = db
            .create_downloader(
                "reservation-source",
                "qbittorrent",
                "http://127.0.0.1:9092",
                "",
                "",
            )
            .await
            .unwrap();
        let owner = "redelivery-reservation-test";
        assert!(
            db.reserve_media_download_redelivery(
                submitted.id,
                submitted.version,
                downloader_id,
                infohash,
                owner,
                60,
            )
            .await
            .unwrap()
        );
        assert!(
            db.renew_media_download_redelivery(
                submitted.id,
                submitted.version,
                downloader_id,
                infohash,
                owner,
                60,
            )
            .await
            .unwrap()
        );

        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash.to_string(), "reserved torrent".to_string())],
            )
            .await
            .unwrap(),
            (0, 1)
        );
        assert!(
            db.release_media_download_redelivery(submitted.id, submitted.version, owner)
                .await
                .unwrap()
        );
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash.to_string(), "reserved torrent".to_string())],
            )
            .await
            .unwrap(),
            (1, 0)
        );
        let released = db.get_media_download(submitted.id).await.unwrap().unwrap();
        assert_eq!(released.version, submitted.version);
        assert_eq!(released.lease_owner, None);
        assert_eq!(released.lease_until, None);
    }

    #[tokio::test]
    async fn expired_redelivery_reservation_cannot_revive_after_subscription_deletion() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&created, site_id, downloader_id);
        request.dedupe_key = format!("subscription:{}:{}", created.id, request.target_key);
        let infohash = "789abcdef0123456789abcdef0123456789abcde";
        submit_test_download(&db, &request, "expired-redelivery", infohash).await;
        let submitted = db
            .list_media_downloads(Some(created.id), Some("submitted"), 10, 0)
            .await
            .unwrap()
            .pop()
            .unwrap();
        let owner = "expired-redelivery-reservation";
        assert!(
            db.reserve_media_download_redelivery(
                submitted.id,
                submitted.version,
                downloader_id,
                infohash,
                owner,
                60,
            )
            .await
            .unwrap()
        );
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_downloads SET lease_until = ? WHERE id = ?",
            params![
                (Utc::now() - Duration::minutes(1)).to_rfc3339(),
                submitted.id
            ],
        )
        .unwrap();
        drop(conn);

        let current_subscription = db.get_subscription(created.id).await.unwrap().unwrap();
        assert!(
            db.delete_subscription(created.id, current_subscription.version)
                .await
                .unwrap()
        );
        assert!(
            !db.renew_media_download_redelivery(
                submitted.id,
                submitted.version,
                downloader_id,
                infohash,
                owner,
                60,
            )
            .await
            .unwrap(),
            "an expired side-effect reservation must never be revived after its subscription is deleted"
        );
        let orphaned_audit = db.get_media_download(submitted.id).await.unwrap().unwrap();
        assert_eq!(orphaned_audit.subscription_id, None);
        assert_eq!(orphaned_audit.lease_owner.as_deref(), Some(owner));
    }

    #[tokio::test]
    async fn target_side_manual_relocation_rolls_back_history_reset() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = db
            .create_subscription(&test_tv_subscription_request(
                209,
                1,
                3,
                site_id,
                downloader_id,
            ))
            .await
            .unwrap();
        let key = target_key("tv", 209, Some(1), Some(3), None);
        set_test_target_status(&db, created.id, &key, "submitted");
        let infohash = "456789abcdef0123456789abcdef0123456789ab";
        let now = Utc::now().to_rfc3339();
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "INSERT INTO media_downloads
             (subscription_id, target_key, dedupe_key, site_id, downloader_id,
              source_site, downloader_name, torrent_id, title, size, release_json,
              decision_json, profile_snapshot_json, infohash, status,
              created_at, updated_at, submitted_at)
             VALUES (?, ?, ?, ?, ?, 'site', 'qB', 'torrent-3', 'Example Show', 1024,
                     '{}', '{}', '{}', ?, 'submitted', ?, ?, ?)",
            params![
                created.id,
                key,
                format!("subscription:{}:{key}", created.id),
                site_id,
                downloader_id,
                infohash,
                now,
                now,
                now,
            ],
        )
        .unwrap();
        let download_id = conn.last_insert_rowid();
        drop(conn);
        let source_id = db
            .create_downloader(
                "manual-reset-source",
                "qbittorrent",
                "http://127.0.0.1:9091",
                "",
                "",
            )
            .await
            .unwrap();
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash.to_string(), "target-side reset guard".to_string())],
            )
            .await
            .unwrap(),
            (1, 0)
        );

        let error = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(3),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: true,
                },
            )
            .await
            .unwrap_err();

        assert!(error.to_string().contains("relocation jobs are active"));
        assert!(db.get_media_download(download_id).await.unwrap().is_some());
        assert_eq!(
            db.get_subscription_target(created.id, &key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "submitted"
        );
        assert_eq!(
            db.get_subscription(created.id)
                .await
                .unwrap()
                .unwrap()
                .version,
            created.version
        );
    }

    #[tokio::test]
    async fn unresolved_failed_download_freezes_downloader_identity_and_reconcile_snapshot() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&created, site_id, downloader_id);
        request.dedupe_key = format!("subscription:{}:{}", created.id, request.target_key);
        let queued = enqueue_linked_download(&db, &request).await;
        let infohash = "56789abcdef0123456789abcdef0123456789abc";
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_downloads
             SET status='failed', infohash=?, next_attempt_at=NULL,
                 lease_owner=NULL, lease_until=NULL, version=version+1,
                 last_error='submission outcome is unknown'
             WHERE id=?",
            params![infohash, queued.id],
        )
        .unwrap();
        drop(conn);
        let failed = db.get_media_download(queued.id).await.unwrap().unwrap();
        let downloader_snapshot = db.get_downloader(downloader_id).await.unwrap().unwrap();
        let source_id = db
            .create_downloader(
                "unresolved-source",
                "qbittorrent",
                "http://127.0.0.1:9093",
                "",
                "",
            )
            .await
            .unwrap();
        assert_eq!(
            db.enqueue_manual_media_relocation_jobs(
                source_id,
                downloader_id,
                "/archive",
                "/archive",
                &[(infohash.to_string(), "unresolved torrent".to_string())],
            )
            .await
            .unwrap(),
            (0, 1),
            "unknown qB submissions must freeze both migration endpoints"
        );

        let update_error = db
            .update_downloader(
                downloader_id,
                &downloader_snapshot.name,
                &downloader_snapshot.downloader_type,
                "http://127.0.0.1:9999",
                &downloader_snapshot.username,
                &downloader_snapshot.password,
            )
            .await
            .unwrap_err();
        assert!(
            update_error
                .to_string()
                .contains("downloads are being processed")
        );
        let delete_error = db.delete_downloader(downloader_id).await.unwrap_err();
        assert!(
            delete_error
                .to_string()
                .contains("downloads are being processed")
        );

        db.update_downloader(
            downloader_id,
            "renamed while awaiting verification",
            &downloader_snapshot.downloader_type,
            &downloader_snapshot.url,
            &downloader_snapshot.username,
            &downloader_snapshot.password,
        )
        .await
        .unwrap();
        let current_downloader = db.get_downloader(downloader_id).await.unwrap().unwrap();
        assert_ne!(
            current_downloader.updated_at,
            downloader_snapshot.updated_at
        );
        assert!(
            db.resolve_verified_failed_media_download(
                failed.id,
                failed.version,
                downloader_id,
                &downloader_snapshot.updated_at,
                false,
            )
            .await
            .unwrap()
            .is_none()
        );
        let unchanged = db.get_media_download(failed.id).await.unwrap().unwrap();
        assert_eq!(unchanged.status, "failed");
        assert_eq!(unchanged.version, failed.version);
        assert_eq!(
            db.get_subscription_target(created.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
    }

    #[tokio::test]
    async fn history_reset_rolls_back_when_a_target_has_active_relocation() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = db
            .create_subscription(&test_tv_subscription_request(
                208,
                1,
                3,
                site_id,
                downloader_id,
            ))
            .await
            .unwrap();
        let key = target_key("tv", 208, Some(1), Some(3), None);
        set_test_target_status(&db, created.id, &key, "submitted");
        let now = Utc::now().to_rfc3339();
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "INSERT INTO media_downloads
             (subscription_id, target_key, dedupe_key, site_id, downloader_id,
              source_site, downloader_name, torrent_id, title, size, release_json,
              decision_json, profile_snapshot_json, infohash, status,
              created_at, updated_at, submitted_at)
             VALUES (?, ?, ?, ?, ?, 'site', 'qB', 'torrent-3', 'Example Show', 1024,
                     '{}', '{}', '{}', 'abcdef0123456789abcdef0123456789abcdef01',
                     'submitted', ?, ?, ?)",
            params![
                created.id,
                key,
                format!("subscription:{}:{key}", created.id),
                site_id,
                downloader_id,
                now,
                now,
                now,
            ],
        )
        .unwrap();
        let download_id = conn.last_insert_rowid();
        conn.execute(
            "INSERT INTO media_relocation_jobs
             (media_download_id, downloader_id, infohash, source_qb_path,
              source_openlist_path, target_openlist_path, target_qb_path,
              torrent_name, stage, created_at, updated_at)
             VALUES (?, ?, 'abcdef0123456789abcdef0123456789abcdef01', '/downloads',
                     '/downloads', '/media', '/media', 'Example Show', 'copying', ?, ?)",
            params![download_id, downloader_id, now, now],
        )
        .unwrap();
        drop(conn);

        assert!(
            !db.delete_subscription(created.id, created.version)
                .await
                .unwrap(),
            "an active relocation must retain its subscription and target identity"
        );

        let error = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(3),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: true,
                },
            )
            .await
            .unwrap_err();
        assert!(error.to_string().contains("relocation jobs are active"));
        assert!(db.get_media_download(download_id).await.unwrap().is_some());
        assert_eq!(
            db.get_subscription_target(created.id, &key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "submitted"
        );
        assert_eq!(
            db.get_subscription(created.id)
                .await
                .unwrap()
                .unwrap()
                .version,
            created.version
        );
    }

    #[tokio::test]
    async fn unresolved_queued_target_preserves_failed_download_evidence() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&created, site_id, downloader_id);
        request.dedupe_key = format!("subscription:{}:{}", created.id, request.target_key);
        let queued = enqueue_linked_download(&db, &request).await;
        assert!(
            !db.delete_subscription(created.id, created.version)
                .await
                .unwrap(),
            "a queued download must keep its subscription identity"
        );
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_downloads
             SET status = 'failed', next_attempt_at = NULL, lease_owner = NULL,
                 lease_until = NULL, version = version + 1,
                 last_error = 'submission outcome is unknown'
             WHERE id = ?",
            [queued.id],
        )
        .unwrap();
        drop(conn);
        let failed = db.get_media_download(queued.id).await.unwrap().unwrap();

        assert!(
            !db.delete_subscription(created.id, created.version)
                .await
                .unwrap(),
            "an unknown qB submission must retain its reconciliation context"
        );

        assert!(matches!(
            db.delete_media_download_record(failed.id, failed.version)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::DownloadActive
        ));
        let reset_error = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: created.next_episode,
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: true,
                },
            )
            .await
            .unwrap_err();
        assert!(reset_error.to_string().contains("submission is unresolved"));
        assert!(db.get_media_download(failed.id).await.unwrap().is_some());
        assert_eq!(
            db.get_subscription_target(created.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        assert_eq!(
            db.get_subscription(created.id)
                .await
                .unwrap()
                .unwrap()
                .version,
            created.version
        );
    }

    #[tokio::test]
    async fn idle_subscription_can_be_deleted_without_orphaning_work() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;

        assert!(
            db.delete_subscription(created.id, created.version)
                .await
                .unwrap()
        );
        assert!(db.get_subscription(created.id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn subscription_edits_reanchor_metadata_without_crossing_active_targets() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = db
            .create_subscription(&test_tv_subscription_request(
                105,
                1,
                10,
                site_id,
                downloader_id,
            ))
            .await
            .unwrap();
        let season_two_key = target_key("tv", 105, Some(2), Some(1), None);
        let submitted_key = target_key("tv", 105, Some(1), Some(5), None);
        let queued_key = target_key("tv", 105, Some(1), Some(6), None);
        let now = Utc::now().to_rfc3339();
        let conn = open_connection(&db.path).unwrap();
        for (key, season, episode, status) in [
            (&season_two_key, 2, 1, "skipped"),
            (&submitted_key, 1, 5, "submitted"),
            (&queued_key, 1, 6, "queued"),
        ] {
            conn.execute(
                "INSERT INTO subscription_targets
                 (subscription_id, target_key, season, episode, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?)",
                params![created.id, key, season, episode, status, now, now],
            )
            .unwrap();
        }
        drop(conn);

        let moved = db
            .update_subscription(
                created.id,
                created.version,
                &UpdateSubscription {
                    season: Some(2),
                    next_episode: Some(1),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(moved.season, Some(2));
        assert_eq!(moved.next_episode, Some(1));
        assert_eq!(moved.start_episode, Some(1));
        let reanchored = db
            .get_subscription_target(created.id, &season_two_key)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(reanchored.status, "metadata_pending");
        assert_eq!(reanchored.air_date, None);

        assert!(
            db.update_subscription(
                created.id,
                moved.version,
                &UpdateSubscription {
                    season: Some(1),
                    next_episode: Some(6),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .is_err()
        );

        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscription_targets SET status = 'queued'
             WHERE subscription_id = ? AND target_key = ?",
            params![created.id, season_two_key],
        )
        .unwrap();
        drop(conn);
        let rules_only = db
            .update_subscription(
                created.id,
                moved.version,
                &UpdateSubscription {
                    season: Some(2),
                    next_episode: Some(1),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: Some("/new/path".to_string()),
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .unwrap()
            .unwrap();
        assert_eq!(rules_only.start_episode, Some(1));
        assert_eq!(rules_only.last_status.as_deref(), Some("queued"));
        assert_eq!(
            db.get_subscription_target(created.id, &season_two_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        assert!(
            db.update_subscription(
                created.id,
                rules_only.version,
                &UpdateSubscription {
                    season: Some(2),
                    next_episode: Some(2),
                    absolute_episode: None,
                    quality_profile_id: 1,
                    downloader_id,
                    site_ids: vec![site_id],
                    save_path: None,
                    enabled: true,
                    reset_download_history: false,
                },
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn claimed_subscription_errors_release_only_the_current_owner_version() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let created = create_tv_subscription(&db, site_id, downloader_id).await;
        db.claim_subscription(created.id, "error-owner", 60)
            .await
            .unwrap()
            .unwrap();
        let retry_at = (Utc::now() + Duration::minutes(10)).to_rfc3339();

        assert!(
            !db.release_claimed_subscription_after_error(
                created.id,
                "other-owner",
                &target_key("tv", 42, Some(1), Some(3), None),
                &retry_at,
                "wrong owner",
            )
            .await
            .unwrap()
        );
        assert!(
            db.get_subscription(created.id)
                .await
                .unwrap()
                .unwrap()
                .lease_owner
                .is_some()
        );
        assert!(
            db.release_claimed_subscription_after_error(
                created.id,
                "error-owner",
                &target_key("tv", 42, Some(1), Some(3), None),
                &retry_at,
                "quality profile disappeared",
            )
            .await
            .unwrap()
        );
        let released = db.get_subscription(created.id).await.unwrap().unwrap();
        assert_eq!(released.lease_owner, None);
        assert_eq!(released.lease_until, None);
        assert_eq!(released.last_status.as_deref(), Some("error"));
        assert_eq!(
            released.last_error.as_deref(),
            Some("quality profile disappeared")
        );
        assert_eq!(released.next_run_at, retry_at);
    }

    #[tokio::test]
    async fn active_subscription_lease_blocks_its_download_claim_until_scan_finishes() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let queued =
            enqueue_linked_download(&db, &new_download(&subscription, site_id, downloader_id))
                .await;
        let claimed_subscription = db
            .claim_subscription(subscription.id, "scan-worker", 60)
            .await
            .unwrap()
            .unwrap();

        assert!(
            db.claim_due_media_downloads("download-worker", 60, 1)
                .await
                .unwrap()
                .is_empty()
        );
        assert!(
            db.finish_subscription_scan(
                subscription.id,
                claimed_subscription.version,
                "scan-worker",
                &Utc::now().to_rfc3339(),
                "queued",
                None,
            )
            .await
            .unwrap()
        );
        let claimed = db
            .claim_due_media_downloads("download-worker", 60, 1)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 1);
        assert_eq!(claimed[0].id, queued.id);
    }

    #[tokio::test]
    async fn active_download_lease_blocks_subscription_claim_and_stale_error_release() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let targets = vec![
            test_target_seed(
                106,
                1,
                3,
                Some("2020-01-01"),
                SubscriptionTargetSeedStatus::Pending,
            ),
            test_target_seed(106, 1, 4, None, SubscriptionTargetSeedStatus::Skipped),
        ];
        let subscription = db
            .create_subscription_with_targets(
                &test_tv_subscription_request(106, 1, 3, site_id, downloader_id),
                &targets,
                None,
                Some("waiting"),
            )
            .await
            .unwrap();
        let request = new_download(&subscription, site_id, downloader_id);
        let queued = enqueue_linked_download(&db, &request).await;
        let stale_scan = db
            .claim_subscription(subscription.id, "stale-scan", 60)
            .await
            .unwrap()
            .unwrap();
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE subscriptions SET lease_until = '1970-01-01T00:00:00Z' WHERE id = ?",
            [subscription.id],
        )
        .unwrap();
        drop(conn);

        let fetching = db
            .claim_due_media_downloads("download-first", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(fetching.id, queued.id);
        assert!(
            db.claim_subscription(subscription.id, "late-scan", 60)
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            db.claim_due_subscriptions("late-batch", 60, 10)
                .await
                .unwrap()
                .iter()
                .all(|candidate| candidate.id != subscription.id)
        );

        let infohash = "cdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcdcd";
        assert!(
            db.mark_media_download_submitting(
                fetching.id,
                fetching.version,
                "download-first",
                infohash,
                60,
            )
            .await
            .unwrap()
        );
        let submitting = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert!(
            db.mark_media_download_submitted(
                submitting.id,
                submitting.version,
                "download-first",
                infohash,
            )
            .await
            .unwrap()
        );
        let retry_at = (Utc::now() + Duration::minutes(10)).to_rfc3339();
        assert!(
            db.release_claimed_subscription_after_error(
                subscription.id,
                "stale-scan",
                &request.target_key,
                &retry_at,
                "late scan failure",
            )
            .await
            .unwrap()
        );
        let completed = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert_eq!(completed.last_status.as_deref(), Some("completed"));
        assert_eq!(completed.last_error, None);
        assert_eq!(completed.next_episode, None);
        assert_eq!(completed.lease_owner, None);
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "submitted"
        );
        assert!(stale_scan.version < completed.version);
    }

    #[tokio::test]
    async fn media_download_list_filters_before_sql_pagination() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut conn = open_connection(&db.path).unwrap();
        let tx = conn.transaction().unwrap();
        let now = Utc::now().to_rfc3339();
        let mut subscription_ids = Vec::new();
        let mut cancelled_ids = Vec::new();

        for index in 0..520 {
            let status = if index % 2 == 0 {
                "cancelled"
            } else {
                "queued"
            };
            tx.execute(
                "INSERT INTO media_downloads
                 (subscription_id, target_key, dedupe_key, site_id, downloader_id,
                  source_site, downloader_name, torrent_id, title, size, release_json,
                  decision_json, profile_snapshot_json, status, created_at, updated_at)
                 VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 1024, '{}', '{}', '{}', ?, ?, ?)",
                params![
                    subscription.id,
                    format!("target-{index}"),
                    format!("dedupe-{index}"),
                    site_id,
                    downloader_id,
                    "media-test-site",
                    "media-test-downloader",
                    format!("torrent-{index}"),
                    format!("Example.Show.S01E{index:03}.1080p.WEB-DL"),
                    status,
                    now,
                    now,
                ],
            )
            .unwrap();
            let id = tx.last_insert_rowid();
            subscription_ids.push(id);
            if status == "cancelled" {
                cancelled_ids.push(id);
            }
        }

        tx.execute(
            "INSERT INTO media_downloads
             (subscription_id, target_key, dedupe_key, site_id, downloader_id,
              source_site, downloader_name, torrent_id, title, size, release_json,
              decision_json, profile_snapshot_json, status, created_at, updated_at)
             VALUES (NULL, 'detached-target', 'detached-dedupe', ?, ?, ?, ?, ?, ?,
                     1024, '{}', '{}', '{}', 'failed', ?, ?)",
            params![
                site_id,
                downloader_id,
                "media-test-site",
                "media-test-downloader",
                "detached-torrent",
                "Detached.Release.1080p.WEB-DL",
                now,
                now,
            ],
        )
        .unwrap();
        let detached_id = tx.last_insert_rowid();
        tx.commit().unwrap();
        drop(conn);

        let global = db.list_media_downloads(None, None, 3, 0).await.unwrap();
        assert_eq!(
            global
                .iter()
                .map(|download| download.id)
                .collect::<Vec<_>>(),
            vec![
                detached_id,
                subscription_ids[subscription_ids.len() - 1],
                subscription_ids[subscription_ids.len() - 2],
            ]
        );

        let subscription_latest = db
            .list_media_downloads(Some(subscription.id), None, 3, 0)
            .await
            .unwrap();
        assert_eq!(
            subscription_latest
                .iter()
                .map(|download| download.id)
                .collect::<Vec<_>>(),
            subscription_ids
                .iter()
                .rev()
                .take(3)
                .copied()
                .collect::<Vec<_>>()
        );

        let failed = db
            .list_media_downloads(None, Some("failed"), 10, 0)
            .await
            .unwrap();
        assert_eq!(
            failed
                .iter()
                .map(|download| download.id)
                .collect::<Vec<_>>(),
            vec![detached_id]
        );

        let deep_cancelled_page = db
            .list_media_downloads(Some(subscription.id), Some("cancelled"), 20, 250)
            .await
            .unwrap();
        assert_eq!(
            deep_cancelled_page
                .iter()
                .map(|download| download.id)
                .collect::<Vec<_>>(),
            cancelled_ids
                .iter()
                .rev()
                .skip(250)
                .take(20)
                .copied()
                .collect::<Vec<_>>()
        );
        assert_eq!(deep_cancelled_page.len(), 10);

        let first_cursor_page = db
            .list_media_downloads(Some(subscription.id), Some("cancelled"), 20, 0)
            .await
            .unwrap();
        let cursor = first_cursor_page.last().unwrap().id;
        let removed = first_cursor_page.first().unwrap();
        assert!(matches!(
            db.delete_media_download_record(removed.id, removed.version)
                .await
                .unwrap(),
            MediaDownloadDeleteOutcome::Deleted { .. }
        ));
        let second_cursor_page = db
            .list_media_downloads_before(Some(subscription.id), Some("cancelled"), 20, cursor)
            .await
            .unwrap();
        assert_eq!(
            second_cursor_page.first().map(|download| download.id),
            cancelled_ids.iter().rev().nth(20).copied(),
            "keyset pagination must not skip a boundary row after a concurrent deletion"
        );
    }

    #[tokio::test]
    async fn infohash_is_idempotent_per_downloader_and_case_insensitive() {
        let (_dir, db, site_id, downloader_a) = database_with_media_references().await;
        let downloader_b = db
            .create_downloader(
                "media-test-downloader-b",
                "qbittorrent",
                "http://127.0.0.1:8081",
                "",
                "",
            )
            .await
            .unwrap();
        let subscription = create_tv_subscription(&db, site_id, downloader_a).await;
        let mut first = new_download(&subscription, site_id, downloader_a);
        first.subscription_id = None;
        first.dedupe_key = "hash-downloader-a".to_string();
        let mut second = first.clone();
        second.dedupe_key = "hash-downloader-b".to_string();
        second.downloader_id = Some(downloader_b);
        second.downloader_name = "media-test-downloader-b".to_string();
        let first = db.enqueue_media_download(&first).await.unwrap();
        let second = db.enqueue_media_download(&second).await.unwrap();
        let claimed = db
            .claim_due_media_downloads("hash-worker", 60, 2)
            .await
            .unwrap();
        assert_eq!(claimed.len(), 2);
        let hash = "ABCDEF0123456789ABCDEF0123456789ABCDEF01";
        for download in &claimed {
            assert!(
                db.mark_media_download_submitting(
                    download.id,
                    download.version,
                    "hash-worker",
                    hash,
                    60,
                )
                .await
                .unwrap()
            );
        }
        assert_eq!(
            db.get_media_download_by_infohash(downloader_a, &hash.to_ascii_lowercase())
                .await
                .unwrap()
                .unwrap()
                .id,
            first.id
        );
        assert_eq!(
            db.get_media_download_by_infohash(downloader_b, hash)
                .await
                .unwrap()
                .unwrap()
                .id,
            second.id
        );

        let mut duplicate = new_download(&subscription, site_id, downloader_a);
        duplicate.subscription_id = None;
        duplicate.dedupe_key = "hash-downloader-a-duplicate".to_string();
        let duplicate = db.enqueue_media_download(&duplicate).await.unwrap();
        let duplicate = db
            .claim_due_media_downloads("duplicate-worker", 60, 1)
            .await
            .unwrap()
            .into_iter()
            .find(|download| download.id == duplicate.id)
            .unwrap();
        assert!(
            db.mark_media_download_submitting(
                duplicate.id,
                duplicate.version,
                "duplicate-worker",
                &hash.to_ascii_lowercase(),
                60,
            )
            .await
            .is_err()
        );
    }

    #[tokio::test]
    async fn download_attempts_stop_at_max_with_one_final_reconciliation() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&subscription, site_id, downloader_id);
        request.subscription_id = None;
        request.dedupe_key = "attempt-cap".to_string();
        let queued = db.enqueue_media_download(&request).await.unwrap();

        for expected_attempt in 1..=MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            let owner = format!("attempt-worker-{expected_attempt}");
            let claimed = db
                .claim_due_media_downloads(&owner, 60, 1)
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(claimed.id, queued.id);
            assert_eq!(claimed.attempts, expected_attempt);
            assert!(
                db.release_media_download_after_error(claimed.id, &owner, "fetch failed", false,)
                    .await
                    .unwrap()
            );
            if expected_attempt < MEDIA_DOWNLOAD_MAX_ATTEMPTS {
                make_download_due(&db, queued.id);
            }
        }
        let failed = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        assert!(
            db.claim_due_media_downloads("extra-worker", 60, 1)
                .await
                .unwrap()
                .is_empty()
        );

        request.dedupe_key = "final-reconciliation".to_string();
        let reconcile = db.enqueue_media_download(&request).await.unwrap();
        for expected_attempt in 1..MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            let owner = format!("pre-submit-worker-{expected_attempt}");
            let claimed = db
                .claim_due_media_downloads(&owner, 60, 1)
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(claimed.id, reconcile.id);
            db.release_media_download_after_error(claimed.id, &owner, "fetch failed", false)
                .await
                .unwrap();
            make_download_due(&db, reconcile.id);
        }
        let submitting = db
            .claim_due_media_downloads("submit-worker", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(submitting.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        assert!(
            db.mark_media_download_submitting(
                submitting.id,
                submitting.version,
                "submit-worker",
                "1111111111111111111111111111111111111111",
                60,
            )
            .await
            .unwrap()
        );
        assert!(
            db.release_media_download_after_error(
                submitting.id,
                "submit-worker",
                "submission result unknown",
                false,
            )
            .await
            .unwrap()
        );
        make_download_due(&db, reconcile.id);
        let reconciling = db
            .claim_due_media_downloads("reconcile-worker", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(reconciling.status, "reconciling");
        assert_eq!(reconciling.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        db.release_media_download_after_error(
            reconciling.id,
            "reconcile-worker",
            "reconciliation unavailable",
            false,
        )
        .await
        .unwrap();
        let failed = db.get_media_download(reconcile.id).await.unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
    }

    #[tokio::test]
    async fn final_fetch_failure_restores_current_target_and_makes_subscription_due() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&subscription, site_id, downloader_id);
        request.dedupe_key = "linked-final-fetch-failure".to_string();
        let queued = enqueue_linked_download(&db, &request).await;
        let mut racing_scan = None;
        for expected_attempt in 1..=MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            let owner = format!("linked-failure-{expected_attempt}");
            let claimed = db
                .claim_due_media_downloads(&owner, 60, 1)
                .await
                .unwrap()
                .pop()
                .unwrap();
            assert_eq!(claimed.attempts, expected_attempt);
            if expected_attempt == MEDIA_DOWNLOAD_MAX_ATTEMPTS {
                let conn = open_connection(&db.path).unwrap();
                conn.execute(
                    "UPDATE media_downloads
                     SET lease_until = '1970-01-01T00:00:00Z' WHERE id = ?",
                    [queued.id],
                )
                .unwrap();
                drop(conn);
                racing_scan = db
                    .claim_subscription(subscription.id, "failure-racing-scan", 60)
                    .await
                    .unwrap();
                assert!(racing_scan.is_some());
            }
            assert!(
                db.release_media_download_after_error(
                    claimed.id,
                    &owner,
                    "delivery failed",
                    false,
                )
                .await
                .unwrap()
            );
            if expected_attempt < MEDIA_DOWNLOAD_MAX_ATTEMPTS {
                make_download_due(&db, queued.id);
            }
        }
        assert_eq!(
            db.get_media_download(queued.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
        let actionable = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert_eq!(actionable.last_status.as_deref(), Some("error"));
        assert_eq!(actionable.last_error.as_deref(), Some("delivery failed"));
        assert_eq!(actionable.lease_owner, None);
        assert!(
            chrono::DateTime::parse_from_rfc3339(&actionable.next_run_at).unwrap() <= Utc::now()
        );
        let racing_scan = racing_scan.unwrap();
        assert!(
            !db.finish_subscription_scan(
                subscription.id,
                racing_scan.version,
                "failure-racing-scan",
                &Utc::now().to_rfc3339(),
                "queued",
                None,
            )
            .await
            .unwrap()
        );
        assert!(
            db.claim_subscription(subscription.id, "retry-scan", 60)
                .await
                .unwrap()
                .is_some()
        );
    }

    #[tokio::test]
    async fn unknown_external_submission_failure_keeps_target_queued() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&subscription, site_id, downloader_id);
        request.dedupe_key = "linked-unknown-submission".to_string();
        let queued = enqueue_linked_download(&db, &request).await;
        for expected_attempt in 1..MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            let owner = format!("pre-submit-failure-{expected_attempt}");
            let claimed = db
                .claim_due_media_downloads(&owner, 60, 1)
                .await
                .unwrap()
                .pop()
                .unwrap();
            db.release_media_download_after_error(claimed.id, &owner, "fetch failed", false)
                .await
                .unwrap();
            make_download_due(&db, queued.id);
        }
        let fetching = db
            .claim_due_media_downloads("unknown-submitter", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(fetching.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        assert!(
            db.mark_media_download_submitting(
                fetching.id,
                fetching.version,
                "unknown-submitter",
                "efefefefefefefefefefefefefefefefefefefef",
                60,
            )
            .await
            .unwrap()
        );
        assert!(
            db.release_media_download_after_error(
                fetching.id,
                "unknown-submitter",
                "submission result unknown",
                false,
            )
            .await
            .unwrap()
        );
        make_download_due(&db, queued.id);
        let reconciling = db
            .claim_due_media_downloads("unknown-reconciler", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(reconciling.status, "reconciling");
        assert!(
            db.release_media_download_after_error(
                reconciling.id,
                "unknown-reconciler",
                "reconciliation unavailable",
                false,
            )
            .await
            .unwrap()
        );
        assert_eq!(
            db.get_media_download(queued.id)
                .await
                .unwrap()
                .unwrap()
                .status,
            "failed"
        );
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "queued"
        );
        let unchanged = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert_ne!(unchanged.last_status.as_deref(), Some("error"));
    }

    #[tokio::test]
    async fn recovery_fails_an_expired_final_fetch_attempt() {
        let (_dir, db, site_id, downloader_id) = database_with_media_references().await;
        let subscription = create_tv_subscription(&db, site_id, downloader_id).await;
        let mut request = new_download(&subscription, site_id, downloader_id);
        request.dedupe_key = "recovery-attempt-cap".to_string();
        let queued = enqueue_linked_download(&db, &request).await;
        for expected_attempt in 1..MEDIA_DOWNLOAD_MAX_ATTEMPTS {
            let owner = format!("recovery-worker-{expected_attempt}");
            let claimed = db
                .claim_due_media_downloads(&owner, 60, 1)
                .await
                .unwrap()
                .pop()
                .unwrap();
            db.release_media_download_after_error(claimed.id, &owner, "temporary", false)
                .await
                .unwrap();
            make_download_due(&db, queued.id);
        }
        let final_claim = db
            .claim_due_media_downloads("crashed-worker", 60, 1)
            .await
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(final_claim.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        let conn = open_connection(&db.path).unwrap();
        conn.execute(
            "UPDATE media_downloads SET lease_until = '1970-01-01T00:00:00Z' WHERE id = ?",
            [queued.id],
        )
        .unwrap();
        drop(conn);

        let racing_scan = db
            .claim_subscription(subscription.id, "recovery-racing-scan", 60)
            .await
            .unwrap()
            .unwrap();

        let (_, recovered) = db.recover_expired_media_leases().await.unwrap();
        assert_eq!(recovered, 1);
        let failed = db.get_media_download(queued.id).await.unwrap().unwrap();
        assert_eq!(failed.status, "failed");
        assert_eq!(failed.attempts, MEDIA_DOWNLOAD_MAX_ATTEMPTS);
        assert_eq!(
            db.get_subscription_target(subscription.id, &request.target_key)
                .await
                .unwrap()
                .unwrap()
                .status,
            "pending"
        );
        let actionable = db.get_subscription(subscription.id).await.unwrap().unwrap();
        assert_eq!(actionable.last_status.as_deref(), Some("error"));
        assert_eq!(
            actionable.last_error.as_deref(),
            Some("download delivery failed after lease recovery")
        );
        assert_eq!(actionable.lease_owner, None);
        assert!(
            !db.finish_subscription_scan(
                subscription.id,
                racing_scan.version,
                "recovery-racing-scan",
                &Utc::now().to_rfc3339(),
                "queued",
                None,
            )
            .await
            .unwrap()
        );
        assert!(
            db.claim_subscription(subscription.id, "recovered-retry-scan", 60)
                .await
                .unwrap()
                .is_some()
        );
    }
}
