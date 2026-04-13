pub mod scheduler;
pub mod selector;
pub mod cleaner;

use serde::{Deserialize, Serialize};

/// 刷流任务配置 (完整数据库记录)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrushTaskRecord {
    pub id: i64,
    pub name: String,
    pub cron_expression: String,
    pub site_id: Option<i64>,
    pub downloader_id: i64,
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
    // 删种规则
    pub delete_mode: String,
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
}

/// 创建/更新刷流任务的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrushTaskRequest {
    pub name: String,
    pub cron_expression: String,
    pub site_id: Option<i64>,
    pub downloader_id: i64,
    pub tag: String,
    pub rss_url: String,
    pub seed_volume_gb: Option<f64>,
    pub save_dir: Option<String>,
    pub active_time_windows: Option<String>,  // JSON array string
    pub promotion: Option<String>,
    pub skip_hit_and_run: Option<bool>,
    pub max_concurrent: Option<i32>,
    pub download_speed_limit: Option<i64>,
    pub upload_speed_limit: Option<i64>,
    pub size_ranges: Option<String>,  // JSON array string
    pub seeder_ranges: Option<String>,  // JSON array string
    pub delete_mode: Option<String>,
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
    pub torrent_hash: String,
    pub torrent_name: String,
    pub added_at: String,
    pub size_bytes: Option<i64>,
    pub is_hr: bool,
    pub status: String,
    pub removed_at: Option<String>,
    pub remove_reason: Option<String>,
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
    ranges.iter().any(|(min, max)| value >= *min && value <= *max)
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

    let now = chrono::Local::now();
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
