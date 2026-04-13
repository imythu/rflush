use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use chrono::Utc;
use cron::Schedule;
use tokio::sync::RwLock;
use tokio::time::{Duration, sleep};
use tracing::{debug, error, info, warn};

use crate::brush::{is_in_active_window, in_any_range, parse_ranges, BrushTaskRecord};
use crate::db::Database;
use crate::downloader::{AddTorrentOptions, DownloaderType, TorrentInfo, create_downloader_client};
use crate::rss;
use crate::site::{SiteAuth, SiteType, create_site_client};
use crate::stats;

use super::cleaner;

const DETAIL_ATTR_CACHE_TTL_SECS: i64 = 600;
const DETAIL_ATTR_FETCH_MAX_CONCURRENCY: usize = 1;

#[derive(Clone, Copy)]
enum FilterStage {
    RssPreFilter,
    PostEnhancement,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct DetailAttrCacheStats {
    pub ttl_secs: i64,
    pub max_concurrency: usize,
    pub site_bucket_count: usize,
    pub cached_entry_count: usize,
    pub total_cache_hits: u64,
    pub total_fetch_successes: u64,
}

#[derive(Clone)]
struct CachedTorrentAttributes {
    attrs: crate::site::TorrentAttributes,
    fetched_at: i64,
}

type SiteDetailAttrCache = HashMap<i64, HashMap<String, CachedTorrentAttributes>>;

fn detail_attr_cache() -> &'static RwLock<SiteDetailAttrCache> {
    static CACHE: OnceLock<RwLock<SiteDetailAttrCache>> = OnceLock::new();
    CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

fn detail_attr_cache_hits() -> &'static AtomicU64 {
    static HITS: OnceLock<AtomicU64> = OnceLock::new();
    HITS.get_or_init(|| AtomicU64::new(0))
}

fn detail_attr_fetch_successes() -> &'static AtomicU64 {
    static SUCCESSES: OnceLock<AtomicU64> = OnceLock::new();
    SUCCESSES.get_or_init(|| AtomicU64::new(0))
}

/// 调度器状态
pub struct BrushScheduler {
    db: Database,
    running_tasks: Arc<RwLock<HashMap<i64, RunningBrushTask>>>,
}

struct RunningBrushTask {
    handle: tokio::task::JoinHandle<()>,
    config: Arc<RwLock<BrushTaskRecord>>,
}

