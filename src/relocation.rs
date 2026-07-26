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

#[allow(dead_code)]
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

async fn any_openlist_task_active(
    openlist: &OpenListClient,
    job: &MediaRelocationJob,
) -> Result<bool, String> {
    let task_ids = job
        .openlist_task_id
        .as_deref()
        .map(decode_openlist_task_ids)
        .unwrap_or_default();
    for task_id in task_ids {
        if let Some(task) = openlist.task_info_if_exists(&task_id).await?
            && !task.succeeded()
            && !task.terminal_failure()
        {
            return Ok(true);
        }
    }
    Ok(false)
}

#[allow(dead_code)]
pub fn category_directory(media_type: &str, year: Option<u32>) -> String {
    match media_type.trim().to_ascii_lowercase().as_str() {
        "movie" | "电影" => "电影".to_string(),
        "anime" | "动漫" => "动漫".to_string(),
        "concert" | "演唱会" => "演唱会".to_string(),
        "year" | "年份" => year
            .map(|v| v.to_string())
            .unwrap_or_else(|| "年份".to_string()),
        "tv" | "电视" | "电视剧" => "电视剧".to_string(),
        _ => "电视剧".to_string(),
    }
}

#[allow(dead_code)]
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
    fn relocation_rejects_identical_source_and_target_paths() {
        assert!(validate_distinct_path_pairs("/src", "/src", "/qb-a", "/qb-b").is_err());
        assert!(validate_distinct_path_pairs("/src", "/dst", "/qb", "/qb").is_err());
        assert!(validate_distinct_path_pairs("/src", "/dst", "/qb-a", "/qb-b").is_ok());
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
    fn tmdb_archive_uses_primary_genre_and_tmdb_year() {
        assert_eq!(
            archive_relative_directory("电视剧", "动画", Some(2025)),
            "云母/电视剧/动画/2025"
        );
        assert_eq!(
            archive_relative_directory("电影", "剧情", None),
            "云母/电影/剧情/年份未知"
        );
    }

    #[test]
    fn scheduler_honors_copy_settle_deadline() {
        let before = Utc::now();
        let deadline = before + ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS);
        assert_eq!(
            relocation_scheduler_delay(
                600,
                Some(&deadline.to_rfc3339()),
                deadline - ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS),
            ),
            COPY_SETTLE_DELAY_SECONDS as u64
        );
        assert_eq!(relocation_scheduler_delay(600, None, before), 600);
    }

    #[test]
    fn target_qb_false_success_becomes_a_recordable_failure() {
        let infohash = "eadb91a4769b1fad89e0dd3a930523e7fc5814b8";
        let false_success = resolve_target_qb_submission(infohash, None, Ok(false)).unwrap_err();
        assert!(false_success.contains("添加接口返回成功"));
        assert!(false_success.contains(infohash));

        let explicit_failure = resolve_target_qb_submission(
            infohash,
            Some("connection refused".to_string()),
            Ok(false),
        )
        .unwrap_err();
        assert!(explicit_failure.contains("connection refused"));

        assert!(
            resolve_target_qb_submission(
                infohash,
                Some("duplicate response".to_string()),
                Ok(true),
            )
            .is_ok()
        );
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

    #[test]
    fn source_manifest_snapshot_preserves_path_and_size() {
        let manifest = normalize_torrent_manifest(vec![
            TorrentFileInfo {
                path: "Show/E02.mkv".to_string(),
                size: 20,
            },
            TorrentFileInfo {
                path: "Show/E01.mkv".to_string(),
                size: 10,
            },
        ])
        .unwrap();
        assert_eq!(
            manifest,
            vec![
                ManifestFile {
                    path: "Show/E01.mkv".to_string(),
                    size: 10,
                },
                ManifestFile {
                    path: "Show/E02.mkv".to_string(),
                    size: 20,
                },
            ]
        );
        let json = serde_json::to_string(&manifest).unwrap();
        assert_eq!(decode_source_manifest(&json).unwrap(), Some(manifest));
        assert_eq!(decode_source_manifest("[]").unwrap(), None);
    }

    #[test]
    fn ambiguous_submission_never_becomes_prepared_automatically() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            phase: CopyCheckpointPhase::Uncertain,
            submitted_at: Some((Utc::now() - ChronoDuration::hours(1)).to_rfc3339()),
        };
        assert_eq!(
            uncertain_submission_next_check(&checkpoint, Utc::now()).unwrap(),
            None
        );
        assert_eq!(
            decode_copy_checkpoint(&encode_copy_checkpoint(&checkpoint).unwrap())
                .unwrap()
                .phase,
            CopyCheckpointPhase::Uncertain
        );
    }

    #[test]
    fn copy_checkpoint_must_match_authoritative_manifest() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 11,
            phase: CopyCheckpointPhase::Prepared,
            submitted_at: None,
        };
        assert!(
            validate_checkpoint_against_manifest(
                &checkpoint,
                "[{\"path\":\"Show/E01.mkv\",\"size\":10}]"
            )
            .is_err()
        );
    }

    #[test]
    fn mixed_copy_tasks_preserve_in_flight_or_uncertain_work() {
        assert_eq!(
            decide_copy_tasks(&[
                CopyTaskObservation::Failed,
                CopyTaskObservation::Pending,
                CopyTaskObservation::Missing,
            ])
            .unwrap(),
            CopyTaskDecision::Wait
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Failed, CopyTaskObservation::Missing,])
                .unwrap(),
            CopyTaskDecision::VerifyTarget
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Failed, CopyTaskObservation::Failed,])
                .unwrap(),
            CopyTaskDecision::AllFailed
        );
        assert_eq!(
            decide_copy_tasks(&[CopyTaskObservation::Succeeded, CopyTaskObservation::Failed,])
                .unwrap(),
            CopyTaskDecision::VerifyTarget
        );

        let mut new_checkpoint = Some(
            encode_copy_checkpoint(&CopyCheckpoint {
                path: "Show/E01.mkv".to_string(),
                size: 10,
                phase: CopyCheckpointPhase::Prepared,
                submitted_at: None,
            })
            .unwrap(),
        );
        assert_eq!(
            prepare_read_only_copy_reconcile(&mut new_checkpoint, Utc::now()).unwrap(),
            "copy_submitting"
        );
        assert_eq!(
            decode_copy_checkpoint(new_checkpoint.as_deref().unwrap())
                .unwrap()
                .phase,
            CopyCheckpointPhase::Uncertain
        );
        let mut legacy_checkpoint = None;
        assert_eq!(
            prepare_read_only_copy_reconcile(&mut legacy_checkpoint, Utc::now()).unwrap(),
            "copy_legacy_reconcile"
        );
        let old_pending = encode_copy_checkpoint(&CopyCheckpoint {
            path: "Show/E01.mkv".to_string(),
            size: 10,
            phase: CopyCheckpointPhase::Uncertain,
            submitted_at: Some(
                (Utc::now() - ChronoDuration::seconds(COPY_TASK_PENDING_MANUAL_SECONDS + 1))
                    .to_rfc3339(),
            ),
        })
        .unwrap();
        assert!(checkpoint_pending_timed_out(Some(&old_pending), Utc::now()).unwrap());
        assert!(!checkpoint_pending_timed_out(None, Utc::now()).unwrap());
    }

    #[test]
    fn manifest_cursor_batches_without_skipping_final_verification_boundary() {
        assert_eq!(manifest_scan_end(0, 250).unwrap(), MANIFEST_FILES_PER_PASS);
        assert_eq!(
            manifest_scan_end(MANIFEST_FILES_PER_PASS, 250).unwrap(),
            MANIFEST_FILES_PER_PASS * 2
        );
        assert_eq!(manifest_scan_end(200, 250).unwrap(), 250);
        assert_eq!(manifest_scan_end(250, 250).unwrap(), 250);
        assert!(manifest_scan_end(251, 250).is_err());
    }

    #[test]
    fn old_advanced_jobs_require_sized_manifest_recovery() {
        for stage in [
            "torrent_exported",
            "source_qb_removed",
            "target_qb_submitted",
            "target_qb_starting",
        ] {
            assert!(advanced_stage_requires_manifest(stage));
        }
        assert!(!advanced_stage_requires_manifest("source_removed"));
        assert!(
            validate_manifest_paths_snapshot(
                &[ManifestFile {
                    path: "Show/E01.mkv".to_string(),
                    size: 10,
                }],
                "[\"Show/E02.mkv\"]",
            )
            .is_err()
        );
        assert_eq!(
            manifest_from_torrent_data(b"d4:infod6:lengthi12e4:name8:file.mkvee").unwrap(),
            vec![ManifestFile {
                path: "file.mkv".to_string(),
                size: 12,
            }]
        );
    }

    #[test]
    fn checkpoint_returns_its_authoritative_manifest_cursor() {
        let checkpoint = CopyCheckpoint {
            path: "Show/E02.mkv".to_string(),
            size: 20,
            phase: CopyCheckpointPhase::Prepared,
            submitted_at: None,
        };
        assert_eq!(
            validate_checkpoint_against_manifest(
                &checkpoint,
                "[{\"path\":\"Show/E01.mkv\",\"size\":10},{\"path\":\"Show/E02.mkv\",\"size\":20}]",
            )
            .unwrap(),
            1
        );
    }
}
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use chrono::{Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::Notify;
use tracing::{error, info, warn};

use crate::db::{Database, MediaRelocationJob, OpenListConfig};
use crate::downloader::{
    AddTorrentOptions, DownloaderClient, DownloaderClientPool, TorrentFileInfo, TorrentInfo,
};
use crate::media::models::media_download_category;
use crate::media::torrent::torrent_file_manifest;
use crate::openlist::{ManifestFileState, OpenListClient};

const COPY_SETTLE_DELAY_SECONDS: i64 = 30;
const COPY_SUBMISSION_ATTENTION_SECONDS: i64 = 300;
const COPY_TASK_PENDING_MANUAL_SECONDS: i64 = 24 * 60 * 60;
const MANIFEST_FILES_PER_PASS: usize = 100;
const RELOCATION_LEASE_SECONDS: i64 = 120;
const RELOCATION_LEASE_HEARTBEAT_SECONDS: u64 = 30;
const TARGET_QB_CONFIRM_ATTEMPTS: usize = 5;
const TARGET_QB_CONFIRM_INTERVAL: Duration = Duration::from_millis(300);

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct ManifestFile {
    path: String,
    size: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum CopyCheckpointPhase {
    Prepared,
    Uncertain,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
struct CopyCheckpoint {
    path: String,
    size: i64,
    phase: CopyCheckpointPhase,
    #[serde(default)]
    submitted_at: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyTaskObservation {
    Pending,
    Succeeded,
    Failed,
    Missing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CopyTaskDecision {
    Wait,
    AllFailed,
    VerifyTarget,
}

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
        for _ in 0..32 {
            let jobs = self
                .db
                .claim_due_media_relocation_jobs(&self.owner, RELOCATION_LEASE_SECONDS, 1)
                .await
                .map_err(|e| e.to_string())?;
            if jobs.is_empty() {
                break;
            }
            for job in jobs {
                if let Err(error) = self.process_one_with_lease(&config, job.clone()).await {
                    warn!(
                        "media relocation job {} failed at {}: {}",
                        job.id, job.stage, error
                    );
                    self.record_retry(job, error).await;
                }
            }
        }
        let next_attempt_at = self
            .db
            .next_media_relocation_attempt_at()
            .await
            .map_err(|e| e.to_string())?;
        Ok(relocation_scheduler_delay(
            config.scan_interval_secs,
            next_attempt_at.as_deref(),
            Utc::now(),
        ))
    }

    async fn process_one_with_lease(
        &self,
        config: &OpenListConfig,
        job: MediaRelocationJob,
    ) -> Result<(), String> {
        let lease_owner = job.lease_owner.clone().ok_or("迁移任务 lease owner 缺失")?;
        let process = self.process_one(config, job.clone());
        tokio::pin!(process);
        loop {
            tokio::select! {
                result = &mut process => return result,
                _ = tokio::time::sleep(Duration::from_secs(RELOCATION_LEASE_HEARTBEAT_SECONDS)) => {
                    let renewed = self.db
                        .renew_media_relocation_lease(
                            job.id,
                            job.version,
                            &job.stage,
                            &lease_owner,
                            RELOCATION_LEASE_SECONDS,
                        )
                        .await
                        .map_err(|error| error.to_string())?;
                    if !renewed {
                        return Err("迁移任务 lease 已变化，已停止当前处理".to_string());
                    }
                }
            }
        }
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
        if expected_stage != "waiting_download" {
            validate_distinct_relocation_paths(&job)?;
        }
        if advanced_stage_requires_manifest(&expected_stage)
            && decode_source_manifest(&job.source_manifest_json)?.is_none()
        {
            let torrents = source_qb
                .list_torrents_by_hashes(&[job.infohash.clone()])
                .await?;
            let recovered = if torrents.is_empty() {
                job.torrent_data
                    .as_deref()
                    .ok_or_else(|| "qB 中已无种子且导出的 torrent_data 缺失".to_string())
                    .and_then(manifest_from_torrent_data)
            } else {
                normalize_torrent_manifest(source_qb.get_torrent_files(&job.infohash).await?)
            };
            match recovered.and_then(|manifest| {
                validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
                Ok(manifest)
            }) {
                Ok(manifest) => {
                    job.source_files_json = encode_manifest_paths(&manifest)?;
                    job.source_manifest_json = serde_json::to_string(&manifest)
                        .map_err(|error| format!("序列化恢复的种子文件大小快照失败: {error}"))?;
                    job.manifest_cursor = manifest.len();
                    job.next_attempt_at = Some(Utc::now().to_rfc3339());
                    job.last_error = None;
                }
                Err(recovery_error) => {
                    job.stage = "manifest_required".to_string();
                    job.copy_lock_acquired = false;
                    job.next_attempt_at = None;
                    job.last_error = Some(format!(
                        "旧迁移任务在阶段 {expected_stage} 无法恢复带大小的权威 manifest，已停止自动清理: {recovery_error}"
                    ));
                }
            }
            let updated = self
                .db
                .update_media_relocation_job(&job, expected_version, &expected_stage)
                .await
                .map_err(|error| error.to_string())?;
            if !updated {
                return Err("迁移任务状态已被其他 worker 修改".to_string());
            }
            return Ok(());
        }
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
        let subscription = match download.subscription_id {
            Some(subscription_id) => self
                .db
                .get_subscription(subscription_id)
                .await
                .map_err(|e| e.to_string())?,
            None => None,
        };
        let tmdb_is_animation = subscription
            .as_ref()
            .is_some_and(|item| item.tmdb_is_animation);
        job.next_attempt_at = None;

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
                let primary_type = if download.target_key.starts_with("movie:") {
                    "电影"
                } else {
                    "电视剧"
                };
                let tmdb_year = subscription.as_ref().and_then(|item| item.year);
                let primary_genre = subscription
                    .as_ref()
                    .and_then(|item| item.tmdb_genres.first())
                    .map(|genre| genre.name.as_str())
                    .unwrap_or("其他");
                let relative_dir =
                    archive_relative_directory(primary_type, primary_genre, tmdb_year);
                let content_qb_path = normalize_path(&content_path)?;
                let content_openlist_path =
                    translate_path(&content_qb_path, &mapping.qb_path, &mapping.openlist_path)?;
                let root_qb_path = normalize_optional_path(&torrent.root_path)?;
                let source_manifest =
                    normalize_torrent_manifest(source_qb.get_torrent_files(&job.infohash).await?)?;
                let source_files = source_manifest
                    .iter()
                    .map(|file| file.path.clone())
                    .collect::<Vec<_>>();
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
                job.source_manifest_json = serde_json::to_string(&source_manifest)
                    .map_err(|e| format!("序列化种子文件大小快照失败: {e}"))?;
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
                validate_distinct_relocation_paths(&job)?;
                job.openlist_task_id = None;
                job.copy_checkpoint_json = None;
                job.copy_lock_acquired = false;
                job.manifest_cursor = 0;
                job.stage = "copy_reconcile".to_string();
            }
            "copy_reconcile" | "copy_legacy_reconcile" => {
                if !self
                    .acquire_copy_lock(&mut job, expected_version, &expected_stage)
                    .await?
                {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                } else if let Some(manifest) = decode_source_manifest(&job.source_manifest_json)? {
                    let end = manifest_scan_end(job.manifest_cursor, manifest.len())?;
                    let mut missing = None;
                    for (index, file) in manifest
                        .iter()
                        .enumerate()
                        .take(end)
                        .skip(job.manifest_cursor)
                    {
                        match openlist
                            .inspect_manifest_file(
                                &job.source_openlist_path,
                                &job.target_openlist_path,
                                &file.path,
                                file.size,
                            )
                            .await?
                        {
                            ManifestFileState::Present => {}
                            ManifestFileState::Missing => {
                                missing = Some((index, file.clone()));
                                break;
                            }
                        }
                        job.manifest_cursor = index + 1;
                    }
                    if let Some((index, file)) = missing {
                        job.manifest_cursor = index;
                        if expected_stage == "copy_legacy_reconcile" {
                            job.stage = "copy_manual_review".to_string();
                            job.next_attempt_at = None;
                            job.last_error = Some(format!(
                                "旧复制任务目标缺少 manifest 文件，未自动重提，请人工核验: {}",
                                file.path
                            ));
                        } else {
                            job.copy_checkpoint_json =
                                Some(encode_copy_checkpoint(&CopyCheckpoint {
                                    path: file.path,
                                    size: file.size,
                                    phase: CopyCheckpointPhase::Prepared,
                                    submitted_at: None,
                                })?);
                            job.openlist_task_id = None;
                            job.stage = "copy_submitting".to_string();
                        }
                    } else if job.manifest_cursor < manifest.len() {
                        job.next_attempt_at = Some(Utc::now().to_rfc3339());
                    } else {
                        verify_openlist_manifest(&openlist, &job, &manifest, false).await?;
                        if expected_stage == "copy_legacy_reconcile"
                            && any_openlist_task_active(&openlist, &job).await?
                        {
                            job.stage = "copy_manual_review".to_string();
                            job.next_attempt_at = None;
                            job.last_error = Some(
                                "旧 OpenList 复制任务仍在运行；只读 manifest 已核验，但在任务终止前不会释放锁或继续迁移"
                                    .to_string(),
                            );
                        } else {
                            let _source = find_completed_torrent(
                                source_qb
                                    .list_torrents_by_hashes(&[job.infohash.clone()])
                                    .await?,
                            )?;
                            job.torrent_data = Some(source_qb.export_torrent(&job.infohash).await?);
                            job.copy_checkpoint_json = None;
                            job.openlist_task_id = None;
                            job.copy_lock_acquired = false;
                            job.stage = "torrent_exported".to_string();
                        }
                    }
                } else {
                    let manifest = normalize_torrent_manifest(
                        source_qb.get_torrent_files(&job.infohash).await?,
                    )?;
                    validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
                    job.source_files_json = encode_manifest_paths(&manifest)?;
                    job.source_manifest_json = serde_json::to_string(&manifest)
                        .map_err(|error| format!("序列化旧种子文件大小快照失败: {error}"))?;
                    job.manifest_cursor = 0;
                }
            }
            "copy_submitting" => {
                if !self
                    .acquire_copy_lock(&mut job, expected_version, &expected_stage)
                    .await?
                {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                } else {
                    let mut checkpoint = decode_copy_checkpoint(
                        job.copy_checkpoint_json
                            .as_deref()
                            .ok_or("复制 checkpoint 缺失")?,
                    )?;
                    let checkpoint_index = validate_checkpoint_against_manifest(
                        &checkpoint,
                        &job.source_manifest_json,
                    )?;
                    if checkpoint_index != job.manifest_cursor {
                        return Err(format!(
                            "复制 checkpoint 与 manifest cursor 不一致: {checkpoint_index} != {}",
                            job.manifest_cursor
                        ));
                    }
                    match openlist
                        .inspect_manifest_file(
                            &job.source_openlist_path,
                            &job.target_openlist_path,
                            &checkpoint.path,
                            checkpoint.size,
                        )
                        .await?
                    {
                        ManifestFileState::Present => {
                            job.manifest_cursor = checkpoint_index + 1;
                            job.copy_checkpoint_json = None;
                            job.openlist_task_id = None;
                            job.stage = "copy_reconcile".to_string();
                        }
                        ManifestFileState::Missing => match checkpoint.phase {
                            CopyCheckpointPhase::Prepared => {
                                checkpoint.phase = CopyCheckpointPhase::Uncertain;
                                checkpoint.submitted_at = Some(Utc::now().to_rfc3339());
                                let checkpoint_json = encode_copy_checkpoint(&checkpoint)?;
                                let owner = job
                                    .lease_owner
                                    .as_deref()
                                    .ok_or("迁移任务 lease owner 缺失")?;
                                let checkpointed = self
                                    .db
                                    .checkpoint_media_relocation_copy_submission(
                                        job.id,
                                        expected_version,
                                        &expected_stage,
                                        owner,
                                        &checkpoint_json,
                                    )
                                    .await
                                    .map_err(|error| error.to_string())?;
                                if !checkpointed {
                                    return Err(
                                        "复制提交前 checkpoint 写入失败，已拒绝调用 OpenList"
                                            .to_string(),
                                    );
                                }
                                job.copy_checkpoint_json = Some(checkpoint_json);
                                let tasks = openlist
                                    .copy_manifest_file(
                                        &job.source_openlist_path,
                                        &job.target_openlist_path,
                                        &checkpoint.path,
                                        checkpoint.size,
                                    )
                                    .await?;
                                job.openlist_task_id = encode_openlist_task_ids(
                                    tasks.iter().map(|task| task.id.clone()),
                                );
                                if tasks.is_empty() {
                                    job.next_attempt_at = Some(
                                        (Utc::now() + ChronoDuration::seconds(10)).to_rfc3339(),
                                    );
                                } else {
                                    job.stage = "copying".to_string();
                                    job.next_attempt_at = Some(
                                        (Utc::now() + ChronoDuration::seconds(30)).to_rfc3339(),
                                    );
                                }
                            }
                            CopyCheckpointPhase::Uncertain => {
                                let now = Utc::now();
                                match uncertain_submission_next_check(&checkpoint, now)? {
                                    Some(next_check) => {
                                        job.next_attempt_at = Some(next_check.to_rfc3339());
                                    }
                                    None => {
                                        job.stage = "copy_manual_review".to_string();
                                        job.next_attempt_at = None;
                                        job.last_error = Some(format!(
                                            "OpenList 复制提交结果不确定且目标文件仍未出现，已拒绝自动重复提交，请人工核验后调用 resolve-copy: {}",
                                            checkpoint.path
                                        ));
                                    }
                                }
                            }
                        },
                    }
                }
            }
            "copying" => {
                if !self
                    .acquire_copy_lock(&mut job, expected_version, &expected_stage)
                    .await?
                {
                    job.next_attempt_at =
                        Some((Utc::now() + ChronoDuration::seconds(30)).to_rfc3339());
                } else {
                    let task_ids = job
                        .openlist_task_id
                        .as_deref()
                        .map(decode_openlist_task_ids)
                        .unwrap_or_default();
                    if task_ids.is_empty() {
                        job.stage = "copy_manual_review".to_string();
                        job.next_attempt_at = None;
                        job.last_error = Some(
                            "复制任务 ID 缺失，无法证明原任务已终止；已拒绝自动重提".to_string(),
                        );
                    } else {
                        let mut observations = Vec::with_capacity(task_ids.len());
                        let mut failure_messages = Vec::new();
                        for task_id in task_ids {
                            match openlist.task_info_if_exists(&task_id).await? {
                                None => observations.push(CopyTaskObservation::Missing),
                                Some(task) if task.terminal_failure() => {
                                    observations.push(CopyTaskObservation::Failed);
                                    if !task.error.trim().is_empty() {
                                        failure_messages.push(task.error);
                                    }
                                }
                                Some(task) if task.succeeded() => {
                                    observations.push(CopyTaskObservation::Succeeded);
                                }
                                Some(_) => observations.push(CopyTaskObservation::Pending),
                            }
                        }
                        match decide_copy_tasks(&observations)? {
                            CopyTaskDecision::Wait => {
                                if checkpoint_pending_timed_out(
                                    job.copy_checkpoint_json.as_deref(),
                                    Utc::now(),
                                )? {
                                    job.stage = "copy_manual_review".to_string();
                                    job.next_attempt_at = None;
                                    job.last_error = Some(
                                        "OpenList 复制任务长时间未终止，已停止自动轮询并保留目标锁；请人工 recheck 或确认终止后 force_retry/cancel"
                                            .to_string(),
                                    );
                                } else {
                                    job.next_attempt_at = Some(
                                        (Utc::now() + ChronoDuration::seconds(30)).to_rfc3339(),
                                    );
                                }
                            }
                            CopyTaskDecision::AllFailed => {
                                if job.copy_checkpoint_json.is_some() {
                                    warn!(
                                        "all OpenList copy tasks for relocation job {} failed: {}",
                                        job.id,
                                        failure_messages.join("; ")
                                    );
                                    job.openlist_task_id = None;
                                    job.copy_checkpoint_json = None;
                                    job.stage = "copy_reconcile".to_string();
                                    job.next_attempt_at = Some(
                                        (Utc::now()
                                            + ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS))
                                        .to_rfc3339(),
                                    );
                                } else {
                                    job.stage = "copy_manual_review".to_string();
                                    job.next_attempt_at = None;
                                    job.last_error = Some(
                                        "旧复制任务全部失败，但缺少逐文件 checkpoint；已拒绝自动重提"
                                            .to_string(),
                                    );
                                }
                            }
                            CopyTaskDecision::VerifyTarget => {
                                job.stage = prepare_read_only_copy_reconcile(
                                    &mut job.copy_checkpoint_json,
                                    Utc::now(),
                                )?
                                .to_string();
                                job.next_attempt_at = Some(
                                    (Utc::now()
                                        + ChronoDuration::seconds(COPY_SETTLE_DELAY_SECONDS))
                                    .to_rfc3339(),
                                );
                            }
                        }
                    }
                }
            }
            "copy_succeeded" => {
                // Compatibility with jobs persisted by older versions. The authoritative
                // manifest reconciliation below is required before export and source removal.
                job.openlist_task_id = None;
                job.copy_checkpoint_json = None;
                job.manifest_cursor = 0;
                job.stage = "copy_legacy_reconcile".to_string();
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
                    let submission_error = target_qb
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
                        .await
                        .err();
                    let confirmation =
                        confirm_target_qb_torrent(target_qb.as_ref(), &job.infohash).await;
                    resolve_target_qb_submission(&job.infohash, submission_error, confirmation)?;
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
                    let manifest = decode_source_manifest(&job.source_manifest_json)?
                        .ok_or("带大小的权威种子 manifest 为空，已拒绝自动删除源文件")?;
                    validate_manifest_paths_snapshot(&manifest, &job.source_files_json)?;
                    let referenced_by_other_torrents =
                        source_paths_referenced_by_other_torrents(source_qb.as_ref(), &job).await?;
                    // This full source/target verification is intentionally adjacent to source
                    // deletion. Earlier cursor checks are only progress checkpoints.
                    verify_openlist_manifest(&openlist, &job, &manifest, true).await?;
                    for file in &manifest {
                        let path = join_path(&job.source_openlist_path, &file.path)?;
                        if referenced_by_other_torrents.contains(&path) {
                            warn!(
                                "media relocation job {} retained source file referenced by another torrent: {}",
                                job.id, path
                            );
                            continue;
                        }
                        if !verify_openlist_manifest_file(&openlist, &job, file, true).await? {
                            continue;
                        }
                        // OpenList has no conditional delete primitive. The removal helper
                        // re-stats the source once more to minimize, but cannot eliminate, the
                        // server-side stat-to-remove race.
                        openlist
                            .remove_manifest_file_if_exists(
                                &job.source_openlist_path,
                                &file.path,
                                file.size,
                            )
                            .await?;
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
        if !matches!(
            job.stage.as_str(),
            "copy_manual_review" | "manifest_required"
        ) {
            job.last_error = None;
        }
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

    async fn acquire_copy_lock(
        &self,
        job: &mut MediaRelocationJob,
        expected_version: i64,
        expected_stage: &str,
    ) -> Result<bool, String> {
        if job.copy_lock_acquired {
            return Ok(true);
        }
        let owner = job
            .lease_owner
            .as_deref()
            .ok_or("迁移任务 lease owner 缺失")?;
        let acquired = self
            .db
            .try_acquire_media_relocation_target_lock(
                job.id,
                expected_version,
                expected_stage,
                owner,
                &job.target_openlist_path,
            )
            .await
            .map_err(|error| error.to_string())?;
        if acquired {
            job.copy_lock_acquired = true;
        }
        Ok(acquired)
    }

    async fn record_retry(&self, mut job: MediaRelocationJob, error: String) {
        job.attempts = job.attempts.saturating_add(1);
        job.last_error = Some(error);
        let backoff = 30_i64.saturating_mul(1_i64 << job.attempts.min(6));
        job.next_attempt_at = Some((Utc::now() + ChronoDuration::seconds(backoff)).to_rfc3339());
        let Some(_) = job.lease_owner.as_deref() else {
            error!(
                "failed to record media relocation job {} error because lease owner is missing",
                job.id
            );
            return;
        };
        match self.db.record_media_relocation_retry(&job).await {
            Ok(true) => {}
            Ok(false) => error!(
                "failed to record media relocation job {} error because its state changed",
                job.id
            ),
            Err(record_error) => error!(
                "failed to record media relocation job {} error: {}",
                job.id, record_error
            ),
        }
    }
}

fn archive_relative_directory(
    primary_type: &str,
    primary_genre: &str,
    tmdb_year: Option<u32>,
) -> String {
    format!(
        "云母/{}/{}/{}",
        primary_type,
        primary_genre,
        tmdb_year
            .map(|year| year.to_string())
            .unwrap_or_else(|| "年份未知".to_string())
    )
}

fn relocation_scheduler_delay(
    configured_delay_secs: u64,
    next_attempt_at: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> u64 {
    let configured_delay_secs = configured_delay_secs.max(5);
    let Some(next_attempt_at) = next_attempt_at else {
        return configured_delay_secs;
    };
    let Ok(next_attempt_at) = chrono::DateTime::parse_from_rfc3339(next_attempt_at) else {
        return 5;
    };
    let remaining_ms = next_attempt_at
        .with_timezone(&Utc)
        .signed_duration_since(now)
        .num_milliseconds()
        .max(0) as u64;
    let remaining_secs = remaining_ms.saturating_add(999) / 1_000;
    configured_delay_secs.min(remaining_secs.max(5))
}

async fn confirm_target_qb_torrent(
    target_qb: &dyn DownloaderClient,
    infohash: &str,
) -> Result<bool, String> {
    let hashes = [infohash.to_string()];
    let mut last_result = Ok(false);
    for attempt in 0..TARGET_QB_CONFIRM_ATTEMPTS {
        match target_qb.list_torrents_by_hashes(&hashes).await {
            Ok(torrents)
                if torrents
                    .iter()
                    .any(|torrent| torrent.hash.eq_ignore_ascii_case(infohash)) =>
            {
                return Ok(true);
            }
            Ok(_) => last_result = Ok(false),
            Err(error) => last_result = Err(error),
        }
        if attempt + 1 < TARGET_QB_CONFIRM_ATTEMPTS {
            tokio::time::sleep(TARGET_QB_CONFIRM_INTERVAL).await;
        }
    }
    last_result
}

fn resolve_target_qb_submission(
    infohash: &str,
    submission_error: Option<String>,
    confirmation: Result<bool, String>,
) -> Result<(), String> {
    match confirmation {
        Ok(true) => Ok(()),
        Ok(false) => Err(submission_error.map_or_else(
            || format!("目标 qB 提交失败: 添加接口返回成功，但核验未发现种子 {infohash}"),
            |error| format!("目标 qB 提交失败: {error}; 核验未发现种子 {infohash}"),
        )),
        Err(confirmation_error) => Err(submission_error.map_or_else(
            || format!("目标 qB 提交结果核验失败: {confirmation_error}"),
            |error| format!("目标 qB 提交失败: {error}; 提交结果核验失败: {confirmation_error}"),
        )),
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
    let actual = normalize_torrent_manifest(target_qb.get_torrent_files(&job.infohash).await?)?;
    let expected = decode_source_manifest(&job.source_manifest_json)?
        .ok_or("目标 qB 校验缺少带大小的权威源 manifest")?;
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
    Ok(normalize_torrent_manifest(files)?
        .into_iter()
        .map(|file| file.path)
        .collect())
}

fn normalize_torrent_manifest(files: Vec<TorrentFileInfo>) -> Result<Vec<ManifestFile>, String> {
    let mut normalized = std::collections::BTreeMap::new();
    for file in files {
        let path = normalize_manifest_path(&file.path)?;
        if file.size < 0 {
            return Err(format!("种子文件大小无效: {path}={}", file.size));
        }
        if let Some(previous_size) = normalized.insert(path.clone(), file.size)
            && previous_size != file.size
        {
            return Err(format!("种子文件清单包含冲突路径: {path}"));
        }
    }
    if normalized.is_empty() {
        return Err("种子文件清单为空".to_string());
    }
    Ok(normalized
        .into_iter()
        .map(|(path, size)| ManifestFile { path, size })
        .collect())
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

fn decode_source_manifest(value: &str) -> Result<Option<Vec<ManifestFile>>, String> {
    let files = serde_json::from_str::<Vec<ManifestFile>>(value)
        .map_err(|error| format!("解析种子文件大小快照失败: {error}"))?;
    if files.is_empty() {
        return Ok(None);
    }
    let normalized = normalize_torrent_manifest(
        files
            .into_iter()
            .map(|file| TorrentFileInfo {
                path: file.path,
                size: file.size,
            })
            .collect(),
    )?;
    Ok(Some(normalized))
}

fn encode_manifest_paths(manifest: &[ManifestFile]) -> Result<String, String> {
    serde_json::to_string(
        &manifest
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>(),
    )
    .map_err(|error| format!("序列化种子文件路径快照失败: {error}"))
}

fn manifest_from_torrent_data(data: &[u8]) -> Result<Vec<ManifestFile>, String> {
    let files = torrent_file_manifest(data)
        .map_err(|error| format!("解析导出的 torrent_data manifest 失败: {error}"))?;
    normalize_torrent_manifest(
        files
            .into_iter()
            .map(|file| TorrentFileInfo {
                path: file.path,
                size: file.size,
            })
            .collect(),
    )
}

fn validate_manifest_paths_snapshot(
    manifest: &[ManifestFile],
    paths_json: &str,
) -> Result<(), String> {
    let paths = decode_string_list(paths_json, "种子文件路径快照")?;
    if paths.is_empty()
        || paths
            == manifest
                .iter()
                .map(|file| file.path.clone())
                .collect::<Vec<_>>()
    {
        Ok(())
    } else {
        Err("带大小的权威 manifest 与原种子文件路径快照不一致".to_string())
    }
}

async fn verify_openlist_manifest(
    openlist: &OpenListClient,
    job: &MediaRelocationJob,
    manifest: &[ManifestFile],
    allow_missing_source: bool,
) -> Result<(), String> {
    if manifest.is_empty() {
        return Err("带大小的权威 manifest 为空".to_string());
    }
    for file in manifest {
        verify_openlist_manifest_file(openlist, job, file, allow_missing_source).await?;
    }
    Ok(())
}

async fn verify_openlist_manifest_file(
    openlist: &OpenListClient,
    job: &MediaRelocationJob,
    file: &ManifestFile,
    allow_missing_source: bool,
) -> Result<bool, String> {
    let expected_name = file.path.rsplit('/').next().unwrap_or_default();
    let source_path = join_path(&job.source_openlist_path, &file.path)?;
    let source_exists = match openlist.stat_if_exists(&source_path).await? {
        Some(source) => {
            validate_openlist_manifest_object(
                &source,
                expected_name,
                file.size,
                "源",
                &source_path,
            )?;
            true
        }
        None if allow_missing_source => false,
        None => return Err(format!("OpenList 源 manifest 文件不存在: {source_path}")),
    };
    let target_path = join_path(&job.target_openlist_path, &file.path)?;
    let target = openlist
        .stat_if_exists(&target_path)
        .await?
        .ok_or_else(|| format!("OpenList 目标 manifest 文件不存在: {target_path}"))?;
    validate_openlist_manifest_object(&target, expected_name, file.size, "目标", &target_path)?;
    Ok(source_exists)
}

fn validate_openlist_manifest_object(
    object: &crate::openlist::OpenListObject,
    expected_name: &str,
    expected_size: i64,
    side: &str,
    path: &str,
) -> Result<(), String> {
    if object.is_dir {
        return Err(format!(
            "OpenList {side} manifest 路径是目录而非文件: {path}"
        ));
    }
    if object.name != expected_name {
        return Err(format!(
            "OpenList {side} manifest 文件名不匹配: {:?} != {:?}",
            object.name, expected_name
        ));
    }
    if object.size != expected_size {
        return Err(format!(
            "OpenList {side} manifest 文件大小不匹配: {path} ({} != {expected_size})",
            object.size
        ));
    }
    Ok(())
}

fn advanced_stage_requires_manifest(stage: &str) -> bool {
    matches!(
        stage,
        "torrent_exported" | "source_qb_removed" | "target_qb_submitted" | "target_qb_starting"
    )
}

fn validate_distinct_relocation_paths(job: &MediaRelocationJob) -> Result<(), String> {
    validate_distinct_path_pairs(
        &job.source_openlist_path,
        &job.target_openlist_path,
        &job.source_qb_path,
        &job.target_qb_path,
    )
}

fn validate_distinct_path_pairs(
    source_openlist: &str,
    target_openlist: &str,
    source_qb: &str,
    target_qb: &str,
) -> Result<(), String> {
    let source_openlist = normalize_path(source_openlist)?;
    let target_openlist = normalize_path(target_openlist)?;
    if source_openlist == target_openlist {
        return Err(format!(
            "源与目标 OpenList 路径相同，已拒绝迁移: {source_openlist}"
        ));
    }
    let source_qb = normalize_path(source_qb)?;
    let target_qb = normalize_path(target_qb)?;
    if source_qb == target_qb {
        return Err(format!("源与目标 qB 路径相同，已拒绝迁移: {source_qb}"));
    }
    Ok(())
}

fn manifest_scan_end(cursor: usize, manifest_len: usize) -> Result<usize, String> {
    if cursor > manifest_len {
        return Err(format!("manifest cursor 越界: {cursor} > {manifest_len}"));
    }
    Ok(cursor
        .saturating_add(MANIFEST_FILES_PER_PASS)
        .min(manifest_len))
}

fn decide_copy_tasks(observations: &[CopyTaskObservation]) -> Result<CopyTaskDecision, String> {
    if observations.is_empty() {
        return Err("OpenList 复制任务观察结果为空".to_string());
    }
    if observations.contains(&CopyTaskObservation::Pending) {
        return Ok(CopyTaskDecision::Wait);
    }
    if observations
        .iter()
        .all(|state| *state == CopyTaskObservation::Failed)
    {
        return Ok(CopyTaskDecision::AllFailed);
    }
    Ok(CopyTaskDecision::VerifyTarget)
}

fn prepare_read_only_copy_reconcile(
    checkpoint_json: &mut Option<String>,
    now: chrono::DateTime<Utc>,
) -> Result<&'static str, String> {
    let Some(value) = checkpoint_json.as_deref() else {
        return Ok("copy_legacy_reconcile");
    };
    let mut checkpoint = decode_copy_checkpoint(value)?;
    checkpoint.phase = CopyCheckpointPhase::Uncertain;
    checkpoint
        .submitted_at
        .get_or_insert_with(|| now.to_rfc3339());
    *checkpoint_json = Some(encode_copy_checkpoint(&checkpoint)?);
    Ok("copy_submitting")
}

fn checkpoint_pending_timed_out(
    checkpoint_json: Option<&str>,
    now: chrono::DateTime<Utc>,
) -> Result<bool, String> {
    let Some(value) = checkpoint_json else {
        return Ok(false);
    };
    let checkpoint = decode_copy_checkpoint(value)?;
    let Some(submitted_at) = checkpoint.submitted_at.as_deref() else {
        return Ok(false);
    };
    let submitted_at = chrono::DateTime::parse_from_rfc3339(submitted_at)
        .map_err(|error| format!("解析复制 checkpoint 提交时间失败: {error}"))?
        .with_timezone(&Utc);
    Ok(now.signed_duration_since(submitted_at)
        >= ChronoDuration::seconds(COPY_TASK_PENDING_MANUAL_SECONDS))
}

fn encode_copy_checkpoint(checkpoint: &CopyCheckpoint) -> Result<String, String> {
    serde_json::to_string(checkpoint)
        .map_err(|error| format!("序列化 OpenList 复制 checkpoint 失败: {error}"))
}

fn decode_copy_checkpoint(value: &str) -> Result<CopyCheckpoint, String> {
    let mut checkpoint = serde_json::from_str::<CopyCheckpoint>(value)
        .map_err(|error| format!("解析 OpenList 复制 checkpoint 失败: {error}"))?;
    checkpoint.path = normalize_manifest_path(&checkpoint.path)?;
    if checkpoint.size < 0 {
        return Err(format!(
            "OpenList 复制 checkpoint 文件大小无效: {}={}",
            checkpoint.path, checkpoint.size
        ));
    }
    Ok(checkpoint)
}

fn validate_checkpoint_against_manifest(
    checkpoint: &CopyCheckpoint,
    manifest_json: &str,
) -> Result<usize, String> {
    let manifest = decode_source_manifest(manifest_json)?
        .ok_or("种子文件大小快照缺失，无法校验复制 checkpoint")?;
    manifest
        .iter()
        .position(|file| file.path == checkpoint.path && file.size == checkpoint.size)
        .ok_or_else(|| {
            format!(
                "OpenList 复制 checkpoint 不在权威种子清单中: {}",
                checkpoint.path
            )
        })
}

fn copy_submission_attention_at(
    checkpoint: &CopyCheckpoint,
) -> Result<chrono::DateTime<Utc>, String> {
    let submitted_at = checkpoint
        .submitted_at
        .as_deref()
        .ok_or("uncertain 复制 checkpoint 缺少提交时间")?;
    let submitted_at = chrono::DateTime::parse_from_rfc3339(submitted_at)
        .map_err(|error| format!("解析复制 checkpoint 提交时间失败: {error}"))?
        .with_timezone(&Utc);
    Ok(submitted_at + ChronoDuration::seconds(COPY_SUBMISSION_ATTENTION_SECONDS))
}

fn uncertain_submission_next_check(
    checkpoint: &CopyCheckpoint,
    now: chrono::DateTime<Utc>,
) -> Result<Option<chrono::DateTime<Utc>>, String> {
    let attention_at = copy_submission_attention_at(checkpoint)?;
    if attention_at <= now {
        Ok(None)
    } else {
        Ok(Some(std::cmp::min(
            attention_at,
            now + ChronoDuration::seconds(30),
        )))
    }
}

fn valid_infohash(value: &str) -> bool {
    value.len() == 40 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
}
