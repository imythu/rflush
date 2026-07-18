pub fn normalize_path(value: &str) -> Result<String, String> {
    let value = value.trim().replace('\\', "/");
    if value.is_empty() {
        return Err("路径不能为空".to_string());
    }
    let absolute = value.starts_with('/');
    let mut parts = Vec::new();
    for part in value.split('/') {
        match part {
            "" | "." => {}
            ".." => return Err("路径不能包含 ..".to_string()),
            other => parts.push(other),
        }
    }
    if parts.is_empty() {
        return Ok("/".to_string());
    }
    Ok(format!(
        "{}{}",
        if absolute { "/" } else { "" },
        parts.join("/")
    ))
}

pub fn is_path_prefix(prefix: &str, path: &str) -> bool {
    prefix == "/"
        || path == prefix
        || path
            .strip_prefix(prefix)
            .is_some_and(|tail| tail.starts_with('/'))
}

pub fn translate_path(path: &str, from: &str, to: &str) -> Result<String, String> {
    let path = normalize_path(path)?;
    let from = normalize_path(from)?;
    let to = normalize_path(to)?;
    if !is_path_prefix(&from, &path) {
        return Err(format!("路径 {path} 不在映射根目录 {from} 下"));
    }
    let suffix = if from == "/" {
        path.trim_start_matches('/')
    } else {
        path.strip_prefix(&from)
            .unwrap_or_default()
            .trim_start_matches('/')
    };
    if suffix.is_empty() {
        Ok(to)
    } else if to == "/" {
        Ok(format!("/{suffix}"))
    } else {
        Ok(format!("{to}/{suffix}"))
    }
}

pub fn validate_non_overlapping(paths: &[String]) -> Result<(), String> {
    let mut normalized = paths
        .iter()
        .map(|path| normalize_path(path))
        .collect::<Result<Vec<_>, _>>()?;
    normalized.sort();
    for (index, left) in normalized.iter().enumerate() {
        for right in normalized.iter().skip(index + 1) {
            if is_path_prefix(left, right) || is_path_prefix(right, left) {
                return Err(format!("路径映射重叠: {left} 与 {right}"));
            }
        }
    }
    Ok(())
}

pub fn parent_and_name(path: &str) -> Result<(String, String), String> {
    let path = normalize_path(path)?;
    if path == "/" {
        return Err("根目录不能作为复制对象".to_string());
    }
    let (parent, name) = path.rsplit_once('/').unwrap_or(("/", path.as_str()));
    let parent = if parent.is_empty() { "/" } else { parent };
    Ok((parent.to_string(), name.to_string()))
}

fn encode_openlist_task_ids(ids: impl IntoIterator<Item = String>) -> Option<String> {
    let ids = ids
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect::<Vec<_>>();
    (!ids.is_empty())
        .then(|| serde_json::to_string(&ids).ok())
        .flatten()
}

fn decode_openlist_task_ids(value: &str) -> Vec<String> {
    serde_json::from_str::<Vec<String>>(value)
        .unwrap_or_else(|_| vec![value.to_string()])
        .into_iter()
        .filter(|id| !id.trim().is_empty())
        .collect()
}

pub fn category_directory(media_type: &str, year: Option<u32>) -> String {
    let category = match media_type.trim().to_ascii_lowercase().as_str() {
        "movie" | "电影" => "电影".to_string(),
        "anime" | "动漫" => "动漫".to_string(),
        "concert" | "演唱会" => "演唱会".to_string(),
        "year" | "年份" => year
            .map(|v| v.to_string())
            .unwrap_or_else(|| "年份".to_string()),
        "tv" | "电视" | "电视剧" => "电视剧".to_string(),
        _ => "电视剧".to_string(),
    };
    category
}

pub fn category_year_directory(media_type: &str, year: Option<u32>) -> String {
    let category = category_directory(media_type, None);
    year.map(|year| format!("{category}/{year}"))
        .unwrap_or(category)
}

