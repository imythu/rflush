use chrono::TimeZone;
use scraper::{Html, Selector};

use crate::rss::TorrentItem;

/// 最大允许的消息时长：超过 12 小时的条目将被丢弃。
const MAX_AGE_SECONDS: u64 = 12 * 3600;

/// U2 种子详情页提取结果。
#[derive(Debug, Default)]
pub struct U2DetailInfo {
    pub size_bytes: Option<u64>,
    pub seeders: Option<i32>,
    pub downloaders: Option<i32>,
    pub free_end_timestamp: Option<i64>,
}

/// Peer list 页面 (viewpeerlist.php) 解析结果：经过过滤的真实做种数和下载数。
#[derive(Debug)]
pub struct PeerListInfo {
    pub seeders: i32,
    pub downloaders: i32,
}

#[derive(Debug, thiserror::Error)]
pub enum U2ShoutboxParseError {
    #[error("HTML selector error: {0}")]
    Selector(String),
}

/// 解析 U2 shoutbox HTML，提取所有 下载0.00 且发布时间不超过 12 小时的魔法条目。
///
/// 返回的 `TorrentItem` 中 `rss_name` 和 `version` 字段由调用方填充。
pub fn parse_shoutbox(html: &str) -> Result<Vec<TorrentItem>, U2ShoutboxParseError> {
    let document = Html::parse_document(html);

    let seed_selector = Selector::parse("a[href^=\"details.php?id=\"]")
        .map_err(|e| U2ShoutboxParseError::Selector(e.to_string()))?;

    let time_selector =
        Selector::parse("time").map_err(|e| U2ShoutboxParseError::Selector(e.to_string()))?;

    let div_selector =
        Selector::parse("div").map_err(|e| U2ShoutboxParseError::Selector(e.to_string()))?;

    let mut items = Vec::new();

    for div in document.select(&div_selector) {
        let div_html = div.inner_html();
        // 快速过滤：必须同时包含种子详情链接和魔法促销链接
        if !div_html.contains("details.php?id=")
            || !div_html.contains("promotion.php?action=detail")
        {
            continue;
        }

        // 解析发布时间，丢弃超过 12 小时的条目
        let mut elapsed: u64 = 0;
        if let Some(time_el) = div.select(&time_selector).next() {
            let time_text: String = time_el.text().collect();
            elapsed = parse_elapsed_time(&time_text);
            if elapsed > MAX_AGE_SECONDS {
                continue;
            }
        }

        // 提取种子上传/下载系数
        let div_text: String = div.text().collect();
        let (upload_factor, download_factor) = parse_upload_download(&div_text);

        // 只保留下载为 0.00 的条目
        if (download_factor - 0.0).abs() >= f64::EPSILON {
            continue;
        }

        // 解析免费总时长，计算剩余时长
        let free_end_timestamp = parse_free_duration(&div_text).and_then(|total_free| {
            let remaining = total_free.saturating_sub(elapsed);
            if remaining > 0 {
                Some(chrono::Utc::now().timestamp() + remaining as i64)
            } else {
                None
            }
        });

        // 提取种子链接信息
        let Some(seed_link) = div.select(&seed_selector).next() else {
            continue;
        };
        let href = seed_link.value().attr("href").unwrap_or("");
        let Some(seed_id) = extract_seed_id(href) else {
            continue;
        };
        let seed_name: String = seed_link.text().collect();
        let seed_name = seed_name.trim().to_string();
        if seed_name.is_empty() {
            continue;
        }

        items.push(TorrentItem {
            rss_name: String::new(),
            guid: seed_id.clone(),
            title: seed_name,
            link: None,
            pub_date: None,
            download_url: format!("https://u2.dmhy.org/download.php?id={id}", id = seed_id),
            version: 0,
            size_bytes: None,
            seeders: None,
            leechers: None,
            free_end_timestamp,
            free_elapsed_seconds: Some(elapsed),
            download_volume_factor: Some(0.0),
            upload_volume_factor: Some(upload_factor),
            minimum_ratio: None,
            minimum_seed_time: None,
        });
    }

    Ok(items)
}

