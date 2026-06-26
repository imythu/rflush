use std::sync::Arc;
use std::time::Duration;

use regex::Regex;
use tracing::{debug, error, info, warn};

use crate::db::Database;
use crate::downloader::DownloaderClientPool;
use crate::tag_rule::{TagMatchCriteria, TagRuleRecord};

/// 标签规则调度器，每分钟扫描一次所有 QB 实例的种子并匹配标签
pub struct TagRuleScheduler {
    db: Database,
    pool: Arc<DownloaderClientPool>,
}

impl TagRuleScheduler {
    pub fn new(db: Database, pool: Arc<DownloaderClientPool>) -> Arc<Self> {
        Arc::new(Self { db, pool })
    }

    pub async fn start(&self) {
        // 首次使用默认值，后续每次循环从数据库读取最新配置
        let mut interval_secs = self
            .db
            .get_settings()
            .await
            .map(|s| s.tag_rule_scan_interval_mins.saturating_mul(60).max(60))
            .unwrap_or(420);
        info!("tag rule scheduler started, scanning every {}s", interval_secs);
        let mut interval = tokio::time::interval(Duration::from_secs(interval_secs));
        loop {
            interval.tick().await;
            // 每次循环重新读取间隔配置
            match self.db.get_settings().await {
                Ok(s) => {
                    let new_secs = s.tag_rule_scan_interval_mins.saturating_mul(60).max(60);
                    if new_secs != interval_secs {
                        info!("tag rule scan interval changed: {}s -> {}s", interval_secs, new_secs);
                        interval_secs = new_secs;
                        interval = tokio::time::interval(Duration::from_secs(interval_secs));
                    }
                }
                Err(e) => {
                    warn!("tag scheduler: 读取设置失败，沿用上次间隔: {}", e);
                }
            }
            if let Err(e) = self.run_once().await {
                error!("tag rule scheduler error: {}", e);
            }
        }
    }

