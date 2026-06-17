use tracing::{debug, info, warn};

use crate::brush::BrushTaskRecord;
use crate::db::Database;
use crate::net::http::AppHttpClient;
use crate::rss;
use crate::site::u2_shoutbox;

/// 检测 URL 是否为 U2 站点 (u2.dmhy.org) 的 shoutbox 请求。
pub fn is_u2_shoutbox_url(url: &str) -> bool {
    url.contains("u2.dmhy.org") && url.contains("shoutbox.php")
}

/// 构造 U2 请求的公共头（模拟浏览器），外加 Cookie 和 Referer。
fn u2_headers<'a>(
    cookie: &'a str,
    referer: &'a str,
    buf: &'a mut Vec<(&'a str, &'a str)>,
) -> &'a [(&'a str, &'a str)] {
    buf.clear();
    buf.push(("Cookie", cookie));
    buf.push(("Referer", referer));
    buf.push(("User-Agent", "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/148.0.0.0 Safari/537.36"));
    buf.push(("Accept", "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7"));
    buf.push(("Accept-Language", "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7,zh-TW;q=0.6"));
    buf.push(("DNT", "1"));
    buf.push(("Upgrade-Insecure-Requests", "1"));
    buf.push(("Sec-Fetch-Dest", "iframe"));
    buf.push(("Sec-Fetch-Mode", "navigate"));
    buf.push(("Sec-Fetch-Site", "same-origin"));
    buf.push(("Sec-Fetch-User", "?1"));
    buf.push(("sec-ch-ua", "\"Chromium\";v=\"148\", \"Google Chrome\";v=\"148\", \"Not/A)Brand\";v=\"99\""));
    buf.push(("sec-ch-ua-mobile", "?0"));
    buf.push(("sec-ch-ua-platform", "\"Windows\""));
    buf.push(("Cache-Control", "max-age=0"));
    buf.as_slice()
}

/// 从站点配置中提取 U2 Cookie。
pub async fn get_u2_site_cookie(
    task: &BrushTaskRecord,
    db: &Database,
) -> Result<String, String> {
    let site_id = task
        .site_id
        .ok_or_else(|| "U2 shoutbox 需要关联站点以获取 Cookie".to_string())?;

    let site = db
        .get_site(site_id)
        .await
        .map_err(|e| format!("加载站点失败: {}", e))?
        .ok_or_else(|| format!("站点不存在: site_id={}", site_id))?;

    let auth: crate::site::SiteAuth =
        serde_json::from_str(&site.auth_config).map_err(|e| {
            format!("解析站点认证配置失败 (site_id={site_id}): {e}")
        })?;

    Ok(match &auth {
        crate::site::SiteAuth::Cookie { cookie } => cookie.clone(),
        crate::site::SiteAuth::CookiePasskey { cookie, .. } => cookie.clone(),
        _ => {
            return Err(format!(
                "U2 shoutbox 需要 Cookie 认证，当前认证类型不支持 (site_id={site_id})"
            ));
        }
    })
}

/// 使用站点 Cookie 预热 U2 会话，访问 /index.php 保持登录状态。
async fn warmup_session(
    task: &BrushTaskRecord,
    cookie: &str,
    http: &AppHttpClient,
) {
    let url = "https://u2.dmhy.org/index.php";
    let referer = "https://u2.dmhy.org/index.php";
    let mut buf = Vec::new();
    let headers = u2_headers(cookie, referer, &mut buf);

    match http.get_with_headers("u2-warmup", url, headers).await {
        Ok(resp) if resp.status.is_success() => {
            debug!("[刷流][{}] U2 会话预热成功", task.name);
        }
        Ok(resp) => {
            warn!(
                "[刷流][{}] U2 会话预热 HTTP {}",
                task.name, resp.status.as_u16()
            );
        }
        Err(e) => {
            warn!("[刷流][{}] U2 会话预热失败: {}", task.name, e);
        }
    }
}