/// 解析相对时间文本（如 "4分钟54秒前"、"1小时7分钟前"），返回秒数。
fn parse_elapsed_time(text: &str) -> u64 {
    // 去除 soft hyphen (U+00AD)
    let cleaned: String = text.chars().filter(|c| *c != '\u{00AD}').collect();
    let s = cleaned.trim();

    // 尝试匹配 "X小时Y分钟前"
    if let Some(result) = parse_two_part(&s, "小时", "分钟", 3600, 60) {
        return result;
    }
    // 尝试匹配 "X分钟Y秒前"
    if let Some(result) = parse_two_part(&s, "分钟", "秒", 60, 1) {
        return result;
    }
    // 尝试匹配 "X小时前"
    if let Some(result) = parse_single_unit(&s, "小时", 3600) {
        return result;
    }
    // 尝试匹配 "X分钟前"
    if let Some(result) = parse_single_unit(&s, "分钟", 60) {
        return result;
    }
    // 尝试匹配 "X秒前"
    if let Some(result) = parse_single_unit(&s, "秒", 1) {
        return result;
    }

    // 无法解析时间时默认不过滤（返回 0）
    0
}

/// 解析 "X<unit1>Y<unit2>前" 格式。
fn parse_two_part(text: &str, unit1: &str, unit2: &str, mult1: u64, mult2: u64) -> Option<u64> {
    let pos1 = text.find(unit1)?;
    let num1: u64 = text[..pos1]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;

    let after_unit1 = &text[pos1 + unit1.len()..];
    let pos2 = after_unit1.find(unit2)?;
    let num2: u64 = after_unit1[..pos2]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;

    Some(num1 * mult1 + num2 * mult2)
}

/// 解析 "X<unit>前" 格式。
fn parse_single_unit(text: &str, unit: &str, multiplier: u64) -> Option<u64> {
    let pos = text.find(unit)?;
    let num: u64 = text[..pos]
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()?;
    Some(num * multiplier)
}

/// 解析 U2 种子详情页，提取大小、做种人数、免费剩余时间。
pub fn parse_detail_page(html: &str) -> Option<U2DetailInfo> {
    let document = Html::parse_document(html);

    let mut info = U2DetailInfo::default();

    // 大小: <b>大小:</b>&nbsp;82.153 GiB
    info.size_bytes = parse_detail_size(&document);

    // 做种者: <b>3个做种者</b>，下载者: <b>74个下载者</b>
    let (seeders, downloaders) = parse_detail_peercount(&document);
    info.seeders = seeders;
    info.downloaders = downloaders;

    // 免费剩余时间: <time title="2026-06-16 21:30:24"> 在流量优惠行内
    info.free_end_timestamp = parse_detail_free_end(&document);

    Some(info)
}

fn parse_detail_size(document: &Html) -> Option<u64> {
    // 选择包含 "大小:" 的 <td> 元素，从中提取大小文本
    let td_selector = Selector::parse("td.rowfollow").ok()?;
    for td in document.select(&td_selector) {
        let text: String = td.text().collect();
        if let Some(pos) = text.find("大小:") {
            let after = text[pos + "大小:".len()..].trim().to_string();
            // 提取 "82.153 GiB" 部分（到下一个空格或标签文本边界）
            if let Some(size_str) = after.split_whitespace().next().and_then(|num| {
                // 找到下一个词（单位）
                let rest = &after[num.len()..].trim();
                let unit = rest.split_whitespace().next().unwrap_or("");
                Some(format!("{} {}", num, unit))
            }) {
                return parse_human_size(&size_str);
            }
        }
    }
    None
}

fn parse_detail_peercount(document: &Html) -> (Option<i32>, Option<i32>) {
    let selector = Selector::parse("#peercount").ok();
    let selector = match selector {
        Some(s) => s,
        None => return (None, None),
    };
    let peercount = match document.select(&selector).next() {
        Some(el) => el,
        None => return (None, None),
    };
    let text: String = peercount.text().collect();
    // 格式: "3个做种者 | 74个下载者"
    let seeders = parse_peer_count(&text, "个做种者");
    let downloaders = parse_peer_count(&text, "个下载者");
    (seeders, downloaders)
}

fn parse_peer_count(text: &str, marker: &str) -> Option<i32> {
    let pos = text.find(marker)?;
    let before = &text[..pos];
    let num: String = before
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect::<String>()
        .chars()
        .rev()
        .collect();
    num.parse().ok()
}

// ── viewpeerlist.php 解析 ───────────────────────────────────────────────

/// 解析 viewpeerlist.php 页面，按 JS 过滤逻辑返回真实做种数和下载数。
///
/// 过滤规则：
/// 1. 空闲时间 > 45 分钟 → 淘汰
/// 2. 连接 > 30 分钟 且 上传 < 1 MiB 且 平均速度 < 10 B/s → 淘汰（双差生）
pub fn parse_peer_list_page(html: &str) -> Option<PeerListInfo> {
    // 1. 查找做种者表格
    let (seeder_table_html, _total_seeders) = find_table_after_marker(html, "做种者</b>")?;

    // 2. 解析做种者表格并过滤
    let real_seeders = parse_seeder_table(seeder_table_html);

    // 3. 查找下载者数量
    let downloaders = extract_count_from_b_tag(html, "下载者</b>");

    Some(PeerListInfo {
        seeders: real_seeders,
        downloaders,
    })
}