pub fn join_path(root: &str, child: &str) -> Result<String, String> {
    let root = normalize_path(root)?;
    let child = normalize_path(child)?;
    Ok(if root == "/" {
        format!("/{}", child.trim_start_matches('/'))
    } else {
        format!("{}/{}", root, child.trim_start_matches('/'))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prefix_is_segment_aware() {
        assert!(is_path_prefix("/pt", "/pt/a"));
        assert!(is_path_prefix("/pt", "/pt"));
        assert!(!is_path_prefix("/pt", "/pt2/a"));
    }

    #[test]
    fn translation_preserves_relative_suffix() {
        assert_eq!(
            translate_path("/pt/show/file.mkv", "/pt", "/local/pt").unwrap(),
            "/local/pt/show/file.mkv"
        );
        assert!(translate_path("/pt2/file", "/pt", "/local/pt").is_err());
    }

    #[test]
    fn overlapping_ancestor_paths_are_rejected() {
        assert!(validate_non_overlapping(&["/pt".into(), "/pt/a".into()]).is_err());
        assert!(validate_non_overlapping(&["/pt".into(), "/pt2".into()]).is_ok());
    }

    #[test]
    fn parent_name_handles_top_level() {
        assert_eq!(
            parent_and_name("/file.mkv").unwrap(),
            ("/".into(), "file.mkv".into())
        );
        assert!(parent_and_name("/").is_err());
    }

    #[test]
    fn openlist_task_ids_round_trip_and_accept_legacy_value() {
        let encoded =
            encode_openlist_task_ids(["task-a".to_string(), "task-b".to_string()]).unwrap();
        assert_eq!(
            decode_openlist_task_ids(&encoded),
            vec!["task-a".to_string(), "task-b".to_string()]
        );
        assert_eq!(
            decode_openlist_task_ids("legacy-task"),
            vec!["legacy-task".to_string()]
        );
    }

    #[test]
    fn category_has_year_layer_when_known() {
        assert_eq!(category_year_directory("movie", Some(2026)), "电影/2026");
        assert_eq!(category_year_directory("tv", None), "电视剧");
        assert_eq!(category_year_directory("电视剧", Some(2026)), "电视剧/2026");
    }

    #[test]
    fn historical_jobs_require_a_valid_sha1_infohash() {
        assert!(valid_infohash("eadb91a4769b1fad89e0dd3a930523e7fc5814b8"));
        assert!(!valid_infohash(""));
        assert!(!valid_infohash("not-a-valid-infohash"));
    }

    #[test]
    fn qb_completion_timestamp_and_seeding_states_override_byte_rounding() {
        assert!(torrent_is_complete(1, 99, 100, "downloading"));
        assert!(torrent_is_complete(0, 99, 100, "stalledUP"));
        assert!(!torrent_is_complete(0, 99, 100, "stalledDL"));
    }

    #[test]
    fn source_is_only_removed_after_target_is_seeding() {
        assert!(target_torrent_is_seeding("uploading"));
        assert!(target_torrent_is_seeding("stalledUP"));
        assert!(target_torrent_is_seeding("forcedUP"));
        assert!(!target_torrent_is_seeding("queuedUP"));
        assert!(!target_torrent_is_seeding("checkingUP"));
        assert!(!target_torrent_is_seeding("pausedUP"));
        assert!(!target_torrent_is_seeding("missingFiles"));
        assert!(!target_torrent_is_seeding("error"));
    }

    #[test]
    fn qb_content_path_is_the_torrent_copy_root() {
        assert_eq!(select_torrent_content_root("/pt/Show", "/pt"), "/pt/Show");
        assert_eq!(select_torrent_content_root("", "/pt"), "/pt");
    }

    #[test]
    fn torrent_manifest_produces_unique_top_level_copy_items() {
        let files = normalize_torrent_file_paths(vec![
            TorrentFileInfo {
                path: "Show/Season 01/E01.mkv".to_string(),
                size: 10,
            },
            TorrentFileInfo {
                path: "Show/Season 01/E02.mkv".to_string(),
                size: 20,
            },
            TorrentFileInfo {
                path: "poster.jpg".to_string(),
                size: 1,
            },
        ])
        .unwrap();
        assert_eq!(
            torrent_top_level_items(&files).unwrap(),
            vec!["Show".to_string(), "poster.jpg".to_string()]
        );
        assert_eq!(
            manifest_parent_directories(&files),
            vec!["Show/Season 01".to_string(), "Show".to_string()]
        );
        assert!(normalize_manifest_path("../outside.mkv").is_err());
    }

    #[test]
    fn relative_content_path_is_preserved_under_the_copied_root() {
        assert_eq!(
            relative_path("/pt/Show/E01.mkv", "/pt/Show").unwrap(),
            "E01.mkv"
        );
        assert_eq!(relative_path("/pt/Show", "/pt/Show").unwrap(), "");
    }
}
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::db::{Database, MediaRelocationJob, OpenListConfig};
use crate::downloader::{
    AddTorrentOptions, DownloaderClient, DownloaderClientPool, TorrentFileInfo, TorrentInfo,
};
use crate::media::models::media_download_category;
use crate::openlist::OpenListClient;

pub struct RelocationScheduler {
    db: Database,
    pool: Arc<DownloaderClientPool>,
    enabled_by_environment: bool,
    running: AtomicBool,
    scan_requested: Notify,
    owner: String,
}

impl RelocationScheduler {
    pub fn new(
        db: Database,
        pool: Arc<DownloaderClientPool>,
        enabled_by_environment: bool,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            pool,
            enabled_by_environment,
            running: AtomicBool::new(true),
            scan_requested: Notify::new(),
            owner: format!(
                "relocation-{}-{}",
                std::process::id(),
                Utc::now().timestamp_nanos_opt().unwrap_or_default()
            ),
        })
    }

    pub async fn start(&self) {
        if !self.enabled_by_environment {
            info!("media relocation scheduler disabled because SELF_USE is not true");
            return;
        }
        while self.running.load(Ordering::Relaxed) {
            let delay = match self.tick().await {
                Ok(delay) => delay,
                Err(error) => {
                    error!("media relocation scheduler tick failed: {error}");
                    60
                }
            };
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(delay.max(5))) => {}
                _ = self.scan_requested.notified() => {}
            }
        }
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
        self.scan_requested.notify_waiters();
    }

    pub fn request_scan(&self) {
        if self.enabled_by_environment {
            self.scan_requested.notify_one();
        }
    }

    async fn tick(&self) -> Result<u64, String> {
        let config = self
            .db
            .get_openlist_config()
            .await
            .map_err(|e| e.to_string())?;
        let inserted = self
            .db
            .enqueue_submitted_media_relocation_jobs()
            .await
            .map_err(|e| e.to_string())?;
        if inserted > 0 {
            info!("enqueued {inserted} media relocation job(s)");
        }
        if !config.enabled || config.base_url.trim().is_empty() || config.api_key.trim().is_empty()
        {
            return Ok(config.scan_interval_secs);
        }
        // A job has several idempotent local transitions after an OpenList copy finishes.
        // Drain those transitions now instead of making each one wait a full scan interval.
        for _ in 0..8 {
            let jobs = self
                .db
                .claim_due_media_relocation_jobs(&self.owner, 120, 10)
                .await
                .map_err(|e| e.to_string())?;
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                if let Err(error) = self.process_one(&config, job.clone()).await {
                    warn!(
                        "media relocation job {} failed at {}: {}",
                        job.id, job.stage, error
                    );
                    self.record_retry(job, error).await;
                }
            }
        }
        Ok(config.scan_interval_secs)
    }

    async fn process_one(
        &self,
        config: &OpenListConfig,
        mut job: MediaRelocationJob,
    ) -> Result<(), String> {
        let expected_version = job.version;
        let expected_stage = job.stage.clone();
        let downloader_id = job.downloader_id.ok_or("迁移任务缺少下载器")?;
        let downloader = self
            .db
            .get_downloader(downloader_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| format!("下载器 {downloader_id} 不存在"))?;
        let source_qb = self.pool.get(&downloader).await?;
        let target_qb = if expected_stage == "waiting_download" {
            None
        } else {
            let target_downloader_id = job
                .target_downloader_id
                .or_else(|| {
                    config.target_directories.iter().find_map(|target| {
                        normalize_path(&target.openlist_path)
                            .is_ok_and(|root| {
                                is_path_prefix(
                                    &root,
                                    &normalize_path(&job.target_openlist_path).unwrap_or_default(),
                                )
                            })
                            .then_some(target.downloader_id)
                    })
                })
                .ok_or("迁移任务的目标下载器快照缺失")?;
            let record = self
                .db
                .get_downloader(target_downloader_id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("目标下载器 {target_downloader_id} 不存在"))?;
            Some(self.pool.get(&record).await?)
        };
        let openlist = OpenListClient::new(&config.base_url, &config.api_key)?;
        let download = self
            .db
            .get_media_download(job.media_download_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or("追剧下载记录不存在")?;
        let tmdb_is_animation = match download.subscription_id {
            Some(subscription_id) => self
                .db
                .get_subscription(subscription_id)
                .await
                .map_err(|e| e.to_string())?
                .is_some_and(|subscription| subscription.tmdb_is_animation),
            None => false,
        };

        match expected_stage.as_str() {
            "waiting_download" => {
                if !valid_infohash(&job.infohash) {
                    job.stage = "cancelled".to_string();
                    job.last_error = Some("历史记录缺少有效 infohash，已跳过归档".to_string());
                    let updated = self
                        .db
                        .update_media_relocation_job(&job, expected_version, &expected_stage)
                        .await
                        .map_err(|e| e.to_string())?;
                    if !updated {
                        return Err("迁移任务状态已被其他 worker 修改".to_string());
                    }
                    return Ok(());
                }
                let torrents = source_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                let Some(torrent) = torrents.into_iter().next() else {
                    job.stage = "cancelled".to_string();
                    job.last_error = Some("qB 中已不存在该种子，已跳过历史归档".to_string());
                    let updated = self
                        .db
                        .update_media_relocation_job(&job, expected_version, &expected_stage)
                        .await
                        .map_err(|e| e.to_string())?;
                    if !updated {
                        return Err("迁移任务状态已被其他 worker 修改".to_string());
                    }
                    return Ok(());
                };
                let torrent = find_completed_torrent(vec![torrent])?;
                let content_path = torrent_content_root(&torrent);
                let save_qb_path = normalize_path(&torrent.save_path)?;
                let mapping = config
                    .path_mappings
                    .iter()
                    .filter(|mapping| mapping.downloader_id == downloader_id)
                    .filter(|mapping| {
                        normalize_path(&mapping.qb_path)
                            .is_ok_and(|root| is_path_prefix(&root, &save_qb_path))
                    })
                    .max_by_key(|mapping| mapping.qb_path.len())
                    .ok_or_else(|| format!("种子保存路径 {save_qb_path} 没有 OpenList 映射"))?;
                let target = config
                    .target_directories
                    .iter()
                    .find(|target| target.id == config.target_directory_id)
                    .ok_or("未选择 OpenList 目标目录")?;
                let media_type = media_download_category(&download.target_key, tmdb_is_animation);
                let year = serde_json::from_str::<serde_json::Value>(&download.release_json)
                    .ok()
                    .and_then(|value| value.get("year").and_then(|year| year.as_u64()))
                    .map(|year| year as u32);
                let relative_dir = category_year_directory(media_type, year);
                let content_qb_path = normalize_path(&content_path)?;
                let content_openlist_path =
                    translate_path(&content_qb_path, &mapping.qb_path, &mapping.openlist_path)?;
                let root_qb_path = normalize_optional_path(&torrent.root_path)?;
                let source_files = normalize_torrent_file_paths(
                    source_qb.get_torrent_files(&job.infohash).await?,
                )?;
                let copy_items = torrent_top_level_items(&source_files)?;
                job.source_qb_path = save_qb_path;
                job.source_openlist_path = translate_path(
                    &job.source_qb_path,
                    &mapping.qb_path,
                    &mapping.openlist_path,
                )?;
                job.source_content_openlist_path = content_openlist_path;
                job.source_files_json = serde_json::to_string(&source_files)
                    .map_err(|e| format!("序列化种子文件清单失败: {e}"))?;
                job.copy_items_json = serde_json::to_string(&copy_items)
                    .map_err(|e| format!("序列化种子顶级项目失败: {e}"))?;
                job.target_openlist_path = join_path(&target.openlist_path, &relative_dir)?;
                job.target_qb_path = join_path(&target.qb_path, &relative_dir)?;
                let content_suffix = relative_path(&content_qb_path, &job.source_qb_path)?;
                job.target_content_qb_path = if content_suffix.is_empty() {
                    job.target_qb_path.clone()
                } else {
                    join_path(&job.target_qb_path, &content_suffix)?
                };
                job.target_downloader_id = Some(target.downloader_id);
                job.target_root_folder = Some(root_qb_path.is_some());
                let mut tasks = Vec::new();
                for item in &copy_items {
                    tasks.extend(
                        openlist
                            .copy(&job.source_openlist_path, &job.target_openlist_path, item)
                            .await?,
                    );
                }
                job.openlist_task_id =
                    encode_openlist_task_ids(tasks.iter().map(|task| task.id.clone()));
                job.stage = if tasks.is_empty() {
                    "copy_succeeded"
                } else {
                    "copying"
                }
                .to_string();
            }
            "copying" => {
                let task_ids = decode_openlist_task_ids(
                    job.openlist_task_id.as_deref().ok_or("复制任务 ID 缺失")?,
                );
                if task_ids.is_empty() {
                    return Err("复制任务 ID 缺失".to_string());
                }
                let mut all_succeeded = true;
                for task_id in task_ids {
                    let task = openlist.task_info(&task_id).await?;
                    if task.terminal_failure() {
                        return Err(format!("OpenList 复制失败: {}", task.error));
                    }
                    if !task.succeeded() {
                        all_succeeded = false;
                    }
                }
                if all_succeeded {
                    job.stage = "copy_succeeded".to_string();
                } else {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                }
            }
            "copy_succeeded" => {
                let _source = find_completed_torrent(
                    source_qb
                        .list_torrents_by_hashes(&[job.infohash.clone()])
                        .await?,
                )?;
                let source_files = decode_string_list(&job.source_files_json, "种子文件清单")?;
                let copy_items = decode_string_list(&job.copy_items_json, "种子顶级项目")?;
                let mut repair_required = false;
                for file in &source_files {
                    let source_path = join_path(&job.source_openlist_path, file)?;
                    let copied_path = join_path(&job.target_openlist_path, file)?;
                    let source_object = openlist.stat(&source_path).await?;
                    if source_object.is_dir {
                        return Err(format!("qB 文件清单项目不是文件: {file}"));
                    }
                    match openlist.stat_if_exists(&copied_path).await? {
                        Some(copied) if !copied.is_dir && copied.size == source_object.size => {}
                        Some(copied) if copied.is_dir => {
                            return Err(format!(
                                "目标路径应为文件但实际是目录，已拒绝自动覆盖: {copied_path}"
                            ));
                        }
                        Some(_) => {
                            let (target_dir, target_name) = parent_and_name(&copied_path)?;
                            openlist.remove_if_exists(&target_dir, &target_name).await?;
                            repair_required = true;
                        }
                        None => repair_required = true,
                    }
                }
                if repair_required {
                    let mut tasks = Vec::new();
                    for item in &copy_items {
                        tasks.extend(
                            openlist
                                .copy(&job.source_openlist_path, &job.target_openlist_path, item)
                                .await?,
                        );
                    }
                    job.openlist_task_id =
                        encode_openlist_task_ids(tasks.iter().map(|task| task.id.clone()));
                    job.stage = if tasks.is_empty() {
                        "copy_succeeded"
                    } else {
                        "copying"
                    }
                    .to_string();
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                } else {
                    job.torrent_data = Some(source_qb.export_torrent(&job.infohash).await?);
                    job.stage = "torrent_exported".to_string();
                }
            }
            "torrent_exported" => {
                let existing = source_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                if !existing.is_empty() {
                    source_qb.pause_torrent(&job.infohash).await?;
                    source_qb.delete_torrent(&job.infohash, false).await?;
                }
                job.stage = "source_qb_removed".to_string();
            }
            "source_qb_removed" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let existing = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                if let Some(torrent) = existing.first() {
                    if validate_target_torrent_location(torrent, &job).is_ok() {
                        job.stage = "target_qb_submitted".to_string();
                    } else {
                        // qB removes torrents asynchronously. When source and target are the
                        // same qB, wait until the old-path instance has actually disappeared.
                        job.next_attempt_at =
                            Some((Utc::now() + ChronoDuration::seconds(5)).to_rfc3339());
                    }
                } else {
                    let torrent = job.torrent_data.clone().ok_or("导出的种子数据缺失")?;
                    target_qb
                        .add_torrent(
                            torrent,
                            &format!("{}.torrent", job.infohash),
                            &AddTorrentOptions {
                                save_path: Some(job.target_qb_path.clone()),
                                tags: Some("云母".to_string()),
                                category: Some(
                                    media_download_category(
                                        &download.target_key,
                                        tmdb_is_animation,
                                    )
                                    .to_string(),
                                ),
                                paused: true,
                                skip_checking: true,
                                root_folder: job.target_root_folder,
                                ..Default::default()
                            },
                        )
                        .await?;
                    job.stage = "target_qb_submitted".to_string();
                }
            }
            "target_qb_submitted" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                if let Some(torrent) = torrents.first() {
                    validate_target_torrent_location(torrent, &job)?;
                    validate_target_torrent_manifest(target_qb.as_ref(), &job).await?;
                    target_qb.start_torrent(&job.infohash).await?;
                    job.stage = "target_qb_starting".to_string();
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                } else {
                    // The add request may have been lost or qB may have removed the old task
                    // after the previous scan. Return to the idempotent submission stage.
                    job.stage = "source_qb_removed".to_string();
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(5)).to_rfc3339());
                }
            }
            "target_qb_starting" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                let torrents = target_qb
                    .list_torrents_by_hashes(&[job.infohash.clone()])
                    .await?;
                let torrent = torrents.first().ok_or("目标 qB 中找不到重新导入的种子")?;
                validate_target_torrent_location(torrent, &job)?;
                validate_target_torrent_manifest(target_qb.as_ref(), &job).await?;
                if !target_torrent_is_seeding(&torrent.state) {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(10)).to_rfc3339());
                } else {
                    let source_files = decode_string_list(&job.source_files_json, "种子文件清单")?;
                    if source_files.is_empty() {
                        return Err("种子文件清单为空，已拒绝自动删除源文件".to_string());
                    }
                    let referenced_by_other_torrents =
                        source_paths_referenced_by_other_torrents(source_qb.as_ref(), &job).await?;
                    for file in &source_files {
                        let path = join_path(&job.source_openlist_path, file)?;
                        if referenced_by_other_torrents.contains(&path) {
                            warn!(
                                "media relocation job {} retained source file referenced by another torrent: {}",
                                job.id, path
                            );
                            continue;
                        }
                        let (source_dir, source_name) = parent_and_name(&path)?;
                        openlist.remove_if_exists(&source_dir, &source_name).await?;
                    }
                    for directory in manifest_parent_directories(&source_files) {
                        let path = join_path(&job.source_openlist_path, &directory)?;
                        openlist.remove_empty_directory_if_exists(&path).await?;
                    }
                    job.stage = "source_removed".to_string();
                }
            }
            "source_removed" => {
                let target_qb = target_qb.as_ref().ok_or("迁移任务缺少目标下载器")?;
                // Keep this call for jobs created by older versions that reached this stage
                // before the target torrent was started.
                target_qb.start_torrent(&job.infohash).await?;
                job.stage = "completed".to_string();
                job.completed_at = Some(Utc::now().to_rfc3339());
                job.torrent_data = None;
            }
            "completed" | "cancelled" => return Ok(()),
            stage => return Err(format!("未知迁移阶段: {stage}")),
        }
        job.last_error = None;
        let updated = self
            .db
            .update_media_relocation_job(&job, expected_version, &expected_stage)
            .await
            .map_err(|e| e.to_string())?;
        if !updated {
            return Err("迁移任务状态已被其他 worker 修改".to_string());
        }
        Ok(())
    }

    async fn record_retry(&self, mut job: MediaRelocationJob, error: String) {
        let expected_version = job.version;
        let expected_stage = job.stage.clone();
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = Some(error);
        let backoff = 30_i64.saturating_mul(1_i64 << job.attempts.min(6));
        job.next_attempt_at = Some((Utc::now() + ChronoDuration::seconds(backoff)).to_rfc3339());
        let _ = self
            .db
            .update_media_relocation_job(&job, expected_version, &expected_stage)
            .await;
    }
}

