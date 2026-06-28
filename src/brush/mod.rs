pub mod cleaner;
pub mod scheduler;
pub mod u2;

use serde::{Deserialize, Serialize};

/// 刷流任务配置 (完整数据库记录)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrushTaskRecord {
    pub id: i64,
    pub name: String,
    pub cron_expression: String,
    pub site_id: Option<i64>,
    pub downloader_ids: Vec<i64>,
    pub tag: String,
    pub rss_url: String,
    // 可选项
    pub seed_volume_gb: Option<f64>,
    pub save_dir: Option<String>,
    pub active_time_windows: Option<String>,
    // 选种规则
    pub promotion: String,
    pub skip_hit_and_run: bool,
    pub max_concurrent: i32,
    pub download_speed_limit: Option<i64>,
    pub upload_speed_limit: Option<i64>,
    pub size_ranges: Option<String>,
    pub seeder_ranges: Option<String>,
    pub downloader_ranges: Option<String>,
    pub downloader_weights: Option<String>,
    pub min_free_hours: Option<f64>,
    // 删种规则
    pub delete_mode: String,
    pub delete_on_free_expiry: bool,
    pub min_seed_time_hours: Option<f64>,
    pub hr_min_seed_time_hours: Option<f64>,
    pub target_ratio: Option<f64>,
    pub max_upload_gb: Option<f64>,
    pub download_timeout_hours: Option<f64>,
    pub min_avg_upload_speed_kbs: Option<f64>,
    pub max_inactive_hours: Option<f64>,
    pub min_disk_space_gb: Option<f64>,
    // 状态
    pub enabled: bool,
    pub created_at: String,
    pub updated_at: String,
    pub last_run_info: Option<String>,
}

impl BrushTaskRecord {
    /// `save_dir` 存储为 JSON `{qb_id: "绝对路径"}`。返回指定 qb 的保存路径。
    pub fn get_save_path(&self, downloader_id: i64) -> Option<String> {
        let json = self.save_dir.as_deref()?;
        let map: std::collections::HashMap<String, String> = serde_json::from_str(json).ok()?;
        map.get(&downloader_id.to_string()).cloned()
    }

    /// `downloader_weights` 存储为 JSON `{qb_id: weight}`。返回指定 qb 的权重。
    pub fn get_downloader_weight(&self, downloader_id: i64) -> Option<i32> {
        let json = self.downloader_weights.as_deref()?;
        let map: std::collections::HashMap<String, i32> = serde_json::from_str(json).ok()?;
        map.get(&downloader_id.to_string()).copied()
    }
}

/// 创建/更新刷流任务的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrushTaskRequest {
    pub name: String,
    pub cron_expression: String,
    pub site_id: Option<i64>,
    pub downloader_ids: Vec<i64>,
    pub tag: String,
    pub rss_url: String,
    pub seed_volume_gb: Option<f64>,
    pub save_dir: Option<String>,
    pub active_time_windows: Option<String>, // JSON array string
    pub promotion: Option<String>,
    pub skip_hit_and_run: Option<bool>,
    pub max_concurrent: Option<i32>,
    pub download_speed_limit: Option<i64>,
    pub upload_speed_limit: Option<i64>,
    pub size_ranges: Option<String>,   // JSON array string
    pub seeder_ranges: Option<String>, // JSON array string
    pub downloader_ranges: Option<String>, // JSON array string
    pub downloader_weights: Option<String>, // JSON {qb_id: weight}
    pub min_free_hours: Option<f64>,
    pub delete_mode: Option<String>,
    pub delete_on_free_expiry: Option<bool>,
    pub min_seed_time_hours: Option<f64>,
    pub hr_min_seed_time_hours: Option<f64>,
    pub target_ratio: Option<f64>,
    pub max_upload_gb: Option<f64>,
    pub download_timeout_hours: Option<f64>,
    pub min_avg_upload_speed_kbs: Option<f64>,
    pub max_inactive_hours: Option<f64>,
    pub min_disk_space_gb: Option<f64>,
}

/// 任务管理的种子记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrushTorrentRecord {
    pub id: i64,
    pub task_id: i64,
    pub torrent_id: Option<String>,
    pub torrent_link: Option<String>,
    pub torrent_hash: String,
    pub torrent_name: String,
    pub added_at: String,
    pub size_bytes: Option<i64>,
    pub is_hr: bool,
    pub free_end_timestamp: Option<i64>,
    pub status: String,
    pub removed_at: Option<String>,
    pub remove_reason: Option<String>,
    pub uploaded_bytes: i64,
    pub downloaded_bytes: i64,
    pub download_duration_secs: i64,
    pub avg_upload_speed: f64,
    pub ratio: f64,
    pub last_stats_at: Option<String>,
    pub downloader_id: Option<i64>,
}

/// 解析范围字符串 (如 "0-10", "1-100")
pub fn parse_range(s: &str) -> Option<(f64, f64)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() == 2 {
        let min = parts[0].trim().parse::<f64>().ok()?;
        let max = parts[1].trim().parse::<f64>().ok()?;
        Some((min, max))
    } else {
        None
    }
}

/// 解析范围列表 JSON
pub fn parse_ranges(json_str: &str) -> Vec<(f64, f64)> {
    serde_json::from_str::<Vec<String>>(json_str)
        .unwrap_or_default()
        .iter()
        .filter_map(|s| parse_range(s))
        .collect()
}

/// 检查值是否在任一范围内
pub fn in_any_range(value: f64, ranges: &[(f64, f64)]) -> bool {
    if ranges.is_empty() {
        return true; // 没有配置范围限制 = 全部通过
    }
    ranges
        .iter()
        .any(|(min, max)| value >= *min && value <= *max)
}

