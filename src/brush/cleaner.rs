use chrono::Utc;

use crate::brush::BrushTaskRecord;
use crate::brush::BrushTorrentRecord;
use crate::db::Database;
use crate::downloader::TorrentInfo;

/// 评估删种规则，返回需要删除的种子 (hash, reason)
pub async fn evaluate_delete_rules(
    task: &BrushTaskRecord,
    managed: &[BrushTorrentRecord],
    downloader_torrents: &[TorrentInfo],
    db: &Database,
) -> Vec<(String, String)> {
    let is_and_mode = task.delete_mode == "and";
    let mut to_remove = Vec::new();

    for record in managed {
        if record.status != "active" {
            continue;
        }

        // 查找对应的下载器种子信息
        let Some(dl_info) = find_matching_downloader_torrent(record, downloader_torrents) else {
            // 下载器中不存在的种子，标记为已移除
            to_remove.push((record.torrent_hash.clone(), "下载器中不存在".to_string()));
            continue;
        };

        let mut reasons = Vec::new();
        let mut rule_results = Vec::new();

        // 规则 1: 最小做种时长
        if let Some(min_hours) = task.min_seed_time_hours {
            let seed_hours = dl_info.time_active as f64 / 3600.0;
            let passed = seed_hours >= min_hours;
            rule_results.push(passed);
            if passed {
                reasons.push(format!("做种时间 {:.1}h >= {:.1}h", seed_hours, min_hours));
            }
        }

        // 规则 1.5: free 到期删除
        if task.delete_on_free_expiry {
            if let Some(free_end_timestamp) = record.free_end_timestamp {
                let now_secs = Utc::now().timestamp();
                let passed = now_secs >= free_end_timestamp;
                rule_results.push(passed);
                if passed {
                    reasons.push("free已到期".to_string());
                }
            }
        }

        // 规则 2: H&R 种子最小做种时长
        if record.is_hr {
            if let Some(hr_min_hours) = task.hr_min_seed_time_hours {
                let seed_hours = dl_info.time_active as f64 / 3600.0;
                let passed = seed_hours >= hr_min_hours;
                rule_results.push(passed);
                if passed {
                    reasons.push(format!(
                        "H&R做种时间 {:.1}h >= {:.1}h",
                        seed_hours, hr_min_hours
                    ));
                }
            }
        }

        // 规则 3: 分享率
        if let Some(target_ratio) = task.target_ratio {
            let passed = dl_info.ratio >= target_ratio;
            rule_results.push(passed);
            if passed {
                reasons.push(format!(
                    "分享率 {:.2} >= {:.2}",
                    dl_info.ratio, target_ratio
                ));
            }
        }

        // 规则 4: 上传量
        if let Some(max_gb) = task.max_upload_gb {
            let uploaded_gb = dl_info.uploaded as f64 / (1024.0 * 1024.0 * 1024.0);
            let passed = uploaded_gb >= max_gb;
            rule_results.push(passed);
            if passed {
                reasons.push(format!("上传量 {:.2}GB >= {:.2}GB", uploaded_gb, max_gb));
            }
        }

        // 规则 5: 下载耗时
        if let Some(timeout_hours) = task.download_timeout_hours {
            if dl_info.completion_on <= 0 {
                // 尚未完成下载
                let added_secs = dl_info.added_on;
                let now_secs = Utc::now().timestamp();
                let elapsed_hours = (now_secs - added_secs) as f64 / 3600.0;
                let passed = elapsed_hours >= timeout_hours;
                rule_results.push(passed);
                if passed {
                    reasons.push(format!(
                        "下载耗时 {:.1}h >= {:.1}h，未完成",
                        elapsed_hours, timeout_hours
                    ));
                }
            }
        }

        // 规则 6: 最近10分钟平均上传速度
        if let Some(min_speed) = task.min_avg_upload_speed_kbs {
            let avg_speed = get_recent_avg_upload_speed(db, task.id, &record.torrent_hash).await;
            let avg_kbs = avg_speed / 1024.0;
            let passed = avg_kbs < min_speed && dl_info.time_active > 600;
            rule_results.push(passed);
            if passed {
                reasons.push(format!(
                    "近10分钟平均上传 {:.1}KB/s < {:.1}KB/s",
                    avg_kbs, min_speed
                ));
            }
        }

        // 规则 7: 最大未活跃时长
        if let Some(max_hours) = task.max_inactive_hours {
            if dl_info.last_activity > 0 {
                let now_secs = Utc::now().timestamp();
                let inactive_hours = (now_secs - dl_info.last_activity) as f64 / 3600.0;
                let passed = inactive_hours >= max_hours;
                rule_results.push(passed);
                if passed {
                    reasons.push(format!(
                        "未活跃 {:.1}h >= {:.1}h",
                        inactive_hours, max_hours
                    ));
                }
            }
        }

        // 规则 8: 磁盘最小剩余空间 (这个是全局的，但仍然按规则模式处理)
        // 磁盘空间检查在外部处理，这里不重复

        // 根据模式判断是否需要删除
        if rule_results.is_empty() {
            continue;
        }

        let should_remove = if is_and_mode {
            rule_results.iter().all(|&r| r)
        } else {
            rule_results.iter().any(|&r| r)
        };

        if should_remove && !reasons.is_empty() {
            to_remove.push((record.torrent_hash.clone(), reasons.join("; ")));
        }
    }

    to_remove
}

fn find_matching_downloader_torrent<'a>(
    record: &BrushTorrentRecord,
    downloader_torrents: &'a [TorrentInfo],
) -> Option<&'a TorrentInfo> {
    downloader_torrents
        .iter()
        .find(|torrent| torrent.hash.eq_ignore_ascii_case(&record.torrent_hash))
        .or_else(|| {
            downloader_torrents
                .iter()
                .find(|torrent| torrent.name == record.torrent_name)
        })
}

/// 获取最近10分钟的平均上传速度 (bytes/s)
async fn get_recent_avg_upload_speed(db: &Database, task_id: i64, hash: &str) -> f64 {
    match db.get_recent_torrent_traffic(task_id, hash, 10).await {
        Ok(snapshots) if snapshots.len() >= 2 => {
            let first = &snapshots[0];
            let last = &snapshots[snapshots.len() - 1];
            let bytes_diff = (last.0 - first.0).max(0) as f64;

            // 解析时间差
            let first_time = chrono::DateTime::parse_from_rfc3339(&first.2).ok();
            let last_time = chrono::DateTime::parse_from_rfc3339(&last.2).ok();
            if let (Some(ft), Some(lt)) = (first_time, last_time) {
                let secs = (lt - ft).num_seconds().max(1) as f64;
                bytes_diff / secs
            } else {
                0.0
            }
        }
        _ => 0.0,
    }
}