impl BrushScheduler {
    pub fn new(db: Database) -> Self {
        Self {
            db,
            running_tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    /// 启动调度器，周期性检查所有启用的刷流任务
    pub async fn start(self: Arc<Self>) {
        info!("brush scheduler started");
        let stats_db = self.db.clone();

        // 启动数据统计采集 (每60秒)
        tokio::spawn(async move {
            stats::start_stats_collector(stats_db).await;
        });

        loop {
            if let Err(e) = self.check_and_schedule().await {
                error!("brush scheduler error: {}", e);
            }
            sleep(Duration::from_secs(15)).await;
        }
    }

    async fn check_and_schedule(&self) -> Result<(), String> {
        let tasks = self
            .db
            .list_brush_tasks()
            .await
            .map_err(|e| e.to_string())?;

        for task in tasks {
            if !task.enabled {
                // 如果任务被禁用，停止已运行的
                let mut running = self.running_tasks.write().await;
                if let Some(running_task) = running.remove(&task.id) {
                    running_task.handle.abort();
                    debug!("stopped disabled brush task: {}", task.name);
                }
                continue;
            }

            // 检查是否已有运行中的执行
            let running = self.running_tasks.read().await;
            let config = running.get(&task.id).map(|running_task| running_task.config.clone());
            drop(running);
            if let Some(config) = config {
                let mut config = config.write().await;
                *config = task.clone();
                continue;
            }

            // 检查是否应该触发
            if should_trigger(&task) {
                let db = self.db.clone();
                let running_tasks = self.running_tasks.clone();
                let task_id = task.id;
                let task_name = task.name.clone();
                let config = Arc::new(RwLock::new(task.clone()));
                let execution_config = config.clone();

                info!("[刷流][{}] cron 触发，开始调度执行", task_name);
                let handle = tokio::spawn(async move {
                    if let Err(e) = execute_brush_task(&db, execution_config).await {
                        error!("[刷流][{}] 任务执行失败: {}", task_name, e);
                    }
                    // 从运行列表移除
                    let mut running = running_tasks.write().await;
                    running.remove(&task_id);
                });

                let mut running = self.running_tasks.write().await;
                running.insert(task_id, RunningBrushTask { handle, config });
            }
        }

        Ok(())
    }

    /// 手动触发一个任务
    pub async fn trigger_task(&self, task_id: i64) -> Result<(), String> {
        let task = self
            .db
            .get_brush_task(task_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "任务不存在".to_string())?;

        let db = self.db.clone();
        let running_tasks = self.running_tasks.clone();

        // 检查是否已有运行中
        let running = self.running_tasks.read().await;
        if running.contains_key(&task_id) {
            return Err("任务正在运行中".to_string());
        }
        drop(running);

        let task_name = task.name.clone();
        let config = Arc::new(RwLock::new(task.clone()));
        let execution_config = config.clone();
        info!("[刷流][{}] 手动触发执行 (id={})", task_name, task_id);
        let handle = tokio::spawn(async move {
            if let Err(e) = execute_brush_task(&db, execution_config).await {
                error!("[刷流][{}] 手动执行失败: {}", task_name, e);
            }
            let mut running = running_tasks.write().await;
            running.remove(&task_id);
        });

        let mut running = self.running_tasks.write().await;
        running.insert(task_id, RunningBrushTask { handle, config });
        Ok(())
    }

    /// 停止一个任务
    pub async fn stop_task(&self, task_id: i64) {
        let mut running = self.running_tasks.write().await;
        if let Some(running_task) = running.remove(&task_id) {
            running_task.handle.abort();
        }
    }

    pub async fn refresh_task_config(&self, task_id: i64) -> Result<(), String> {
        let latest_task = self
            .db
            .get_brush_task(task_id)
            .await
            .map_err(|e| e.to_string())?
            .ok_or_else(|| "任务不存在".to_string())?;

        let running = self.running_tasks.read().await;
        let config = running.get(&task_id).map(|running_task| running_task.config.clone());
        drop(running);
        if let Some(config) = config {
            let mut config = config.write().await;
            *config = latest_task.clone();
            info!(
                "[刷流][{}] 运行中配置已刷新 (id={})",
                latest_task.name, latest_task.id
            );
        }

        Ok(())
    }

    pub async fn detail_attr_cache_stats(&self) -> DetailAttrCacheStats {
        let now = Utc::now().timestamp();
        prune_expired_detail_attr_cache(now).await;

        let cache = detail_attr_cache();
        let cache_guard = cache.read().await;
        let site_bucket_count = cache_guard.len();
        let cached_entry_count = cache_guard.values().map(|site_cache| site_cache.len()).sum();

        DetailAttrCacheStats {
            ttl_secs: DETAIL_ATTR_CACHE_TTL_SECS,
            max_concurrency: DETAIL_ATTR_FETCH_MAX_CONCURRENCY,
            site_bucket_count,
            cached_entry_count,
            total_cache_hits: detail_attr_cache_hits().load(Ordering::Relaxed),
            total_fetch_successes: detail_attr_fetch_successes().load(Ordering::Relaxed),
        }
    }
}

fn should_trigger(task: &BrushTaskRecord) -> bool {
    // 检查活跃时间窗口
    if !is_in_active_window(task.active_time_windows.as_deref()) {
        return false;
    }

    // 规范化并解析 cron 表达式 (5字段补秒)
    let cron_expr = {
        let fields: Vec<&str> = task.cron_expression.trim().split_whitespace().collect();
        if fields.len() == 5 {
            format!("0 {}", task.cron_expression.trim())
        } else {
            task.cron_expression.trim().to_string()
        }
    };
    let schedule: Schedule = match cron_expr.parse() {
        Ok(s) => s,
        Err(e) => {
            warn!("invalid cron '{}': {}", task.cron_expression, e);
            return false;
        }
    };

    // 检查上次执行时间和下次应执行时间
    let now = Utc::now();
    if let Some(next) = schedule.upcoming(Utc).next() {
        let diff = (next - now).num_seconds().abs();
        // 在 15 秒调度窗口内触发
        diff <= 15
    } else {
        false
    }
}

fn snapshot_task(task: &Arc<RwLock<BrushTaskRecord>>) -> impl std::future::Future<Output = BrushTaskRecord> + '_ {
    async move { task.read().await.clone() }
}

async fn execute_brush_task(
    db: &Database,
    shared_task: Arc<RwLock<BrushTaskRecord>>,
) -> Result<(), String> {
    let task_start = std::time::Instant::now();
    let task = snapshot_task(&shared_task).await;
    info!("[刷流][{}] 开始执行任务 (id={})", task.name, task.id);

    // 1. 获取下载器配置
    let downloader_record = db
        .get_downloader(task.downloader_id)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| "下载器不存在".to_string())?;

    info!(
        "[刷流][{}] 使用下载器: {} ({})",
        task.name, downloader_record.name, downloader_record.url
    );

    let dl_type = DownloaderType::from_str(&downloader_record.downloader_type)
        .ok_or_else(|| "不支持的下载器类型".to_string())?;

    let client = create_downloader_client(
        dl_type,
        &downloader_record.url,
        &downloader_record.username,
        &downloader_record.password,
    );

    // 2. 获取当前管理的种子列表
    let managed_torrents = db
        .list_active_brush_torrents(task.id)
        .await
        .map_err(|e| e.to_string())?;