/// 解析时间窗口 (如 "00:00-09:00")
pub fn parse_time_window(s: &str) -> Option<(u32, u32, u32, u32)> {
    let parts: Vec<&str> = s.split('-').collect();
    if parts.len() != 2 {
        return None;
    }
    let start_parts: Vec<&str> = parts[0].trim().split(':').collect();
    let end_parts: Vec<&str> = parts[1].trim().split(':').collect();
    if start_parts.len() != 2 || end_parts.len() != 2 {
        return None;
    }
    let sh = start_parts[0].parse::<u32>().ok()?;
    let sm = start_parts[1].parse::<u32>().ok()?;
    let eh = end_parts[0].parse::<u32>().ok()?;
    let em = end_parts[1].parse::<u32>().ok()?;
    Some((sh, sm, eh, em))
}

/// 检查当前时间是否在活跃时间窗口内
pub fn is_in_active_window(windows_json: Option<&str>) -> bool {
    let Some(json_str) = windows_json else {
        return true; // 没配置 = 全天活跃
    };

    let windows: Vec<String> = match serde_json::from_str(json_str) {
        Ok(w) => w,
        Err(_) => return true,
    };

    if windows.is_empty() {
        return true;
    }

    let now = chrono::Utc::now();
    let current_minutes = now.format("%H").to_string().parse::<u32>().unwrap_or(0) * 60
        + now.format("%M").to_string().parse::<u32>().unwrap_or(0);

    for window in &windows {
        if let Some((sh, sm, eh, em)) = parse_time_window(window) {
            let start = sh * 60 + sm;
            let end = eh * 60 + em;
            if start <= end {
                if current_minutes >= start && current_minutes < end {
                    return true;
                }
            } else {
                // 跨天窗口 (如 22:00-06:00)
                if current_minutes >= start || current_minutes < end {
                    return true;
                }
            }
        }
    }

    false
}

/// 平均上传速度 (bytes/s)。做种时长非正时返回 0。
pub fn average_upload_speed(uploaded_bytes: i64, duration_secs: i64) -> f64 {
    if duration_secs <= 0 {
        0.0
    } else {
        uploaded_bytes as f64 / duration_secs as f64
    }
}

/// 分享率。有下载量时按 上传/下载 计算；无下载但有上传时回退到下载器上报值。
pub fn calculate_ratio(uploaded_bytes: i64, downloaded_bytes: i64, fallback: f64) -> f64 {
    if downloaded_bytes > 0 {
        uploaded_bytes as f64 / downloaded_bytes as f64
    } else if uploaded_bytes > 0 {
        fallback.max(0.0)
    } else {
        0.0
    }
}

/// 刷流任务最后一次执行的详细信息 (序列化为 JSON 存入 brush_tasks.last_run_info)
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct BrushTaskLastRunInfo {
    /// 触发方式: cron / manual
    pub trigger_type: String,
    /// 执行开始时间 (RFC3339)
    pub started_at: String,
    /// 执行结束时间 (RFC3339)
    pub finished_at: String,
    /// 总耗时 (秒)
    pub duration_secs: f64,
    /// 整体结果: success / failed / skipped
    pub status: String,
    /// 失败时的错误信息
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// 提前结束原因: no_downloaders / concurrency_limit / seed_volume_limit
    #[serde(skip_serializing_if = "Option::is_none")]
    pub early_exit_reason: Option<String>,
    pub downloaders: LastRunDownloaders,
    pub sync: LastRunSync,
    pub concurrency: LastRunConcurrency,
    pub seed_volume: LastRunSeedVolume,
    pub source: LastRunSource,
    pub selection: LastRunSelection,
    pub added_torrents: Vec<LastRunAddedTorrent>,
    pub failed_torrents: Vec<LastRunFailedTorrent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastRunDownloaders {
    /// 进入候选的下载器
    pub candidates: Vec<LastRunDownloaderCandidate>,
    /// 被排除的下载器
    pub skipped: Vec<LastRunDownloaderSkipped>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRunDownloaderCandidate {
    pub id: i64,
    pub name: String,
    pub free_space_gb: f64,
    pub weight: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRunDownloaderSkipped {
    pub id: i64,
    pub name: String,
    /// not_exist / client_create_failed / free_space_fetch_failed / space_insufficient
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastRunSync {
    /// 执行前系统管理的活跃种子数
    pub managed_before: usize,
    /// 下载器中已不存在、本轮标记为 removed 的数量
    pub missing_marked_removed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastRunConcurrency {
    pub active_count: i32,
    pub max_concurrent: i32,
    pub can_add: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastRunSeedVolume {
    pub current_gb: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit_gb: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastRunSource {
    /// rss / u2_shoutbox
    #[serde(rename = "type")]
    pub source_type: String,
    pub items_parsed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LastRunSelection {
    pub checked: usize,
    pub added: usize,
    pub failed: usize,
    pub skipped_detail_failure: usize,
    pub skipped_existing: usize,
    pub skipped_pre_filter: usize,
    pub skipped_post_filter: usize,
    pub skipped_no_space: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRunAddedTorrent {
    pub title: String,
    pub hash: String,
    pub size_bytes: Option<i64>,
    pub downloader_id: i64,
    pub downloader_name: String,
    pub is_hr: bool,
    pub is_free: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LastRunFailedTorrent {
    pub title: String,
    /// download_failed / invalid_torrent / all_downloaders_failed / detail_fetch_failed
    pub reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

impl BrushTaskLastRunInfo {
    pub fn new(trigger_type: &str, started_at: String) -> Self {
        Self {
            trigger_type: trigger_type.to_string(),
            started_at,
            ..Default::default()
        }
    }
}