fn find_completed_torrent(mut torrents: Vec<TorrentInfo>) -> Result<TorrentInfo, String> {
    let torrent = torrents.pop().ok_or("qB 中找不到对应种子")?;
    if !torrent_is_complete(
        torrent.completion_on,
        torrent.downloaded,
        torrent.size,
        &torrent.state,
    ) {
        return Err("种子尚未下载完成".to_string());
    }
    Ok(torrent)
}

fn torrent_is_complete(completion_on: i64, downloaded: i64, size: i64, state: &str) -> bool {
    let state = state.to_ascii_lowercase();
    completion_on > 0
        || (size > 0 && downloaded >= size)
        || matches!(
            state.as_str(),
            "uploading"
                | "stalledup"
                | "queuedup"
                | "forcedup"
                | "pausedup"
                | "stoppedup"
                | "checkingup"
        )
}

fn target_torrent_is_seeding(state: &str) -> bool {
    matches!(
        state.to_ascii_lowercase().as_str(),
        "uploading" | "stalledup" | "forcedup"
    )
}

fn validate_target_torrent_location(
    torrent: &TorrentInfo,
    job: &MediaRelocationJob,
) -> Result<(), String> {
    if job.target_content_qb_path.is_empty() {
        return Err("旧归档任务缺少目标内容路径快照，已拒绝继续自动清理".to_string());
    }
    let actual_save = normalize_path(&torrent.save_path)?;
    let expected_save = normalize_path(&job.target_qb_path)?;
    if actual_save != expected_save {
        return Err(format!(
            "目标 qB 保存路径不匹配: {actual_save} != {expected_save}"
        ));
    }
    let actual_content = normalize_path(&torrent.content_path)?;
    let expected_content = normalize_path(&job.target_content_qb_path)?;
    if actual_content != expected_content {
        return Err(format!(
            "目标 qB 内容路径不匹配: {actual_content} != {expected_content}"
        ));
    }
    Ok(())
}