/// 使用站点 Cookie 拉取 U2 shoutbox HTML。
pub async fn fetch_shoutbox_html(
    task: &BrushTaskRecord,
    db: &Database,
    http: &AppHttpClient,
) -> Result<String, String> {
    let cookie = get_u2_site_cookie(task, db).await?;

    // 预热会话：先访问 /index.php 保持 Cookie 有效
    warmup_session(task, &cookie, http).await;

    info!(
        "[刷流][{}] 使用站点 Cookie 拉取 U2 shoutbox: {}",
        task.name, task.rss_url
    );

    let referer = task.rss_url.as_str();
    let mut buf = Vec::new();
    let headers = u2_headers(&cookie, referer, &mut buf);

    let resp = http
        .get_with_headers("u2-shoutbox", &task.rss_url, headers)
        .await
        .map_err(|e| format!("U2 shoutbox 请求失败: {}", e))?;

    if !resp.status.is_success() {
        return Err(format!("U2 shoutbox HTTP {}", resp.status));
    }

    String::from_utf8(resp.body.to_vec()).map_err(|_| "U2 shoutbox 非 UTF-8 响应".to_string())
}

/// 使用 U2 shoutbox 解析器解析 HTML 并构造 `FeedSnapshot`。
pub fn parse_shoutbox_snapshot(
    html: &str,
    task_name: &str,
) -> Result<rss::FeedSnapshot, String> {
    let mut items = u2_shoutbox::parse_shoutbox(html)
        .map_err(|e| format!("U2 shoutbox 解析失败: {}", e))?;

    if items.is_empty() {
        let preview: String = html.chars().take(500).collect();
        warn!(
            "[刷流][{}] U2 shoutbox 解析到 0 条有效条目，响应预览:\n{}",
            task_name, preview
        );
    }

    for item in &mut items {
        item.rss_name = task_name.to_string();
        item.version = 1;
    }

    let mut item_map = std::collections::HashMap::with_capacity(items.len());
    for item in items {
        let guid = item.guid.clone();
        item_map.insert(guid, item);
    }

    Ok(rss::FeedSnapshot {
        version: 1,
        items: item_map,
    })
}