    pub async fn run_once(&self) -> Result<(), String> {
        let rules = self
            .db
            .list_enabled_tag_rules()
            .await
            .map_err(|e| format!("加载标签规则失败: {}", e))?;

        if rules.is_empty() {
            debug!("no enabled tag rules, skipping");
            return Ok(());
        }

        let parsed_rules = self.parse_rules(&rules)?;
        debug!("tag scheduler: === 开始扫描 ===");
        debug!("tag scheduler: 已加载 {} 条启用规则", parsed_rules.len());
        for (rule, compiled) in &parsed_rules {
            let ids_desc = match rule.downloader_ids.as_ref().and_then(|s| serde_json::from_str::<Vec<i64>>(s).ok()) {
                None => "所有".to_string(),
                Some(ids) if ids.is_empty() => "无".to_string(),
                Some(ids) => ids.iter().map(|id| id.to_string()).collect::<Vec<_>>().join(","),
            };
            let criteria_desc: Vec<String> = compiled.iter().map(|c| match c {
                CompiledCriteria::Prefix(p) => format!("前缀:{}", p),
                CompiledCriteria::Suffix(s) => format!("后缀:{}", s),
                CompiledCriteria::Contains(c) => format!("包含:{}", c),
                CompiledCriteria::Exact(e) => format!("精确:{}", e),
                CompiledCriteria::Regex(r) => format!("正则:{}", r.as_str()),
            }).collect();
            debug!(
                "tag scheduler:   规则[{}] → 标签 '{}' | 匹配条件: [{}] | 实例: {}",
                rule.name, rule.tag_name, criteria_desc.join(", "), ids_desc
            );
        }

        let downloaders = self
            .db
            .list_downloaders()
            .await
            .map_err(|e| format!("加载下载器列表失败: {}", e))?;

        let mut total_torrents = 0usize;
        let mut total_matched = 0usize;
        let mut total_tagged = 0usize;
        let mut total_already = 0usize;
        let mut total_no_tracker = 0usize;

        for downloader in &downloaders {
            // 检查是否有规则需要此下载器
            let applicable_rules: Vec<_> = parsed_rules.iter().filter(|(rule, _)| {
                match &rule.downloader_ids {
                    Some(ids_str) => {
                        let ids: Vec<i64> = serde_json::from_str(ids_str).unwrap_or_default();
                        ids.contains(&downloader.id)
                    }
                    None => true,
                }
            }).collect();
            if applicable_rules.is_empty() {
                debug!("tag scheduler: 下载器 '{}' 无适用规则，跳过", downloader.name);
                continue;
            }
            debug!(
                "tag scheduler: 下载器 '{}' 有 {} 条适用规则",
                downloader.name, applicable_rules.len()
            );

            let client = match self.pool.get(downloader).await {
                Ok(c) => c,
                Err(e) => {
                    warn!("tag scheduler: 获取 '{}' 客户端失败: {}", downloader.name, e);
                    continue;
                }
            };

            let torrents = match client.list_torrents(None).await {
                Ok(t) => t,
                Err(e) => {
                    warn!("tag scheduler: 列出 '{}' 种子失败: {}", downloader.name, e);
                    continue;
                }
            };

            debug!(
                "tag scheduler: 下载器 '{}' 共 {} 个种子",
                downloader.name, torrents.len()
            );
            total_torrents += torrents.len();

            for torrent in &torrents {
                // 获取该种子的 tracker URL
                let trackers = match client.get_torrent_trackers(&torrent.hash).await {
                    Ok(t) => t,
                    Err(e) => {
                        debug!(
                            "tag scheduler: 种子 '{}' (hash={}...) 获取tracker失败: {}",
                            torrent.name, &torrent.hash[..8.min(torrent.hash.len())], e
                        );
                        continue;
                    }
                };

                // 提取每条 tracker 的域名用于匹配
                let tracker_domains: Vec<(&str, &str)> = trackers
                    .iter()
                    .filter(|u| !u.is_empty() && !u.starts_with("**"))
                    .filter_map(|u| extract_domain(u).map(|d| (u.as_str(), d)))
                    .collect();
                if tracker_domains.is_empty() {
                    total_no_tracker += 1;
                    debug!(
                        "tag scheduler: 种子 '{}' (hash={}...) 无可用tracker域名，跳过",
                        torrent.name, &torrent.hash[..8.min(torrent.hash.len())]
                    );
                    continue;
                }

                let existing_tags: Vec<&str> =
                    torrent.tags.split(',').map(|t| t.trim()).filter(|t| !t.is_empty()).collect();

                for (rule, compiled) in &applicable_rules {
                    // 检查种子是否已有该标签
                    if existing_tags.contains(&rule.tag_name.as_str()) {
                        total_already += 1;
                        debug!(
                            "tag scheduler: 种子 '{}' 已有标签 '{}'，跳过规则 '{}'",
                            torrent.name, rule.tag_name, rule.name
                        );
                        continue;
                    }

                    // 逐条 tracker 域名匹配
                    let mut matched = false;
                    for (raw_url, domain) in &tracker_domains {
                        match match_tracker_detailed(domain, compiled) {
                            Some((criteria_idx, criteria)) => {
                                total_matched += 1;
                                debug!(
                                    "tag scheduler: ✓ 种子 '{}' | 域名 '{}' (tracker: {}) | 规则 '{}' 第{}条命中 → 标签 '{}'",
                                    torrent.name, domain, raw_url, rule.name, criteria_idx + 1, rule.tag_name
                                );
                                debug!(
                                    "tag scheduler:   匹配详情: {} ~ '{}'",
                                    criteria_desc(criteria), domain
                                );
                                matched = true;

                                // 执行打标签
                                match client
                                    .add_torrent_tags(
                                        vec![torrent.hash.clone()],
                                        vec![rule.tag_name.clone()],
                                    )
                                    .await
                                {
                                    Ok(()) => {
                                        total_tagged += 1;
                                        info!(
                                            "tag scheduler: 已为种子 '{}' (hash={}...) 添加标签 '{}'",
                                            torrent.name, &torrent.hash[..8.min(torrent.hash.len())], rule.tag_name
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            "tag scheduler: 为种子 '{}' 添加标签 '{}' 失败: {}",
                                            torrent.name, rule.tag_name, e
                                        );
                                    }
                                }
                                break; // 一条 tracker 命中就够了
                            }
                            None => {
                                debug!(
                                    "tag scheduler:   种子 '{}' | 域名 '{}' | 规则 '{}' 未命中任何条件",
                                    torrent.name, domain, rule.name
                                );
                            }
                        }
                    }
                    if !matched {
                        debug!(
                            "tag scheduler: ✗ 种子 '{}' | 规则 '{}' 全部域名均未匹配 (共{}条): [{}]",
                            torrent.name, rule.name, tracker_domains.len(),
                            tracker_domains.iter().map(|(_, d)| *d).collect::<Vec<_>>().join(", ")
                        );
                    }
                }
            }
        }

        debug!(
            "tag scheduler: === 扫描完成 === 种子:{} 已有标签跳过:{} 无tracker跳过:{} 命中:{} 标签成功:{}",
            total_torrents, total_already, total_no_tracker, total_matched, total_tagged
        );

        // 统计每条规则的标签种子数和总体积，写入数据库
        for (rule, _) in &parsed_rules {
            let target_ids: Option<Vec<i64>> = rule
                .downloader_ids
                .as_ref()
                .and_then(|s| serde_json::from_str(s).ok());

            let mut count: i64 = 0;
            let mut total_size: i64 = 0;
            for downloader in &downloaders {
                if let Some(ref ids) = target_ids {
                    if !ids.contains(&downloader.id) {
                        continue;
                    }
                }
                if let Ok(client) = self.pool.get(downloader).await {
                    if let Ok(torrents) = client.list_torrents(Some(&rule.tag_name)).await {
                        count += torrents.len() as i64;
                        total_size += torrents.iter().map(|t| t.size).sum::<i64>();
                    }
                }
            }
            if let Err(e) = self.db.update_tag_rule_stats(rule.id, count, total_size).await {
                warn!("tag scheduler: 更新规则 '{}' 统计失败: {}", rule.name, e);
            } else {
                debug!(
                    "tag scheduler: 规则 '{}' 统计已更新: 种子数={}, 总体积={}B",
                    rule.name, count, total_size
                );
            }
        }

        Ok(())
    }