/// 在 HTML 中查找 marker 所在的 `<b>` 标签，提取其前的数字，并返回紧跟的 `<table>` 内容。
fn find_table_after_marker<'a>(html: &'a str, marker: &str) -> Option<(&'a str, i32)> {
    let marker_pos = html.find(marker)?;

    // 提取 <b> 中的数字
    let _total = extract_count_from_b_slice(html, marker, marker_pos);

    // 查找 </b> 后的下一个 <table
    let after_b = marker_pos + marker.len();
    let table_start = html[after_b..].find("<table")?;
    let table_html_start = after_b + table_start;
    let table_end = html[table_html_start..].find("</table>")? + "</table>".len();
    let table_html = &html[table_html_start..table_html_start + table_end];

    Some((table_html, _total))
}

/// 从 `<b>N 下载者</b>` 中提取数字 N。
fn extract_count_from_b_tag(html: &str, marker: &str) -> i32 {
    let pos = match html.find(marker) {
        Some(p) => p,
        None => return 0,
    };
    extract_count_from_b_slice(html, marker, pos)
}

fn extract_count_from_b_slice(html: &str, marker: &str, marker_pos: usize) -> i32 {
    let before = &html[..marker_pos];
    let b_start = match before.rfind("<b>") {
        Some(p) => p + 3,
        None => return 0,
    };
    let b_content = &html[b_start..marker_pos + marker.len() - "</b>".len()];
    b_content
        .chars()
        .filter(|c| c.is_ascii_digit())
        .collect::<String>()
        .parse()
        .unwrap_or(0)
}

/// 解析做种者表格 HTML 片段，返回过滤后的真实做种数。
fn parse_seeder_table(table_html: &str) -> i32 {
    use scraper::ElementRef;

    let fragment = Html::parse_fragment(table_html);
    let tr_selector = match Selector::parse("tr") {
        Ok(s) => s,
        Err(_) => return 0,
    };
    let td_selector = match Selector::parse("td") {
        Ok(s) => s,
        Err(_) => return 0,
    };

    let mut real_count = 0;

    for (i, tr) in fragment.select(&tr_selector).enumerate() {
        if i == 0 {
            continue; // 跳过表头
        }

        let cells: Vec<ElementRef> = tr.select(&td_selector).collect();
        if cells.len() < 10 {
            continue;
        }

        let upload_text = clean_element_text(&cells[1]);
        let avg_speed_text = clean_element_text(&cells[2]);
        let connect_text = clean_element_text(&cells[7]);
        let idle_text = clean_element_text(&cells[8]);

        let idle_secs = parse_idle_time_to_secs(&idle_text);

        // 规则 1: 空闲 > 45 分钟 (2700 秒)
        if idle_secs > 2700 {
            continue;
        }

        let upload_bytes = parse_human_size(&upload_text).unwrap_or(0);
        let avg_speed = parse_speed_to_bytes(&avg_speed_text);
        let connect_secs = parse_connect_time_to_secs(&connect_text);

        // 规则 2: 双差生 — 连接 > 30min 且上传 < 1MiB 且速度 < 10 B/s
        let is_useless = connect_secs > 1800 && upload_bytes < 1048576 && avg_speed < 10;

        if !is_useless {
            real_count += 1;
        }
    }

    real_count
}

/// 从 scraper ElementRef 提取文本，过滤软连字符 (U+00AD)。
fn clean_element_text(el: &scraper::ElementRef) -> String {
    el.text()
        .collect::<String>()
        .chars()
        .filter(|c| *c != '\u{00AD}')
        .collect::<String>()
        .trim()
        .to_string()
}

/// 解析速度字符串（如 "142.609 KiB/s"）为 B/s。
fn parse_speed_to_bytes(s: &str) -> u64 {
    let s = s.trim();
    if s.is_empty() || s == "N/A" || s == "---" {
        return 0;
    }
    let without_per_sec = s.strip_suffix("/s").unwrap_or(s).trim();
    parse_human_size(without_per_sec).unwrap_or(0)
}

/// 解析空闲时间 "HH:MM:SS" 为秒数。
fn parse_idle_time_to_secs(s: &str) -> u64 {
    let parts: Vec<&str> = s.trim().split(':').collect();
    if parts.len() != 3 {
        return 0;
    }
    let h: u64 = parts[0].parse().unwrap_or(0);
    let m: u64 = parts[1].parse().unwrap_or(0);
    let sec: u64 = parts[2].parse().unwrap_or(0);
    h * 3600 + m * 60 + sec
}