async fn validate_target_torrent_manifest(
    target_qb: &dyn DownloaderClient,
    job: &MediaRelocationJob,
) -> Result<(), String> {
    let expected = decode_string_list(&job.source_files_json, "源种子文件清单")?;
    let actual = normalize_torrent_file_paths(target_qb.get_torrent_files(&job.infohash).await?)?;
    if actual != expected {
        return Err("目标 qB 的种子文件结构与源种子不一致，已拒绝删除源文件".to_string());
    }
    Ok(())
}

async fn source_paths_referenced_by_other_torrents(
    source_qb: &dyn DownloaderClient,
    job: &MediaRelocationJob,
) -> Result<std::collections::BTreeSet<String>, String> {
    let source_files = decode_string_list(&job.source_files_json, "源种子文件清单")?;
    let source_paths = source_files
        .iter()
        .map(|file| join_path(&job.source_qb_path, file))
        .collect::<Result<std::collections::BTreeSet<_>, _>>()?;
    let mut referenced = std::collections::BTreeSet::new();
    for torrent in source_qb.list_torrents(None).await? {
        if torrent.hash.eq_ignore_ascii_case(&job.infohash) {
            continue;
        }
        let save_path = normalize_path(&torrent.save_path)?;
        if !source_paths
            .iter()
            .any(|source_path| is_path_prefix(&save_path, source_path))
        {
            continue;
        }
        for file in normalize_torrent_file_paths(source_qb.get_torrent_files(&torrent.hash).await?)?
        {
            let path = join_path(&save_path, &file)?;
            if source_paths.contains(&path) {
                let openlist_path =
                    translate_path(&path, &job.source_qb_path, &job.source_openlist_path)?;
                referenced.insert(openlist_path);
            }
        }
    }
    Ok(referenced)
}