    let downloader_torrents = client.list_torrents(Some(&task.tag)).await?;

    info!(
        "[刷流][{}] 当前状态: 本系统管理 {} 个种子, 下载器中标签[{}]共 {} 个种子",
        task.name,
        managed_torrents.len(),
        task.tag,
        downloader_torrents.len()
    );

    // 3. 执行删种规则
    let task = snapshot_task(&shared_task).await;
    let to_remove = cleaner::evaluate_delete_rules(&task, &managed_torrents, &downloader_torrents, db).await;
    if to_remove.is_empty() {
        info!("[刷流][{}] 删种检查: 无需删除种子", task.name);
    } else {
        info!("[刷流][{}] 删种检查: 准备删除 {} 个种子", task.name, to_remove.len());
    }
    for (hash, reason) in &to_remove {
        info!("[刷流][{}] 删种: hash={} 原因={}", task.name, &hash[..8.min(hash.len())], reason);
        if let Err(e) = client.delete_torrent(hash, true).await {
            warn!("[刷流][{}] 删种失败: hash={} err={}", task.name, &hash[..8.min(hash.len())], e);
        }
        let _ = db
            .update_brush_torrent_status(task.id, hash, "removed", Some(reason))
            .await;
    }

    // 4. 检查是否可以添加新种子
    let active_count = managed_torrents
        .iter()
        .filter(|t| t.status == "active")
        .count() as i32
        - to_remove.len() as i32;

    let can_add = (task.max_concurrent - active_count.max(0)).max(0) as usize;
    info!(
        "[刷流][{}] 并发检查: 活跃 {} 个, 最大 {}, 可添加 {} 个",
        task.name, active_count.max(0), task.max_concurrent, can_add
    );
    if can_add == 0 {
        info!("[刷流][{}] 已达并发上限，跳过选种", task.name);
        return Ok(());
    }

    // 5. 检查保种体积
    let mut current_size_gb = {
        let total: i64 = downloader_torrents.iter().map(|t| t.size).sum();
        total as f64 / (1024.0 * 1024.0 * 1024.0)
    };
    if let Some(max_gb) = task.seed_volume_gb {
        info!(
            "[刷流][{}] 保种体积: 当前 {:.2} GB / 上限 {:.2} GB",
            task.name, current_size_gb, max_gb
        );
        if current_size_gb >= max_gb {
            info!("[刷流][{}] 已达保种体积上限，跳过选种", task.name);
            return Ok(());
        }
    }

    let downloader_free_space_bytes = client
        .get_free_space(task.save_dir.as_deref())
        .await
        .ok();
    let pending_download_bytes = calculate_pending_download_bytes(&downloader_torrents);
    let mut effective_free_space_bytes = downloader_free_space_bytes
        .map(|free_space| free_space.saturating_sub(pending_download_bytes));

    if let Some(min_disk_space_gb) = task.min_disk_space_gb {
        let min_disk_space_bytes = gb_to_bytes(min_disk_space_gb);
        if let Some(free_space) = downloader_free_space_bytes {
            info!(
                "[刷流][{}] 磁盘空间: 当前空闲 {:.2} GB, 未完成剩余 {:.2} GB, 预测可用 {:.2} GB, 最低保留 {:.2} GB",
                task.name,
                bytes_to_gb(free_space),
                bytes_to_gb(pending_download_bytes),
                bytes_to_gb(effective_free_space_bytes.unwrap_or(0)),
                min_disk_space_gb
            );
        } else {
            warn!("[刷流][{}] 无法获取下载器剩余空间，跳过最小剩余空间检查", task.name);
        }

        if effective_free_space_bytes.is_some_and(|free_space| free_space < min_disk_space_bytes) {
            info!("[刷流][{}] 预测剩余空间低于阈值，跳过选种", task.name);
            return Ok(());
        }
    }

    // 6. 获取 RSS
    info!("[刷流][{}] 拉取 RSS: {}", task.name, task.rss_url);
    let rss_body = fetch_rss_text(&task.rss_url).await?;
    let rss_xml = std::str::from_utf8(rss_body.as_bytes()).map_err(|_| "RSS 编码错误".to_string())?;
    let parsed = rss::parse_feed(rss_xml).map_err(|e| format!("RSS 解析失败: {}", e))?;
    let snapshot = parsed.into_snapshot(task.name.clone(), 1);

    info!("[刷流][{}] RSS 解析完成，共 {} 个条目", task.name, snapshot.items.len());

    let existing_hashes: std::collections::HashSet<String> = managed_torrents
        .iter()
        .map(|t| t.torrent_id.clone().unwrap_or_else(|| t.torrent_hash.clone()))
        .collect();

    // 7. 准备站点详情增强
    let mut site_client: Option<Box<dyn crate::site::SiteClient>> = None;
    let mut site_client_binding: Option<Option<i64>> = None;