/// 解析连接时间（"X天 HH:MM:SS" 或 "HH:MM:SS"）为秒数。
fn parse_connect_time_to_secs(s: &str) -> u64 {
    let s = s.trim();

    let (days, time_part) = if let Some(pos) = s.find('天') {
        let days_str = &s[..pos];
        let days: u64 = days_str
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .unwrap_or(0);
        // 跳过 '天' 和可能的空白
        let after = s[pos + '天'.len_utf8()..].trim();
        (days, after)
    } else {
        (0, s)
    };

    let parts: Vec<&str> = time_part.split(':').collect();
    let h: u64 = parts.first().and_then(|s| s.parse().ok()).unwrap_or(0);
    let m: u64 = parts.get(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    let sec: u64 = parts.get(2).and_then(|s| s.parse().ok()).unwrap_or(0);

    days * 86400 + h * 3600 + m * 60 + sec
}

fn parse_detail_free_end(document: &Html) -> Option<i64> {
    let td_selector = Selector::parse("td.rowfollow").ok()?;
    let pro_free_selector = Selector::parse("img.pro_free").ok()?;
    let time_selector = Selector::parse("time").ok()?;

    // 只在包含 pro_free 图标的 <td> 内查找免费结束时间，避免误匹配发布时间
    for td in document.select(&td_selector) {
        if td.select(&pro_free_selector).next().is_none() {
            continue;
        }
        for time_el in td.select(&time_selector) {
            if let Some(title) = time_el.value().attr("title") {
                if let Some(ts) = parse_datetime_to_timestamp(title) {
                    return Some(ts);
                }
            }
        }
    }
    None
}

/// 解析 "YYYY-MM-DD HH:MM:SS" 为 Unix 时间戳 (UTC+8)。
fn parse_datetime_to_timestamp(s: &str) -> Option<i64> {
    let parts: Vec<&str> = s.split(&['-', ' ', ':']).collect();
    if parts.len() != 6 {
        return None;
    }
    let year: i32 = parts[0].parse().ok()?;
    let month: u32 = parts[1].parse().ok()?;
    let day: u32 = parts[2].parse().ok()?;
    let hour: u32 = parts[3].parse().ok()?;
    let min: u32 = parts[4].parse().ok()?;
    let sec: u32 = parts[5].parse().ok()?;

    // U2 使用北京时间 (UTC+8)
    let offset = chrono::FixedOffset::east_opt(8 * 3600)?;
    let dt = offset
        .with_ymd_and_hms(year, month, day, hour, min, sec)
        .single()?;
    Some(dt.timestamp())
}

/// 从 "82.153 GiB" / "512 MB" 等文本解析字节数。
fn parse_human_size(s: &str) -> Option<u64> {
    let s = s.trim();
    // 提取数字部分
    let num_end = s
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(s.len());
    let num: f64 = s[..num_end].parse().ok()?;
    let unit = s[num_end..].trim().to_ascii_uppercase();

    let multiplier = match unit.as_str() {
        "TIB" | "TB" => 1024u64 * 1024 * 1024 * 1024,
        "GIB" | "GB" => 1024 * 1024 * 1024,
        "MIB" | "MB" => 1024 * 1024,
        "KIB" | "KB" => 1024,
        _ => 1,
    };

    Some((num * multiplier as f64) as u64)
}

/// 从形如 "持续1天0小时" 的文本中解析总免费时长（秒）。
/// 支持不同单位组合：0天12小时、5天0小时、0天30分钟 等。
pub fn parse_free_duration(text: &str) -> Option<u64> {
    let pos = text.find("持续")?;
    let after = &text[pos + "持续".len()..];

    let mut total = 0u64;
    let mut found = false;

    // 尝试匹配 X天
    if let Some(day_pos) = after.find("天") {
        let num: u64 = after[..day_pos]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?;
        total += num * 86400;
        found = true;

        // 继续匹配 Y小时
        let rest = &after[day_pos + "天".len()..];
        if let Some(hour_pos) = rest.find("小时") {
            if let Ok(num) = rest[..hour_pos]
                .chars()
                .filter(|c| c.is_ascii_digit())
                .collect::<String>()
                .parse::<u64>()
            {
                total += num * 3600;
            }
        }
    } else if let Some(hour_pos) = after.find("小时") {
        let num: u64 = after[..hour_pos]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?;
        total += num * 3600;
        found = true;
    } else if let Some(min_pos) = after.find("分钟") {
        let num: u64 = after[..min_pos]
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()?;
        total += num * 60;
        found = true;
    }

    if found { Some(total) } else { None }
}

/// 将秒数格式化为人类可读的中文时长。
pub fn human_duration(secs: u64) -> String {
    if secs == 0 {
        return "0秒".to_string();
    }
    let days = secs / 86400;
    let hours = (secs % 86400) / 3600;
    let mins = (secs % 3600) / 60;
    let secs = secs % 60;

    let mut parts = Vec::new();
    if days > 0 {
        parts.push(format!("{}天", days));
    }
    if hours > 0 {
        parts.push(format!("{}小时", hours));
    }
    if mins > 0 {
        parts.push(format!("{}分钟", mins));
    }
    if secs > 0 || (secs == 0 && parts.is_empty()) {
        parts.push(format!("{}秒", secs));
    }
    parts.join("")
}

/// 从形如 "上传1.00下载0.00的魔法" 的文本中解析上传和下载倍率。
fn parse_upload_download(text: &str) -> (f64, f64) {
    let upload_start = match text.find("上传") {
        Some(pos) => pos + "上传".len(),
        None => return (1.0, 1.0),
    };
    let upload_end = text[upload_start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|p| upload_start + p)
        .unwrap_or(text.len());
    let upload_val: f64 = text[upload_start..upload_end].parse().unwrap_or(1.0);

    let download_start = match text[upload_end..].find("下载") {
        Some(pos) => upload_end + pos + "下载".len(),
        None => return (upload_val, 1.0),
    };
    let download_end = text[download_start..]
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .map(|p| download_start + p)
        .unwrap_or(text.len());
    let download_val: f64 = text[download_start..download_end].parse().unwrap_or(1.0);

    (upload_val, download_val)
}

/// 从 URL 查询串中提取 id 参数的值。
fn extract_seed_id(href: &str) -> Option<String> {
    let needle = "details.php?id=";
    let start = href.find(needle)? + needle.len();
    let id: String = href[start..]
        .chars()
        .take_while(|ch| ch.is_ascii_digit())
        .collect();
    if id.is_empty() { None } else { Some(id) }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── parse_elapsed_time ────────────────────────────────────────

    #[test]
    fn elapsed_hours_minutes() {
        assert_eq!(parse_elapsed_time("4小时15分钟前"), 4 * 3600 + 15 * 60);
    }

    #[test]
    fn elapsed_minutes_seconds() {
        assert_eq!(parse_elapsed_time("3分钟20秒前"), 3 * 60 + 20);
    }

    #[test]
    fn elapsed_single_hour() {
        assert_eq!(parse_elapsed_time("5小时前"), 5 * 3600);
    }

    #[test]
    fn elapsed_single_minute() {
        assert_eq!(parse_elapsed_time("8分钟前"), 8 * 60);
    }

    #[test]
    fn elapsed_with_soft_hyphens() {
        // &shy; → U+00AD
        let text = "4\u{00AD}分钟\u{00AD}54\u{00AD}秒\u{00AD}前";
        assert_eq!(parse_elapsed_time(text), 4 * 60 + 54);
    }

    // -- parse_free_duration / human_duration --

    #[test]
    fn free_duration_days_hours() {
        assert_eq!(parse_free_duration("持续1天0小时"), Some(86400));
        assert_eq!(parse_free_duration("持续5天0小时"), Some(5 * 86400));
    }

    #[test]
    fn free_duration_hours_only() {
        assert_eq!(parse_free_duration("持续12小时"), Some(12 * 3600));
    }

    #[test]
    fn free_duration_days_hours_varied() {
        assert_eq!(parse_free_duration("持续0天12小时"), Some(12 * 3600));
    }

    #[test]
    fn human_duration_formats() {
        assert_eq!(human_duration(0), "0秒");
        assert_eq!(human_duration(3600), "1小时");
        assert_eq!(human_duration(3661), "1小时1分钟1秒");
        assert_eq!(human_duration(86400 + 3600), "1天1小时");
    }

    // -- parse_shoutbox --

    #[test]
    fn parses_magic_entry_with_download_zero() {
        let html = r#"
        <div>
          <span class='date'>[<time title="21:58:17">4分钟54秒前</time>]</span>
          魔法使 <a href="userdetails.php?id=123">testuser</a> 对种子
          <a href="details.php?id=98765&amp;hits=1">[Test.Seed.Name.V1]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=456">上传1.00下载0.00的魔法</a>
          , 持续1天0小时
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 1);
        let item = &items[0];
        assert_eq!(item.guid, "98765");
        assert_eq!(item.title, "[Test.Seed.Name.V1]");
        assert_eq!(
            item.download_url,
            "https://u2.dmhy.org/download.php?id=98765"
        );
        assert_eq!(item.link, None);
        assert_eq!(item.download_volume_factor, Some(0.0));
        assert_eq!(item.upload_volume_factor, Some(1.0));
        // 持续1天0小时 - 4分钟54秒 ≈ 86306s
        assert!(item.free_end_timestamp.is_some());
    }

    #[test]
    fn parses_magic_entry_with_double_upload() {
        let html = r#"
        <div>
          <span class='date'>[<time title="20:55:46">1小时7分钟前</time>]</span>
          魔法少女 <a href="userdetails.php?id=123">Neko</a> 对种子
          <a href="details.php?id=55555&amp;hits=1">[2X.Release.Name]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=456">上传2.00下载0.00的魔法</a>
          , 持续1天0小时
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].upload_volume_factor, Some(2.0));
        assert_eq!(items[0].download_volume_factor, Some(0.0));
        assert!(items[0].free_end_timestamp.is_some());
    }

    #[test]
    fn skips_entry_with_nonzero_download() {
        let html = r#"
        <div>
          <span class='date'>[<time>30分钟前</time>]</span>
          魔法使 <a href="userdetails.php?id=123">testuser</a> 对种子
          <a href="details.php?id=77777&amp;hits=1">[Normal.Seed]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=456">上传2.00下载1.00的魔法</a>
          , 持续1天0小时
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn skips_entry_older_than_12_hours() {
        let html = r#"
        <div>
          <span class='date'>[<time>13小时30分钟前</time>]</span>
          魔法使 <a href="userdetails.php?id=123">testuser</a> 对种子
          <a href="details.php?id=88888&amp;hits=1">[Too.Old.Seed]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=456">上传1.00下载0.00的魔法</a>
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn keeps_entry_within_12_hours() {
        let html = r#"
        <div>
          <span class='date'>[<time>11小时59分钟前</time>]</span>
          魔法使 <a href="userdetails.php?id=123">testuser</a> 对种子
          <a href="details.php?id=99999&amp;hits=1">[Still.Fresh.Seed]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=456">上传1.00下载0.00的魔法</a>
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].guid, "99999");
    }

    #[test]
    fn skips_non_magic_entry() {
        let html = r#"
        <div>
          <a href="userdetails.php?id=123">testuser</a>
          秒收2.514UCoin, 哦哦哦好多小钱钱$_$
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 0);
    }

    #[test]
    fn parses_multiple_entries() {
        let html = r#"
        <div>
          <span class='date'>[<time>30分钟前</time>]</span>
          魔法使 <a href="userdetails.php?id=123">user1</a> 对种子
          <a href="details.php?id=11111&amp;hits=1">[Seed.One]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=1">上传1.00下载0.00的魔法</a>
        </div>
        <div>
          <span class='date'>[<time>1小时前</time>]</span>
          魔法使 <a href="userdetails.php?id=456">user2</a> 对种子
          <a href="details.php?id=22222&amp;hits=1">[Seed.Two]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=2">上传2.00下载0.00的魔法</a>
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert_eq!(items.len(), 2);
        assert_eq!(items[0].guid, "11111");
        assert_eq!(items[0].upload_volume_factor, Some(1.0));
        assert_eq!(items[1].guid, "22222");
        assert_eq!(items[1].upload_volume_factor, Some(2.0));
    }

    #[test]
    fn handles_empty_html() {
        let items = parse_shoutbox("<html><body></body></html>").unwrap();
        assert!(items.is_empty());
    }

    #[test]
    fn returns_empty_for_no_download_zero_entries() {
        let html = r#"
        <div>
          <span class='date'>[<time>5分钟前</time>]</span>
          魔法使 <a href="userdetails.php?id=123">user</a> 对种子
          <a href="details.php?id=33333&amp;hits=1">[Seed]</a>
          完成了一次<a href="promotion.php?action=detail&amp;id=3">上传1.00下载0.50的魔法</a>
        </div>"#;

        let items = parse_shoutbox(html).unwrap();
        assert!(items.is_empty());
    }

    // ── parse_detail_page ──────────────────────────────────────────

    #[test]
    fn parses_size_seeders_and_free_from_detail() {
        let html = r#"
        <html><body>
        <table>
        <tr>
          <td class="rowhead">基本信息</td>
          <td class="rowfollow">
            <b>发布时间:</b>&nbsp;<time title="2026-06-11 20:31:53">2小时前</time>&nbsp;&nbsp;&nbsp;
            <b>大小:</b>&nbsp;82.153 GiB&nbsp;&nbsp;&nbsp;
            <b>类型:</b>&nbsp;BDMV
          </td>
        </tr>
        <tr>
          <td class="rowhead">流量优惠</td>
          <td class="rowfollow">
            <img class="pro_free" src="pic/trans.gif" alt="FREE" />
            &nbsp;<b>剩余 <time title="2026-06-16 21:30:24">4天22小时</time></b>
          </td>
        </tr>
        <tr>
          <td class="rowhead">同伴</td>
          <td class="rowfollow">
            <div id="peercount"><b>3个做种者</b> | <b>74个下载者</b></div>
          </td>
        </tr>
        </table>
        </body></html>"#;

        let info = parse_detail_page(html).unwrap();
        // 82.153 GiB
        assert!(info.size_bytes.is_some());
        let size_gb = info.size_bytes.unwrap() as f64 / (1024.0 * 1024.0 * 1024.0);
        assert!((size_gb - 82.153).abs() < 0.01);
        // 3 seeders, 74 downloaders
        assert_eq!(info.seeders, Some(3));
        assert_eq!(info.downloaders, Some(74));
        // free end timestamp
        assert!(info.free_end_timestamp.is_some());
    }

    #[test]
    fn parses_detail_without_free() {
        let html = r#"
        <html><body>
        <table>
        <tr>
          <td class="rowhead">基本信息</td>
          <td class="rowfollow">
            <b>大小:</b>&nbsp;1.5 GB
          </td>
        </tr>
        <tr>
          <td class="rowhead">同伴</td>
          <td class="rowfollow">
            <div id="peercount"><b>0个做种者</b> | <b>0个下载者</b></div>
          </td>
        </tr>
        </table>
        </body></html>"#;

        let info = parse_detail_page(html).unwrap();
        assert!(info.size_bytes.is_some());
        assert_eq!(info.seeders, Some(0));
        assert_eq!(info.free_end_timestamp, None);
    }

    // ── parse_peer_list_page ─────────────────────────────────────────

    #[test]
    fn idle_time_parsing() {
        assert_eq!(parse_idle_time_to_secs("00:19:36"), 19 * 60 + 36);
        assert_eq!(parse_idle_time_to_secs("01:17:54"), 3600 + 17 * 60 + 54);
        assert_eq!(parse_idle_time_to_secs("00:00:00"), 0);
    }

    #[test]
    fn connect_time_parsing_with_days() {
        // "36天 00:19:38" → 36*86400 + 19*60 + 38
        assert_eq!(
            parse_connect_time_to_secs("36天 00:19:38"),
            36 * 86400 + 19 * 60 + 38
        );
    }

    #[test]
    fn connect_time_parsing_no_days() {
        assert_eq!(
            parse_connect_time_to_secs("17:17:54"),
            17 * 3600 + 17 * 60 + 54
        );
        assert_eq!(parse_connect_time_to_secs("00:07:57"), 7 * 60 + 57);
    }

    #[test]
    fn speed_parsing() {
        // 142.609 KiB/s → 142.609 * 1024 ≈ 146031 B/s
        assert_eq!(
            parse_speed_to_bytes("142.609 KiB/s"),
            (142.609f64 * 1024.0) as u64
        );
        assert_eq!(parse_speed_to_bytes("0 B/s"), 0);
        // 0.115 B/s → 0 (truncated)
        assert_eq!(parse_speed_to_bytes("0.115 B/s"), 0);
    }

    /// 第一个种子 (id=18285): 标称 7 做种者，应过滤出 6（排除空闲超 45 分钟的 Hoshino0881118）
    #[test]
    fn parses_peer_list_torrent_1() {
        let html = r#"
<b>7 做种者</b>

<table class="main-inner" min-width="825px" border="1" cellspacing="0" cellpadding="3"><tr><td class="colhead" align="center" width="1%">用户</td><td class="colhead" align="center" width="1%">上传量</td><td class="colhead" align="center" width="1%">平均速度</td><td class="colhead" align="center" width="1%">瞬时速度</td><td class="colhead" align="center" width="1%">下载量</td><td class="colhead" align="center" width="1%">平均速度</td><td class="colhead" align="center" width="1%">分享率</td><td class="colhead" align="center" width="1%">连接时间</td><td class="colhead" align="center" width="1%">空闲</td><td class="colhead" align="center" width="1%">客户端</td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">TnsSub</td><td class="rowfollow" align="right" width="1%"><nobr>649.218 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>142.609 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>36天 00:19:38</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:19:36</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/4.4.5</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">Waykey</td><td class="rowfollow" align="right" width="1%"><nobr>247.625 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>164.296 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>18天 07:10:04</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:10:03</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/4.3.9</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">whyaltruist</td><td class="rowfollow" align="right" width="1%"><nobr>42.100 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>93.906 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>5天 10:55:05</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:20:04</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/5.2.0</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">Hoshino0881118</td><td class="rowfollow" align="right" width="1%"><nobr>2.395 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>9.520 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>3天 02:05:30</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:48:17</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/5.0.3</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">shoo</td><td class="rowfollow" align="right" width="1%"><nobr>1.823 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>4.194 MiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>45.077 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>103.728 MiB/s</nobr></td><td class="rowfollow" align="center" width="1%">0.040</td><td class="rowfollow" align="right" width="1%"><nobr>00:07:57</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:00:32</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/4.3.8</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">zhangxzh</td><td class="rowfollow" align="right" width="1%"><nobr>1.210 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>2.881 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>43.543 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>5天 02:21:09</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:02:10</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/5.1.4</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">-39-</td><td class="rowfollow" align="right" width="1%"><nobr>732.156 MiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>262.000 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>3天 01:00:58</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:08:46</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/4.3.9</nobr></td></tr>
</table><b>3 下载者</b>

<table class="main-inner"><tr><td class="colhead">用户</td></tr>
<tr><td class="rowfollow">cyril2007</td></tr>
<tr><td class="rowfollow">匿名</td></tr>
<tr><td class="rowfollow">Baychimo</td></tr>
</table>"#;

        let result = parse_peer_list_page(html).unwrap();
        // 标称 7，排除 Hoshino0881118 (空闲 00:48:17 > 45min)
        assert_eq!(result.seeders, 6);
        assert_eq!(result.downloaders, 3);
    }

    /// 第二个种子: 标称 5 做种者，应过滤出 3
    /// (排除空闲超 45 分钟的 hahahaha6789 + 双差生 天生是凡人)
    #[test]
    fn parses_peer_list_torrent_2() {
        let html = r#"
<b>5 做种者</b>

<table class="main-inner" min-width="825px" border="1" cellspacing="0" cellpadding="3"><tr><td class="colhead" align="center" width="1%">用户</td><td class="colhead" align="center" width="1%">上传量</td><td class="colhead" align="center" width="1%">平均速度</td><td class="colhead" align="center" width="1%">瞬时速度</td><td class="colhead" align="center" width="1%">下载量</td><td class="colhead" align="center" width="1%">平均速度</td><td class="colhead" align="center" width="1%">分享率</td><td class="colhead" align="center" width="1%">连接时间</td><td class="colhead" align="center" width="1%">空闲</td><td class="colhead" align="center" width="1%">客户端</td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">洛天依</td><td class="rowfollow" align="right" width="1%"><nobr>305.222 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>147.432 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>41.102 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>38.648 MiB/s</nobr></td><td class="rowfollow" align="center" width="1%">7.426</td><td class="rowfollow" align="right" width="1%"><nobr>25天 03:32:30</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:32:15</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/4.3.9</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">whyaltruist</td><td class="rowfollow" align="right" width="1%"><nobr>24.378 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>54.377 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>5天 11:04:28</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:29:32</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/5.2.0</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">Hoshino0881118</td><td class="rowfollow" align="right" width="1%"><nobr>1.596 GiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>6.283 KiB/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>3天 02:13:59</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:13:59</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/5.1.4</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">天生是凡人</td><td class="rowfollow" align="right" width="1%"><nobr>368.292 KiB</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0.115 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">无限</td><td class="rowfollow" align="right" width="1%"><nobr>37天 23:26:06</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>00:24:49</nobr></td><td class=rowfollow align=center width=1%><nobr>Transmission/4.0.5</nobr></td></tr>
<tr><td class="rowfollow nowrap" align="left" width="1%">hahahaha6789</td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B/s</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>0 B</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>N/A</nobr></td><td class="rowfollow" align="center" width="1%">---</td><td class="rowfollow" align="right" width="1%"><nobr>17:17:54</nobr></td><td class="rowfollow" align="right" width="1%"><nobr>01:17:54</nobr></td><td class=rowfollow align=center width=1%><nobr>qBittorrent/5.0.4</nobr></td></tr>
</table><b>10 下载者</b>

<table class="main-inner"><tr><td class="colhead">用户</td></tr>
<tr><td class="rowfollow">shoo</td></tr>
</table>"#;

        let result = parse_peer_list_page(html).unwrap();
        // 标称 5: 排除 hahahaha6789 (空闲 01:17:54 > 45min) + 天生是凡人 (双差生: 37天, 368KiB, 0.115 B/s)
        assert_eq!(result.seeders, 3);
        assert_eq!(result.downloaders, 10);
    }
}
