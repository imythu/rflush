use std::collections::HashSet;

use tracing::debug;

use crate::brush::{in_any_range, parse_ranges, BrushTaskRecord};
use crate::rss::{FeedSnapshot, TorrentItem};

/// 从 RSS 快照中选择符合条件的种子
pub fn select_torrents(
    task: &BrushTaskRecord,
    snapshot: &FeedSnapshot,
    existing_hashes: &HashSet<String>,
) -> Vec<TorrentItem> {
    let size_ranges = task
        .size_ranges
        .as_deref()
        .map(parse_ranges)
        .unwrap_or_default();

    let seeder_ranges = task
        .seeder_ranges
        .as_deref()
        .map(parse_ranges)
        .unwrap_or_default();

    let mut candidates: Vec<TorrentItem> = snapshot
        .items
        .values()
        .filter(|item| {
            // 跳过已存在的种子
            if existing_hashes.contains(&item.guid) {
                return false;
            }

            // 种子体积筛选 (单位 GB)
            if !size_ranges.is_empty() {
                match item.size_bytes {
                    Some(bytes) => {
                        let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                        if !in_any_range(gb, &size_ranges) {
                            return false;
                        }
                    }
                    None => {
                        // RSS 未提供体积信息时跳过该规则（不过滤）
                    }
                }
            }

            // 做种人数筛选
            if !seeder_ranges.is_empty() {
                match item.seeders {
                    Some(s) => {
                        if !in_any_range(s as f64, &seeder_ranges) {
                            return false;
                        }
                    }
                    None => {
                        // RSS 未提供做种数信息时跳过该规则
                    }
                }
            }

            // 促销筛选
            match task.promotion.as_str() {
                "free" => {
                    let is_free = item.is_free();
                    if !is_free {
                        debug!("[刷流][{}] 跳过非免费种子: {} dl_factor={:?} ul_factor={:?}",
                               task.name, item.title, item.download_volume_factor, item.upload_volume_factor);
                        return false;
                    } else {
                        debug!("[刷流][{}] 接受免费种子: {} dl_factor={:?} ul_factor={:?}",
                               task.name, item.title, item.download_volume_factor, item.upload_volume_factor);
                    }
                }
                "normal" => {
                    if item.is_promoted() {
                        return false;
                    }
                }
                _ => {}
            }

            // H&R 检查
            if task.skip_hit_and_run && item.is_hr() {
                return false;
            }

            true
        })
        .cloned()
        .collect();

    // 按发布时间排序，优先选择新种子
    candidates.sort_by(|a, b| {
        b.pub_date
            .as_deref()
            .unwrap_or("")
            .cmp(a.pub_date.as_deref().unwrap_or(""))
    });

    candidates
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};

    use crate::brush::BrushTaskRecord;
    use crate::rss::{FeedSnapshot, TorrentItem};

    use super::select_torrents;

    fn task() -> BrushTaskRecord {
        BrushTaskRecord {
            id: 1,
            name: "test".to_string(),
            cron_expression: "0 * * * * *".to_string(),
            site_id: None,
            downloader_id: 1,
            tag: "brush".to_string(),
            rss_url: "https://example.test/rss".to_string(),
            seed_volume_gb: None,
            save_dir: None,
            active_time_windows: None,
            promotion: "all".to_string(),
            skip_hit_and_run: false,
            max_concurrent: 10,
            download_speed_limit: None,
            upload_speed_limit: None,
            size_ranges: None,
            seeder_ranges: None,
            delete_mode: "or".to_string(),
            min_seed_time_hours: None,
            hr_min_seed_time_hours: None,
            target_ratio: None,
            max_upload_gb: None,
            download_timeout_hours: None,
            min_avg_upload_speed_kbs: None,
            max_inactive_hours: None,
            min_disk_space_gb: None,
            enabled: true,
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            updated_at: "2026-01-01T00:00:00+00:00".to_string(),
        }
    }

    fn item(guid: &str, download_volume_factor: Option<f64>, minimum_seed_time: Option<u64>) -> TorrentItem {
        TorrentItem {
            rss_name: "rss".to_string(),
            guid: guid.to_string(),
            title: guid.to_string(),
            link: None,
            pub_date: Some("Mon, 01 Jan 2026 00:00:00 GMT".to_string()),
            download_url: format!("https://example.test/{guid}.torrent"),
            version: 1,
            size_bytes: Some(1024),
            seeders: Some(10),
            download_volume_factor,
            upload_volume_factor: Some(1.0),
            minimum_ratio: None,
            minimum_seed_time,
        }
    }

    fn snapshot(items: Vec<TorrentItem>) -> FeedSnapshot {
        FeedSnapshot {
            version: 1,
            items: items
                .into_iter()
                .map(|item| (item.guid.clone(), item))
                .collect::<HashMap<_, _>>(),
        }
    }

    #[test]
    fn filters_free_only_items() {
        let mut task = task();
        task.promotion = "free".to_string();

        let selected = select_torrents(
            &task,
            &snapshot(vec![item("free", Some(0.0), None), item("normal", Some(1.0), None)]),
            &HashSet::new(),
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].guid, "free");
    }

    #[test]
    fn filters_out_promoted_items_for_normal_mode() {
        let mut task = task();
        task.promotion = "normal".to_string();

        let selected = select_torrents(
            &task,
            &snapshot(vec![
                item("discount", Some(0.5), None),
                item("normal", Some(1.0), None),
            ]),
            &HashSet::new(),
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].guid, "normal");
    }

    #[test]
    fn skips_hr_items_when_configured() {
        let mut task = task();
        task.skip_hit_and_run = true;

        let selected = select_torrents(
            &task,
            &snapshot(vec![item("hr", Some(1.0), Some(86400)), item("safe", Some(1.0), None)]),
            &HashSet::new(),
        );

        assert_eq!(selected.len(), 1);
        assert_eq!(selected[0].guid, "safe");
    }
}