/// 拉取 U2 种子详情页并解析大小、做种人数、免费剩余时间，填充到 item 中。
pub async fn enrich_item(
    task: &BrushTaskRecord,
    db: &Database,
    http: &AppHttpClient,
    item: &mut rss::TorrentItem,
) {
    let detail_url = format!("https://u2.dmhy.org/details.php?id={}&hit=1", item.guid);
    let cookie = match get_u2_site_cookie(task, db).await {
        Ok(c) => c,
        Err(e) => {
            warn!("[刷流][{}] 获取 U2 cookie 失败: {}", task.name, e);
            return;
        }
    };

    debug!("[刷流][{}] 拉取 U2 种子详情: guid={}", task.name, item.guid);

    let referer = "https://u2.dmhy.org/torrents.php";
    let mut buf = Vec::new();
    let headers = u2_headers(&cookie, referer, &mut buf);

    let resp = match http
        .get_with_headers("u2-detail", &detail_url, headers)
        .await
    {
        Ok(r) => r,
        Err(e) => {
            warn!(
                "[刷流][{}] U2 详情请求失败: guid={} err={}",
                task.name, item.guid, e
            );
            return;
        }
    };

    if !resp.status.is_success() {
        warn!(
            "[刷流][{}] U2 详情 HTTP {}: guid={}",
            task.name,
            resp.status.as_u16(),
            item.guid
        );
        return;
    }

    let html = match String::from_utf8(resp.body.to_vec()) {
        Ok(s) => s,
        Err(_) => return,
    };

    let Some(detail) = u2_shoutbox::parse_detail_page(&html) else {
        return;
    };

    if let Some(size) = detail.size_bytes {
        item.size_bytes = Some(size);
    }
    if let Some(seeders) = detail.seeders {
        item.seeders = Some(seeders);
    }
    if let Some(downloaders) = detail.downloaders {
        item.leechers = Some(downloaders);
    }
    if let Some(ts) = detail.free_end_timestamp {
        item.free_end_timestamp = Some(ts);
    }
    if item.minimum_seed_time.is_none() {
        item.minimum_seed_time = Some(0);
    }

    // ── 请求 viewpeerlist.php 获取精准做种/下载数 ─────────────────
    {
        let peer_list_url = format!(
            "https://u2.dmhy.org/viewpeerlist.php?id={}",
            item.guid
        );
        let peer_referer = format!(
            "https://u2.dmhy.org/details.php?id={}&hits=1",
            item.guid
        );

        let mut buf2 = Vec::new();
        let peer_headers = u2_headers(&cookie, &peer_referer, &mut buf2);

        match http
            .get_with_headers("u2-peerlist", &peer_list_url, peer_headers)
            .await
        {
            Ok(resp) if resp.status.is_success() => {
                if let Ok(html) = String::from_utf8(resp.body.to_vec()) {
                    if let Some(peer_info) = u2_shoutbox::parse_peer_list_page(&html) {
                        debug!(
                            "[刷流][{}] peer list 解析: guid={} 真实做种={} 下载={}",
                            task.name, item.guid, peer_info.seeders, peer_info.downloaders
                        );
                        item.seeders = Some(peer_info.seeders);
                        item.leechers = Some(peer_info.downloaders);
                    }
                }
            }
            Err(e) => {
                warn!(
                    "[刷流][{}] U2 peer list 请求失败: guid={} err={}",
                    task.name, item.guid, e
                );
            }
            _ => {
                // 非 200 状态码，静默回退到详情页数值
            }
        }
    }

    // info 级别打印详情完成
    let free_info = match (item.free_end_timestamp, item.free_elapsed_seconds) {
        (Some(ts), Some(elapsed)) => {
            let remaining = (ts - chrono::Utc::now().timestamp()).max(0) as u64;
            format!(
                "剩余{}(已持续{})",
                u2_shoutbox::human_duration(remaining),
                u2_shoutbox::human_duration(elapsed)
            )
        }
        (Some(ts), None) => {
            let remaining = (ts - chrono::Utc::now().timestamp()).max(0) as u64;
            format!("剩余{}", u2_shoutbox::human_duration(remaining))
        }
        (None, _) => "无免费".to_string(),
    };
    let size_info = item
        .size_bytes
        .map(|b| {
            let gb = b as f64 / (1024.0 * 1024.0 * 1024.0);
            format!("{:.2} GiB", gb)
        })
        .unwrap_or_else(|| "?".to_string());
    let seeders = item
        .seeders
        .map(|s| s.to_string())
        .unwrap_or_else(|| "?".to_string());
    let downloaders = item
        .leechers
        .map(|d| d.to_string())
        .unwrap_or_else(|| "?".to_string());

    info!(
        "[刷流][{}] U2种子详情: {} id={} 大小={} 免费={} 做种={} 下载={}",
        task.name, item.title, item.guid, size_info, free_info, seeders, downloaders,
    );
}

/// 使用站点 Cookie 下载 U2 种子文件，返回 .torrent 文件的字节内容。
pub async fn download_torrent(
    task: &BrushTaskRecord,
    db: &Database,
    http: &AppHttpClient,
    download_url: &str,
) -> Result<Vec<u8>, String> {
    let cookie = get_u2_site_cookie(task, db).await?;

    info!(
        "[刷流][{}] 使用站点 Cookie 下载 U2 种子: {}",
        task.name, download_url
    );

    let referer = "https://u2.dmhy.org/torrents.php";
    let mut buf = Vec::new();
    let headers = u2_headers(&cookie, referer, &mut buf);

    let resp = http
        .get_with_headers("u2-torrent", download_url, headers)
        .await
        .map_err(|e| format!("U2 种子下载请求失败: {}", e))?;

    if !resp.status.is_success() {
        return Err(format!("U2 种子下载 HTTP {}", resp.status));
    }

    Ok(resp.body.to_vec())
}