    // 排序：按发布时间降序，优先处理新种子
    let mut sorted_items: Vec<&rss::TorrentItem> = snapshot.items.values().collect();
    sorted_items.sort_by(|a, b| {
        b.pub_date.as_deref().unwrap_or("").cmp(a.pub_date.as_deref().unwrap_or(""))
    });

    // 8. 逐个增强、选种、添加
    let mut added = 0usize;
    let mut failed = 0usize;
    let mut checked = 0usize;
    let mut skipped_attrs = 0usize;

    for item in &sorted_items {
        let task = snapshot_task(&shared_task).await;
        let size_ranges = task.size_ranges.as_deref().map(parse_ranges).unwrap_or_default();
        let seeder_ranges = task.seeder_ranges.as_deref().map(parse_ranges).unwrap_or_default();
        let needs_site_attrs = task.promotion != "all" || task.skip_hit_and_run;
        checked += 1;

        // 跳过已存在的种子
        if existing_hashes.contains(&item.guid) {
            continue;
        }

        // 第一轮过滤：用 RSS 已有属性快速筛选，避免不必要的详情请求
        // RSS 中通常已有体积、做种数，部分站点也有促销/H&R 信息
        let pre_filter = check_filter_reason(
            &task,
            item,
            &size_ranges,
            &seeder_ranges,
            FilterStage::RssPreFilter,
        );
        if let Some(reason) = pre_filter {
            debug!(
                "[刷流][{}] ✗ {} id={} size={} seeders={} dl={:?} ul={:?} 原因: {}",
                task.name, item.title, extract_torrent_id(&item.guid),
                format_size(item.size_bytes),
                item.seeders.map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                item.download_volume_factor, item.upload_volume_factor,
                reason
            );
            continue;
        }

        // 详情增强：获取站点属性（只有 RSS 数据不足时才请求）
        let mut effective_item = (*item).clone();
        if needs_site_attrs {
            if site_client_binding != Some(task.site_id) {
                site_client = None;
                if let Some(site_id) = task.site_id {
                    if let Ok(sites) = db.list_sites().await {
                        if let Some(site) = sites.iter().find(|s| s.id == site_id) {
                            if let Some(site_type) = SiteType::from_str(&site.site_type) {
                                if let Ok(auth) = serde_json::from_str::<SiteAuth>(&site.auth_config) {
                                    site_client = Some(create_site_client(site_type, &site.base_url, &auth));
                                }
                            }
                        }
                    }
                }
                site_client_binding = Some(task.site_id);
            }

            // 检查 RSS 数据是否已经足够判断促销/H&R，足够则跳过请求
            let need_fetch = item.download_volume_factor.is_none()
                || (task.skip_hit_and_run && item.minimum_seed_time.is_none() && item.minimum_ratio.is_none());

            if need_fetch {
                if let Some(ref client) = site_client {
                    let detail_url = if task.site_id.is_some() {
                        item.link.as_deref().filter(|s| !s.is_empty())
                            .unwrap_or(item.guid.as_str())
                    } else {
                        item.guid.as_str()
                    };
                    let now = Utc::now().timestamp();

                    // 先查缓存
                    let mut cache_hit = false;
                    if let Some(sid) = task.site_id {
                        let cache = detail_attr_cache();
                        let cache_guard = cache.read().await;
                        if let Some(entry) = cache_guard
                            .get(&sid)
                            .and_then(|sc| sc.get(detail_url))
                            .filter(|e| now - e.fetched_at <= DETAIL_ATTR_CACHE_TTL_SECS)
                        {
                            apply_attrs_to_item(&mut effective_item, &entry.attrs);
                            cache_hit = true;
                            detail_attr_cache_hits().fetch_add(1, Ordering::Relaxed);
                        }
                    }

                    if !cache_hit {
                        match client.get_torrent_attributes(detail_url).await {
                            Ok(attrs) => {
                                apply_attrs_to_item(&mut effective_item, &attrs);
                                detail_attr_fetch_successes().fetch_add(1, Ordering::Relaxed);

                                // 写入缓存
                                if let Some(sid) = task.site_id {
                                    let mut cache_guard = detail_attr_cache().write().await;
                                    let site_cache = cache_guard.entry(sid).or_insert_with(HashMap::new);
                                    site_cache.insert(detail_url.to_string(), CachedTorrentAttributes {
                                        attrs,
                                        fetched_at: now,
                                    });
                                }
                            }
                            Err(e) => {
                                warn!(
                                    "[刷流][{}] ✗ id={} 详情获取失败: {} {}",
                                    task.name, extract_torrent_id(detail_url), e, detail_url
                                );
                                skipped_attrs += 1;
                                continue;
                            }
                        }
                    }
                }
            }
        }

        // 第二轮过滤：用详情增强后的完整属性再次筛选
        let post_filter = check_filter_reason(
            &task,
            &effective_item,
            &size_ranges,
            &seeder_ranges,
            FilterStage::PostEnhancement,
        );
        if let Some(reason) = post_filter {
            debug!(
                "[刷流][{}] ✗ {} id={} size={} seeders={} dl={:?} ul={:?} 原因: {}",
                task.name, effective_item.title, extract_torrent_id(&effective_item.guid),
                format_size(effective_item.size_bytes),
                effective_item.seeders.map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                effective_item.download_volume_factor, effective_item.upload_volume_factor,
                reason
            );
            continue;
        }

        // 检查并发是否还够
        let current_active = active_count as usize + added;
        if current_active >= task.max_concurrent as usize {
            info!(
                "[刷流][{}] 并发已达上限 {}/{}，停止添加",
                task.name, current_active, task.max_concurrent
            );
            break;
        }

        // 检查保种体积是否还够
        if let Some(max_gb) = task.seed_volume_gb {
            if current_size_gb >= max_gb {
                info!(
                    "[刷流][{}] 保种体积已达上限 {:.2}/{:.2} GB，停止添加",
                    task.name, current_size_gb, max_gb
                );
                break;
            }
        }

        if let Some(min_disk_space_gb) = task.min_disk_space_gb {
            if let (Some(predicted_free_space), Some(item_size_bytes)) =
                (effective_free_space_bytes, effective_item.size_bytes)
            {
                let min_disk_space_bytes = gb_to_bytes(min_disk_space_gb);
                if predicted_free_space < min_disk_space_bytes.saturating_add(item_size_bytes) {
                    info!(
                        "[刷流][{}] 跳过种子: {}，预测剩余空间不足。当前预测可用 {:.2} GB，种子大小 {:.2} GB，最低保留 {:.2} GB",
                        task.name,
                        effective_item.title,
                        bytes_to_gb(predicted_free_space),
                        bytes_to_gb(item_size_bytes),
                        min_disk_space_gb
                    );
                    continue;
                }
            }
        }

        // 下载种子文件并添加到下载器
        let torrent_data = fetch_torrent_bytes(&effective_item.download_url).await;
        match torrent_data {
            Ok(data) => {
                let save_path = task.save_dir.clone().unwrap_or_default();
                let options = AddTorrentOptions {
                    save_path: if save_path.is_empty() {
                        None
                    } else {
                        Some(save_path)
                    },
                    tags: Some(task.tag.clone()),
                    download_limit: task.download_speed_limit.map(|v| v * 1024),
                    upload_limit: task.upload_speed_limit.map(|v| v * 1024),
                    ..Default::default()
                };

                let filename = format!("{}.torrent", effective_item.guid);
                debug!(
                    "[刷流][{}] 准备添加到下载器: title={} filename={} download_url={} save_path={:?} tag={} dl_limit={:?} ul_limit={:?} torrent_bytes={}",
                    task.name,
                    effective_item.title,
                    filename,
                    effective_item.download_url,
                    options.save_path,
                    task.tag,
                    options.download_limit,
                    options.upload_limit,
                    data.len()
                );
                match client.add_torrent(data.clone(), &filename, &options).await {
                    Ok(()) => {
                        let torrent_id = effective_item
                            .link
                            .as_deref()
                            .map(extract_torrent_id)
                            .or_else(|| Some(extract_torrent_id(&effective_item.guid)))
                            .map(str::to_string)
                            .filter(|value| !value.is_empty());
                        let info_hash = extract_info_hash(&data).unwrap_or_else(|| effective_item.guid.clone());
                        let _ = db
                            .add_brush_torrent(
                                task.id,
                                torrent_id.as_deref(),
                                &info_hash,
                                &effective_item.title,
                                effective_item.size_bytes.map(|size| size as i64),
                                effective_item.is_hr(),
                            )
                            .await;
                        info!(
                            "[刷流][{}] ✓ 添加成功: {} id={} size={} seeders={} dl={:?} ul={:?} hr={}",
                            task.name, effective_item.title,
                            extract_torrent_id(&effective_item.guid),
                            format_size(effective_item.size_bytes),
                            effective_item.seeders.map(|s| s.to_string()).unwrap_or_else(|| "?".into()),
                            effective_item.download_volume_factor, effective_item.upload_volume_factor,
                            effective_item.is_hr()
                        );
                        added += 1;
                        if let Some(bytes) = effective_item.size_bytes {
                            current_size_gb += bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                            if let Some(free_space) = effective_free_space_bytes.as_mut() {
                                *free_space = free_space.saturating_sub(bytes);
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[刷流][{}] ✗ 添加到下载器失败: title={} filename={} download_url={} save_path={:?} tag={} err={}",
                            task.name,
                            effective_item.title,
                            filename,
                            effective_item.download_url,
                            options.save_path,
                            task.tag,
                            e
                        );
                        failed += 1;
                        info!("[刷流][{}] 下载器返回错误，停止本次添加", task.name);
                        break;
                    }
                }
            }
            Err(e) => {
                warn!("[刷流][{}] ✗ 下载种子文件失败: {} err={}", task.name, effective_item.title, e);
                failed += 1;
            }
        }
    }

    let elapsed = task_start.elapsed();
    info!(
        "[刷流][{}] 任务完成: 新增 {} 个, 删除 {} 个, 失败 {} 个, 跳过(详情失败) {} 个, 共检查 {} 个, 耗时 {:.1}s",
        task.name,
        added,
        to_remove.len(),
        failed,
        skipped_attrs,
        checked,
        elapsed.as_secs_f64()
    );

    Ok(())
}

/// 检查种子是否通过配置过滤条件，返回不通过的原因（通过则返回 None）
fn check_filter_reason(
    task: &BrushTaskRecord,
    item: &rss::TorrentItem,
    size_ranges: &[(f64, f64)],
    seeder_ranges: &[(f64, f64)],
    stage: FilterStage,
) -> Option<String> {
    // 种子体积筛选 (单位 GB)
    if !size_ranges.is_empty() {
        match item.size_bytes {
            Some(bytes) => {
                let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
                if !in_any_range(gb, size_ranges) {
                    return Some(format!("体积{:.2}GB不在范围", gb));
                }
            }
            None => {}
        }
    }

    // 做种人数筛选
    if !seeder_ranges.is_empty() {
        match item.seeders {
            Some(s) => {
                if !in_any_range(s as f64, seeder_ranges) {
                    return Some(format!("做种数{}不在范围", s));
                }
            }
            None => {}
        }
    }

    // 促销筛选
    match task.promotion.as_str() {
        "free" => {
            match item.download_volume_factor {
                Some(download_volume_factor) => {
                    if download_volume_factor > f64::EPSILON {
                        return Some(format!("非免费(dl={download_volume_factor:?})"));
                    }
                }
                None if matches!(stage, FilterStage::PostEnhancement) => {
                    return Some("缺少免费属性".to_string());
                }
                None => {}
            }
        }
        "normal" => {
            match (item.download_volume_factor, item.upload_volume_factor) {
                (Some(download_volume_factor), Some(upload_volume_factor)) => {
                    if download_volume_factor < 1.0 - f64::EPSILON
                        || (upload_volume_factor - 1.0).abs() > f64::EPSILON
                    {
                        return Some("有促销活动".to_string());
                    }
                }
                (Some(download_volume_factor), None) => {
                    if download_volume_factor < 1.0 - f64::EPSILON {
                        return Some("有促销活动".to_string());
                    }
                }
                (None, Some(upload_volume_factor)) => {
                    if (upload_volume_factor - 1.0).abs() > f64::EPSILON {
                        return Some("有促销活动".to_string());
                    }
                }
                (None, None) if matches!(stage, FilterStage::PostEnhancement) => {
                    return Some("缺少促销属性".to_string());
                }
                (None, None) => {}
            }
        }
        _ => {}
    }

    // H&R 检查
    if task.skip_hit_and_run {
        match (item.minimum_seed_time, item.minimum_ratio) {
            (Some(seed_time), _) if seed_time > 0 => return Some("H&R种子".to_string()),
            (_, Some(minimum_ratio)) if minimum_ratio > 0.0 => {
                return Some("H&R种子".to_string());
            }
            (None, None) if matches!(stage, FilterStage::PostEnhancement) => {
                return Some("缺少H&R属性".to_string());
            }
            _ => {}
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use crate::brush::BrushTaskRecord;
    use crate::downloader::TorrentInfo;
    use crate::rss::TorrentItem;

    use super::{FilterStage, calculate_pending_download_bytes, check_filter_reason};

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

    fn item() -> TorrentItem {
        TorrentItem {
            rss_name: "rss".to_string(),
            guid: "guid".to_string(),
            title: "title".to_string(),
            link: Some("https://kp.m-team.cc/detail/1".to_string()),
            pub_date: None,
            download_url: "https://kp.m-team.cc/download/1".to_string(),
            version: 1,
            size_bytes: Some(1024),
            seeders: Some(10),
            download_volume_factor: None,
            upload_volume_factor: None,
            minimum_ratio: None,
            minimum_seed_time: None,
        }
    }

    #[test]
    fn rss_pre_filter_keeps_items_with_unknown_free_attrs() {
        let mut task = task();
        task.promotion = "free".to_string();

        let reason = check_filter_reason(&task, &item(), &[], &[], FilterStage::RssPreFilter);

        assert!(reason.is_none());
    }

    #[test]
    fn post_filter_requires_free_attrs_when_free_is_requested() {
        let mut task = task();
        task.promotion = "free".to_string();

        let reason = check_filter_reason(&task, &item(), &[], &[], FilterStage::PostEnhancement);

        assert_eq!(reason.as_deref(), Some("缺少免费属性"));
    }

    #[test]
    fn rss_pre_filter_keeps_items_with_unknown_hr_attrs() {
        let mut task = task();
        task.skip_hit_and_run = true;

        let reason = check_filter_reason(&task, &item(), &[], &[], FilterStage::RssPreFilter);

        assert!(reason.is_none());
    }

    #[test]
    fn post_filter_requires_hr_attrs_when_skip_hr_is_enabled() {
        let mut task = task();
        task.skip_hit_and_run = true;

        let reason = check_filter_reason(&task, &item(), &[], &[], FilterStage::PostEnhancement);

        assert_eq!(reason.as_deref(), Some("缺少H&R属性"));
    }

    #[test]
    fn calculates_pending_download_bytes_from_incomplete_torrents() {
        let torrents = vec![
            TorrentInfo {
                hash: "a".to_string(),
                name: "a".to_string(),
                size: 1000,
                uploaded: 0,
                downloaded: 400,
                upload_speed: 0,
                download_speed: 0,
                ratio: 0.0,
                state: "downloading".to_string(),
                added_on: 0,
                completion_on: 0,
                num_seeds: 0,
                num_leechs: 0,
                save_path: String::new(),
                tags: String::new(),
                category: String::new(),
                time_active: 0,
                last_activity: 0,
            },
            TorrentInfo {
                hash: "b".to_string(),
                name: "b".to_string(),
                size: 2000,
                uploaded: 0,
                downloaded: 2000,
                upload_speed: 0,
                download_speed: 0,
                ratio: 0.0,
                state: "uploading".to_string(),
                added_on: 0,
                completion_on: 1,
                num_seeds: 0,
                num_leechs: 0,
                save_path: String::new(),
                tags: String::new(),
                category: String::new(),
                time_active: 0,
                last_activity: 0,
            },
        ];

        assert_eq!(calculate_pending_download_bytes(&torrents), 600);
    }
}

fn format_size(bytes: Option<u64>) -> String {
    match bytes {
        Some(b) if b >= 1024 * 1024 * 1024 => format!("{:.2} GB", b as f64 / (1024.0 * 1024.0 * 1024.0)),
        Some(b) if b >= 1024 * 1024 => format!("{:.1} MB", b as f64 / (1024.0 * 1024.0)),
        Some(b) => format!("{} B", b),
        None => "?".to_string(),
    }
}

fn calculate_pending_download_bytes(torrents: &[TorrentInfo]) -> u64 {
    torrents
        .iter()
        .map(|torrent| {
            if torrent.completion_on > 0 || torrent.downloaded >= torrent.size {
                return 0;
            }

            (torrent.size - torrent.downloaded).max(0) as u64
        })
        .sum()
}

fn gb_to_bytes(gb: f64) -> u64 {
    if gb <= 0.0 {
        0
    } else {
        (gb * 1024.0 * 1024.0 * 1024.0) as u64
    }
}

fn bytes_to_gb(bytes: u64) -> f64 {
    bytes as f64 / (1024.0 * 1024.0 * 1024.0)
}

/// 将站点属性应用到 RSS 条目上
fn apply_attrs_to_item(item: &mut rss::TorrentItem, attrs: &crate::site::TorrentAttributes) {
    if attrs.download_volume_factor.is_some() {
        item.download_volume_factor = attrs.download_volume_factor;
    }
    if attrs.upload_volume_factor.is_some() {
        item.upload_volume_factor = attrs.upload_volume_factor;
    }
    if attrs.hit_and_run {
        item.minimum_seed_time.get_or_insert(1);
    }
    if attrs.peer_count.is_some() {
        item.seeders = attrs.peer_count;
    }
}

/// 从详情链接中提取末尾的数字 ID（如 "https://kp.m-team.cc/detail/1165802" → "1165802"）
fn extract_torrent_id(detail_url: &str) -> &str {
    detail_url.rsplit('/').find(|s| !s.is_empty()).unwrap_or(detail_url)
}

fn extract_info_hash(torrent_data: &[u8]) -> Option<String> {
    let (start, end) = find_info_dict_range(torrent_data)?;
    Some(hex_encode(&sha1_digest(&torrent_data[start..end])))
}

fn find_info_dict_range(bytes: &[u8]) -> Option<(usize, usize)> {
    let mut index = 0;
    match bytes.get(index)? {
        b'd' => {
            index += 1;
            while bytes.get(index).copied()? != b'e' {
                let (key, next_index) = parse_bencode_bytes(bytes, index)?;
                index = next_index;
                if key == b"info" {
                    let value_start = index;
                    let value_end = parse_bencode_value_end(bytes, index)?;
                    return Some((value_start, value_end));
                }
                index = parse_bencode_value_end(bytes, index)?;
            }
            None
        }
        _ => None,
    }
}

fn parse_bencode_bytes(bytes: &[u8], start: usize) -> Option<(&[u8], usize)> {
    let colon = bytes[start..]
        .iter()
        .position(|byte| *byte == b':')
        .map(|offset| start + offset)?;
    let len = std::str::from_utf8(&bytes[start..colon]).ok()?.parse::<usize>().ok()?;
    let value_start = colon + 1;
    let value_end = value_start.checked_add(len)?;
    Some((bytes.get(value_start..value_end)?, value_end))
}

fn parse_bencode_value_end(bytes: &[u8], start: usize) -> Option<usize> {
    match bytes.get(start).copied()? {
        b'i' => {
            let end = bytes[start + 1..]
                .iter()
                .position(|byte| *byte == b'e')
                .map(|offset| start + 1 + offset)?;
            Some(end + 1)
        }
        b'l' => {
            let mut index = start + 1;
            while bytes.get(index).copied()? != b'e' {
                index = parse_bencode_value_end(bytes, index)?;
            }
            Some(index + 1)
        }
        b'd' => {
            let mut index = start + 1;
            while bytes.get(index).copied()? != b'e' {
                let (_, next_index) = parse_bencode_bytes(bytes, index)?;
                index = parse_bencode_value_end(bytes, next_index)?;
            }
            Some(index + 1)
        }
        b'0'..=b'9' => {
            let (_, end) = parse_bencode_bytes(bytes, start)?;
            Some(end)
        }
        _ => None,
    }
}

fn sha1_digest(data: &[u8]) -> [u8; 20] {
    let mut h0 = 0x6745_2301u32;
    let mut h1 = 0xEFCD_AB89u32;
    let mut h2 = 0x98BA_DCFEu32;
    let mut h3 = 0x1032_5476u32;
    let mut h4 = 0xC3D2_E1F0u32;

    let bit_len = (data.len() as u64) * 8;
    let mut message = data.to_vec();
    message.push(0x80);
    while (message.len() % 64) != 56 {
        message.push(0);
    }
    message.extend_from_slice(&bit_len.to_be_bytes());

    for chunk in message.chunks_exact(64) {
        let mut w = [0u32; 80];
        for (index, word) in w.iter_mut().take(16).enumerate() {
            let offset = index * 4;
            *word = u32::from_be_bytes([
                chunk[offset],
                chunk[offset + 1],
                chunk[offset + 2],
                chunk[offset + 3],
            ]);
        }
        for index in 16..80 {
            w[index] = (w[index - 3] ^ w[index - 8] ^ w[index - 14] ^ w[index - 16]).rotate_left(1);
        }

        let (mut a, mut b, mut c, mut d, mut e) = (h0, h1, h2, h3, h4);
        for (index, word) in w.iter().enumerate() {
            let (f, k) = match index {
                0..=19 => (((b & c) | ((!b) & d)), 0x5A82_7999),
                20..=39 => (b ^ c ^ d, 0x6ED9_EBA1),
                40..=59 => (((b & c) | (b & d) | (c & d)), 0x8F1B_BCDC),
                _ => (b ^ c ^ d, 0xCA62_C1D6),
            };
            let temp = a
                .rotate_left(5)
                .wrapping_add(f)
                .wrapping_add(e)
                .wrapping_add(k)
                .wrapping_add(*word);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = temp;
        }

        h0 = h0.wrapping_add(a);
        h1 = h1.wrapping_add(b);
        h2 = h2.wrapping_add(c);
        h3 = h3.wrapping_add(d);
        h4 = h4.wrapping_add(e);
    }

    let mut digest = [0u8; 20];
    digest[..4].copy_from_slice(&h0.to_be_bytes());
    digest[4..8].copy_from_slice(&h1.to_be_bytes());
    digest[8..12].copy_from_slice(&h2.to_be_bytes());
    digest[12..16].copy_from_slice(&h3.to_be_bytes());
    digest[16..20].copy_from_slice(&h4.to_be_bytes());
    digest
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

async fn prune_expired_detail_attr_cache(now: i64) {
    let cache = detail_attr_cache();
    let mut cache_guard = cache.write().await;
    cache_guard.retain(|_, site_cache| {
        site_cache.retain(|_, entry| now - entry.fetched_at <= DETAIL_ATTR_CACHE_TTL_SECS);
        !site_cache.is_empty()
    });
}

async fn fetch_rss_text(url: &str) -> Result<String, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|e| format!("创建HTTP客户端失败: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("RSS 请求失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("RSS HTTP {}", resp.status()));
    }

    resp.text()
        .await
        .map_err(|e| format!("读取RSS失败: {}", e))
}

async fn fetch_torrent_bytes(url: &str) -> Result<Vec<u8>, String> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60))
        .build()
        .map_err(|e| format!("创建HTTP客户端失败: {}", e))?;

    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| format!("下载种子失败: {}", e))?;

    if !resp.status().is_success() {
        return Err(format!("HTTP {}", resp.status()));
    }

    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(|e| format!("读取种子数据失败: {}", e))
}