fn torrent_content_root(torrent: &TorrentInfo) -> String {
    select_torrent_content_root(&torrent.content_path, &torrent.save_path)
}

fn select_torrent_content_root(content_path: &str, save_path: &str) -> String {
    [content_path, save_path]
        .into_iter()
        .find(|path| !path.trim().is_empty())
        .map(str::to_string)
        .unwrap_or_default()
}

fn normalize_optional_path(path: &str) -> Result<Option<String>, String> {
    if path.trim().is_empty() {
        Ok(None)
    } else {
        normalize_path(path).map(Some)
    }
}

fn relative_path(path: &str, root: &str) -> Result<String, String> {
    let path = normalize_path(path)?;
    let root = normalize_path(root)?;
    if !is_path_prefix(&root, &path) {
        return Err(format!("路径 {path} 不在根目录 {root} 下"));
    }
    Ok(if path == root {
        String::new()
    } else if root == "/" {
        path.trim_start_matches('/').to_string()
    } else {
        path[root.len()..].trim_start_matches('/').to_string()
    })
}

fn normalize_torrent_file_paths(files: Vec<TorrentFileInfo>) -> Result<Vec<String>, String> {
    let mut normalized = std::collections::BTreeMap::new();
    for file in files {
        let path = normalize_manifest_path(&file.path)?;
        if let Some(previous_size) = normalized.insert(path.clone(), file.size) {
            if previous_size != file.size {
                return Err(format!("种子文件清单包含冲突路径: {path}"));
            }
        }
    }
    if normalized.is_empty() {
        return Err("种子文件清单为空".to_string());
    }
    Ok(normalized.into_keys().collect())
}

