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
            download_url: format!(
                "https://u2.dmhy.org/download.php?id={id}",
                id = seed_id
            ),
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
            if let Some(size_str) = after
                .split_whitespace()
                .next()
                .and_then(|num| {
                    // 找到下一个词（单位）
                    let rest = &after[num.len()..].trim();
                    let unit = rest.split_whitespace().next().unwrap_or("");
                    Some(format!("{} {}", num, unit))
                })
            {
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
    if id.is_empty() {
        None
    } else {
        Some(id)
    }
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
}