    /// 解析规则中的匹配条件，预编译正则表达式
    fn parse_rules(
        &self,
        rules: &[TagRuleRecord],
    ) -> Result<Vec<(TagRuleRecord, Vec<CompiledCriteria>)>, String> {
        let mut result = Vec::new();
        for rule in rules {
            let criteria: Vec<TagMatchCriteria> =
                serde_json::from_str(&rule.match_rules)
                    .map_err(|e| format!("解析规则 '{}' 的匹配条件失败: {}", rule.name, e))?;
            let compiled: Vec<CompiledCriteria> = criteria
                .into_iter()
                .map(|c| compile_criteria(&c))
                .collect::<Result<Vec<_>, _>>()?;
            result.push((rule.clone(), compiled));
        }
        Ok(result)
    }
}

/// 编译后的匹配条件
enum CompiledCriteria {
    Prefix(String),
    Suffix(String),
    Contains(String),
    Exact(String),
    Regex(Regex),
}

fn compile_criteria(criteria: &TagMatchCriteria) -> Result<CompiledCriteria, String> {
    match criteria.match_type.as_str() {
        "prefix" => Ok(CompiledCriteria::Prefix(criteria.pattern.clone())),
        "suffix" => Ok(CompiledCriteria::Suffix(criteria.pattern.clone())),
        "contains" => Ok(CompiledCriteria::Contains(criteria.pattern.clone())),
        "exact" => Ok(CompiledCriteria::Exact(criteria.pattern.clone())),
        "regex" => {
            let re = Regex::new(&criteria.pattern)
                .map_err(|e| format!("无效的正则表达式 '{}': {}", criteria.pattern, e))?;
            Ok(CompiledCriteria::Regex(re))
        }
        other => Err(format!("未知的匹配类型: {}", other)),
    }
}

/// 从 tracker URL 中提取域名，例如 "https://kp.m-team.xyz/announce" → "kp.m-team.xyz"
fn extract_domain(url: &str) -> Option<&str> {
    let after_scheme = if let Some(pos) = url.find("://") {
        &url[pos + 3..]
    } else {
        url
    };
    let end = after_scheme
        .find('/')
        .or_else(|| after_scheme.find('?'))
        .or_else(|| after_scheme.find('#'))
        .unwrap_or(after_scheme.len());
    let host = &after_scheme[..end];
    // 去掉端口号
    let domain = host.split(':').next().unwrap_or(host);
    if domain.is_empty() { None } else { Some(domain) }
}

fn match_tracker_detailed<'a>(domain: &str, criteria: &'a [CompiledCriteria]) -> Option<(usize, &'a CompiledCriteria)> {
    if criteria.is_empty() {
        return None;
    }
    criteria.iter().enumerate().find_map(|(i, c)| {
        let hit = match c {
            CompiledCriteria::Prefix(prefix) => domain.starts_with(prefix),
            CompiledCriteria::Suffix(suffix) => domain.ends_with(suffix),
            CompiledCriteria::Contains(needle) => domain.contains(needle),
            CompiledCriteria::Exact(expected) => domain == expected,
            CompiledCriteria::Regex(re) => re.is_match(domain),
        };
        if hit { Some((i, c)) } else { None }
    })
}

fn criteria_desc(c: &CompiledCriteria) -> String {
    match c {
        CompiledCriteria::Prefix(p) => format!("前缀匹配 '{}'", p),
        CompiledCriteria::Suffix(s) => format!("后缀匹配 '{}'", s),
        CompiledCriteria::Contains(n) => format!("包含 '{}'", n),
        CompiledCriteria::Exact(e) => format!("精确匹配 '{}'", e),
        CompiledCriteria::Regex(r) => format!("正则 '{}'", r.as_str()),
    }
}