fn normalize_manifest_path(path: &str) -> Result<String, String> {
    let path = path.replace('\\', "/");
    if path.is_empty()
        || path.starts_with('/')
        || path.as_bytes().get(1) == Some(&b':')
        || path.contains('\0')
    {
        return Err(format!("种子文件路径无效: {path:?}"));
    }
    let mut parts = Vec::new();
    for part in path.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!("种子文件路径无效: {path:?}"));
        }
        parts.push(part);
    }
    Ok(parts.join("/"))
}

fn torrent_top_level_items(files: &[String]) -> Result<Vec<String>, String> {
    let items = files
        .iter()
        .map(|path| path.split('/').next().unwrap_or_default().to_string())
        .collect::<std::collections::BTreeSet<_>>();
    if items.is_empty() || items.contains("") {
        return Err("种子顶级文件/文件夹列表为空".to_string());
    }
    Ok(items.into_iter().collect())
}

fn decode_string_list(value: &str, label: &str) -> Result<Vec<String>, String> {
    let values =
        serde_json::from_str::<Vec<String>>(value).map_err(|e| format!("解析{label}失败: {e}"))?;
    values
        .into_iter()
        .map(|value| normalize_manifest_path(&value))
        .collect()
}

fn manifest_parent_directories(files: &[String]) -> Vec<String> {
    let mut directories = std::collections::BTreeSet::new();
    for file in files {
        let mut parts = file.split('/').collect::<Vec<_>>();
        parts.pop();
        while !parts.is_empty() {
            directories.insert(parts.join("/"));
            parts.pop();
        }
    }
    let mut directories = directories.into_iter().collect::<Vec<_>>();
    directories.sort_by(|left, right| {
        right
            .matches('/')
            .count()
            .cmp(&left.matches('/').count())
            .then_with(|| right.cmp(left))
    });
    directories
}

fn valid_infohash(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
