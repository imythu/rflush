use chrono::{FixedOffset, NaiveDateTime, TimeZone};
use regex::{Regex, escape};
use reqwest::header::{COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, Url};
use scraper::{Html, Selector};
use serde_json::Value;
use tracing::{debug, warn};

use super::{SiteAdapter, SiteAuth, SiteTestResult, TorrentAttributes, UserStats};
use std::future::Future;
use std::pin::Pin;

pub struct NexusPhpAdapter {
    base_url: String,
    auth: SiteAuth,
    request_headers: HeaderMap,
    client: Client,
}

impl NexusPhpAdapter {
    pub fn new(
        base_url: String,
        auth: SiteAuth,
        request_headers: HeaderMap,
        client: Client,
    ) -> Self {
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
            request_headers,
            client,
        }
    }

    fn cookie_value(&self) -> Option<&str> {
        match &self.auth {
            SiteAuth::Cookie { cookie } => Some(cookie.as_str()),
            SiteAuth::CookiePasskey { cookie, .. } => Some(cookie.as_str()),
            _ => None,
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = self.request_headers.clone();
        if let Some(cookie) = self.cookie_value()
            && let Ok(val) = HeaderValue::from_str(cookie)
        {
            headers.insert(COOKIE, val);
        }
        headers
    }

    async fn fetch_user_info_api(&self) -> Result<UserStats, String> {
        let url = format!("{}/api/user", self.base_url);
        debug!("NexusPHP API request: {}", url);

        let resp = self
            .client
            .get(&url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;
        let json: Value =
            serde_json::from_str(&text).map_err(|_| "响应不是有效JSON".to_string())?;

        if json.get("success").and_then(Value::as_bool) == Some(false) {
            return Err(json_error_message(&json).unwrap_or_else(|| "API 返回失败".to_string()));
        }

        let data = json
            .get("data")
            .and_then(|value| value.get("user").or(Some(value)))
            .unwrap_or(&json);
        let counters = data
            .get("memberCount")
            .or_else(|| data.get("stats"))
            .unwrap_or(data);

        let username = data
            .get("username")
            .or_else(|| data.get("name"))
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty() && !value.eq_ignore_ascii_case("unknown"))
            .ok_or_else(|| "API 响应缺少用户名".to_string())?
            .to_string();

        let uid = data
            .get("uid")
            .or_else(|| data.get("id"))
            .or_else(|| data.get("user_id"))
            .and_then(json_value_to_string);

        let uploaded = counters
            .get("uploaded")
            .or_else(|| counters.get("upload"))
            .or_else(|| data.get("upload"))
            .and_then(json_value_to_bytes);

        let downloaded = counters
            .get("downloaded")
            .or_else(|| counters.get("download"))
            .or_else(|| data.get("download"))
            .and_then(json_value_to_bytes);

        if uploaded.is_none() && downloaded.is_none() {
            return Err("API 响应缺少上传量和下载量".to_string());
        }

        let ratio = counters
            .get("ratio")
            .or_else(|| data.get("ratio"))
            .and_then(json_value_to_f64);
        let bonus = data
            .get("bonus")
            .or_else(|| data.get("seedbonus"))
            .or_else(|| counters.get("bonus"))
            .and_then(json_value_to_f64);

        Ok(UserStats {
            uid,
            username,
            uploaded: uploaded.unwrap_or(0),
            downloaded: downloaded.unwrap_or(0),
            ratio: ratio.or_else(|| ratio_from_totals(uploaded, downloaded)),
            bonus,
            seeding_count: counters
                .get("seeding")
                .or_else(|| counters.get("seeding_count"))
                .and_then(json_value_to_u32),
            leeching_count: counters
                .get("leeching")
                .or_else(|| counters.get("leeching_count"))
                .and_then(json_value_to_u32),
        })
    }

    async fn fetch_user_info_html(&self) -> Result<UserStats, String> {
        let url = format!("{}/index.php", self.base_url);
        debug!("NexusPHP HTML request: {}", url);

        let index_html = self.fetch_html_page(&url, "首页").await?;
        let identity = extract_current_user(&index_html)
            .ok_or_else(|| "首页没有找到当前用户链接，请检查 Cookie 是否有效".to_string())?;
        let detail_url = self.resolve_same_origin_url(&identity.href)?;
        let detail_html = self.fetch_html_page(&detail_url, "用户详情页").await?;

        let detail_identity = extract_current_user(&detail_html);
        let uid = Some(
            detail_identity
                .as_ref()
                .map(|value| value.uid.as_str())
                .unwrap_or(identity.uid.as_str())
                .to_string(),
        );
        let username = detail_identity
            .as_ref()
            .and_then(|value| value.username.clone())
            .or(identity.username)
            .unwrap_or_else(|| uid.clone().unwrap_or_else(|| "unknown".to_string()));

        let detail_text = extract_visible_text(&detail_html);
        let index_text = extract_visible_text(&index_html);
        let uploaded = parse_labeled_size(&detail_text, &["上传量", "上傳量", "Uploaded"])
            .or_else(|| parse_labeled_size(&index_text, &["上传量", "上傳量", "Uploaded"]));
        let downloaded = parse_labeled_size(&detail_text, &["下载量", "下載量", "Downloaded"])
            .or_else(|| parse_labeled_size(&index_text, &["下载量", "下載量", "Downloaded"]));

        if uploaded.is_none() && downloaded.is_none() {
            return Err(
                "用户详情页没有找到上传量或下载量，站点页面结构可能需要单独适配".to_string(),
            );
        }

        let ratio = parse_labeled_number(&detail_text, &["分享率", "Ratio"])
            .or_else(|| ratio_from_totals(uploaded, downloaded));
        let bonus = parse_labeled_number(
            &detail_text,
            &[
                "魔力值",
                "Karma Points",
                "做种积分",
                "做種積分",
                "Seeding Points",
                "保种积分",
                "魅力值",
                "星焱",
                "沙粒",
                "魔力",
                "Bonus",
            ],
        );
        let mut seeding_count = parse_labeled_integer(
            &detail_text,
            &["当前做种", "當前做種", "做种数", "做種數", "Seeding"],
        );
        let leeching_count = parse_labeled_integer(
            &detail_text,
            &["当前下载", "當前下載", "下载数", "下載數", "Leeching"],
        );

        // PT-Depiler 的 NexusPHP 通用 schema 在详情页缺少做种数据时，也会查询此 AJAX 接口。
        if seeding_count.is_none()
            && let Some(user_id) = uid.as_deref()
        {
            seeding_count = self.fetch_seeding_count(user_id).await;
        }

        Ok(UserStats {
            uid,
            username,
            uploaded: uploaded.unwrap_or(0),
            downloaded: downloaded.unwrap_or(0),
            ratio,
            bonus,
            seeding_count,
            leeching_count,
        })
    }

    async fn fetch_html_page(&self, url: &str, label: &str) -> Result<String, String> {
        let response = self
            .client
            .get(url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|error| format!("{label}请求失败: {error}"))?;
        let status = response.status();
        let final_url = response.url().clone();
        if !status.is_success() {
            return Err(format!("{label}返回 HTTP {status}"));
        }
        let html = response
            .text()
            .await
            .map_err(|error| format!("读取{label}响应失败: {error}"))?;

        if looks_like_cloudflare_challenge(&html) {
            return Err(format!("{label}被 Cloudflare 验证页拦截"));
        }
        if looks_like_login_page(&html, &final_url) {
            return Err("Cookie 无效或已过期，站点返回了登录页".to_string());
        }
        Ok(html)
    }

    fn resolve_same_origin_url(&self, href: &str) -> Result<String, String> {
        let base = Url::parse(&format!("{}/", self.base_url.trim_end_matches('/')))
            .map_err(|error| format!("站点地址无效: {error}"))?;
        let url = base
            .join(href)
            .map_err(|error| format!("用户详情链接无效: {error}"))?;
        if base.scheme() != url.scheme()
            || base.host_str() != url.host_str()
            || base.port_or_known_default() != url.port_or_known_default()
        {
            return Err("用户详情链接跳转到了其他站点，已拒绝请求".to_string());
        }
        Ok(url.to_string())
    }

    async fn fetch_seeding_count(&self, user_id: &str) -> Option<u32> {
        let mut url = Url::parse(&format!("{}/getusertorrentlistajax.php", self.base_url)).ok()?;
        url.query_pairs_mut()
            .append_pair("userid", user_id)
            .append_pair("type", "seeding");
        let response = self
            .client
            .get(url)
            .headers(self.build_headers())
            .send()
            .await
            .ok()?;
        if !response.status().is_success() {
            return None;
        }
        let html = response.text().await.ok()?;
        let count = parse_seeding_ajax_count(&html);
        if count.is_none() {
            warn!("NexusPHP 做种列表已返回，但无法解析记录数");
        }
        count
    }

    async fn fetch_torrent_detail_html(&self, detail_url: &str) -> Result<String, String> {
        debug!("NexusPHP detail request: {}", detail_url);

        let resp = self
            .client
            .get(detail_url)
            .headers(self.build_headers())
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let html = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;
        if html.contains("login.php") && !html.contains("details") {
            return Err("Cookie 无效或已过期".to_string());
        }

        Ok(html)
    }

    fn detect_torrent_attributes(html: &str) -> TorrentAttributes {
        let document = Html::parse_document(html);
        let selectors = [
            "body",
            ".torrentname",
            ".embedded",
            ".sticky",
            ".pro_free",
            ".pro_free2up",
            ".free",
            ".twoupfree",
            ".twoup",
            ".hitandrun",
            ".hr",
            ".promotion-tag",
            ".torrent-promote",
            ".torrent-detail",
            ".torrent_info",
            "span",
            "a",
            "b",
            "strong",
            "font",
        ];
        let mut fragments = Vec::new();
        for selector_str in selectors {
            if let Ok(selector) = Selector::parse(selector_str) {
                for element in document.select(&selector) {
                    let text = element.text().collect::<Vec<_>>().join(" ");
                    let trimmed = text.trim();
                    if !trimmed.is_empty() {
                        fragments.push(trimmed.to_string());
                    }
                    if let Some(class) = element.value().attr("class") {
                        fragments.push(class.to_string());
                    }
                    if let Some(title) = element.value().attr("title") {
                        fragments.push(title.to_string());
                    }
                }
            }
        }
        let upper = fragments.join(" ").to_ascii_uppercase();

        let has_two_x_free = contains_any(
            &upper,
            &[
                "2XFREE",
                "2X FREE",
                "FREE 2XUP",
                "FREE,2XUP",
                "TWOUPFREE",
                "PRO_FREE2UP",
            ],
        );
        let has_free = has_two_x_free
            || contains_any(
                &upper,
                &[
                    "FREELEECH",
                    "FREE LEECH",
                    " FREE ",
                    "PRO_FREE",
                    " 免费 ",
                    " FREE<",
                    ">FREE ",
                ],
            );
        let hit_and_run = contains_any(
            &upper,
            &[
                "H&R",
                "HIT AND RUN",
                "HIT&RUN",
                "HR:",
                "HNR",
                "HITRUN",
                "HITANDRUN",
                " HR ",
            ],
        );

        let (download_volume_factor, upload_volume_factor) = if has_two_x_free {
            (Some(0.0), Some(2.0))
        } else if has_free {
            (Some(0.0), Some(1.0))
        } else {
            (
                detect_download_factor(&upper).or(Some(1.0)),
                detect_upload_factor(&upper).or(Some(1.0)),
            )
        };

        let free_end_timestamp = if has_free {
            detect_free_end_timestamp(html)
        } else {
            None
        };

        TorrentAttributes {
            free: has_free || download_volume_factor == Some(0.0),
            two_x_free: has_two_x_free,
            hit_and_run,
            seeder_count: None,
            leecher_count: None,
            free_end_timestamp,
            download_volume_factor,
            upload_volume_factor,
        }
    }
}

impl SiteAdapter for NexusPhpAdapter {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SiteTestResult, String>> + Send + '_>> {
        Box::pin(async move {
            match self.get_user_stats().await {
                Ok(stats) => Ok(SiteTestResult {
                    success: true,
                    message: format!("连接成功，用户: {}", stats.username),
                    user_stats: Some(stats),
                }),
                Err(e) => Ok(SiteTestResult {
                    success: false,
                    message: e,
                    user_stats: None,
                }),
            }
        })
    }

    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(async move {
            match self.fetch_user_info_api().await {
                Ok(stats) => Ok(stats),
                Err(api_error) => self.fetch_user_info_html().await.map_err(|html_error| {
                    format!("API 获取失败（{api_error}）；HTML 获取失败（{html_error}）")
                }),
            }
        })
    }

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        let detail_url = detail_url.to_string();
        Box::pin(async move {
            let html = self.fetch_torrent_detail_html(&detail_url).await?;
            Ok(Self::detect_torrent_attributes(&html))
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
struct CurrentUser {
    uid: String,
    username: Option<String>,
    href: String,
}

fn json_value_to_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| value.as_u64().map(|n| n.to_string()))
        .or_else(|| value.as_i64().map(|n| n.to_string()))
}

fn json_value_to_bytes(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_i64().and_then(|number| u64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_f64()
                .filter(|number| *number >= 0.0)
                .map(|number| number as u64)
        })
        .or_else(|| {
            let value = value.as_str()?.trim().replace(',', "");
            value
                .parse::<u64>()
                .ok()
                .or_else(|| extract_size_value(&value))
        })
}

fn json_value_to_f64(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<f64>().ok())
}

fn json_value_to_u32(value: &Value) -> Option<u32> {
    value
        .as_u64()
        .and_then(|number| u32::try_from(number).ok())
        .or_else(|| value.as_i64().and_then(|number| u32::try_from(number).ok()))
        .or_else(|| value.as_str()?.trim().replace(',', "").parse::<u32>().ok())
}

fn json_error_message(value: &Value) -> Option<String> {
    ["message", "msg", "error"]
        .iter()
        .find_map(|key| value.get(key).and_then(Value::as_str))
        .map(str::to_string)
}

fn ratio_from_totals(uploaded: Option<u64>, downloaded: Option<u64>) -> Option<f64> {
    match (uploaded, downloaded) {
        (Some(uploaded), Some(downloaded)) if downloaded > 0 => {
            Some(uploaded as f64 / downloaded as f64)
        }
        _ => None,
    }
}

fn extract_current_user(html: &str) -> Option<CurrentUser> {
    let document = Html::parse_document(html);
    let selectors = [
        "#info_block a[href*='userdetails.php'][href*='id=']",
        "#info_block a[href*='user.php'][href*='id=']",
        "a[href*='userdetails.php'][class*='Name']",
        "a[href*='userdetails.php'][href*='id=']",
        "a[href*='user.php'][href*='id=']",
    ];

    for selector in selectors {
        let Ok(selector) = Selector::parse(selector) else {
            continue;
        };
        for element in document.select(&selector) {
            let Some(href) = element.value().attr("href") else {
                continue;
            };
            let Some(uid) = extract_user_id_from_href(href) else {
                continue;
            };
            let username =
                normalize_text(element.text()).filter(|name| !name.eq_ignore_ascii_case("details"));
            return Some(CurrentUser {
                uid,
                username,
                href: href.to_string(),
            });
        }
    }
    None
}

fn extract_user_id_from_href(href: &str) -> Option<String> {
    let base = Url::parse("https://tracker.invalid/").ok()?;
    let url = base.join(href).ok()?;
    url.query_pairs()
        .find(|(key, _)| key == "id" || key == "userid" || key == "user_id")
        .map(|(_, value)| value.into_owned())
        .filter(|value| !value.is_empty())
}

fn normalize_text<'a>(parts: impl Iterator<Item = &'a str>) -> Option<String> {
    let text = parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ");
    (!text.is_empty()).then_some(text)
}

fn extract_visible_text(html: &str) -> String {
    let document = Html::parse_document(html);
    normalize_text(document.root_element().text()).unwrap_or_default()
}

fn parse_labeled_size(text: &str, labels: &[&str]) -> Option<u64> {
    labels.iter().find_map(|label| {
        let expression = format!(
            r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)\s*(bytes?|[kmgtpez]i?b)",
            escape(label)
        );
        let captures = Regex::new(&expression).ok()?.captures(text)?;
        size_from_parts(captures.get(1)?.as_str(), captures.get(2)?.as_str())
    })
}

fn parse_labeled_number(text: &str, labels: &[&str]) -> Option<f64> {
    labels.iter().find_map(|label| {
        let expression = format!(r"(?i){}\s*[^0-9]{{0,48}}([0-9][0-9,.]*)", escape(label));
        let value = Regex::new(&expression)
            .ok()?
            .captures(text)?
            .get(1)?
            .as_str()
            .replace(',', "");
        value.parse::<f64>().ok()
    })
}

fn parse_labeled_integer(text: &str, labels: &[&str]) -> Option<u32> {
    parse_labeled_number(text, labels).and_then(|value| {
        if value.is_finite() && value >= 0.0 && value <= u32::MAX as f64 {
            Some(value as u32)
        } else {
            None
        }
    })
}

fn parse_seeding_ajax_count(html: &str) -> Option<u32> {
    let text = extract_visible_text(html);
    let record_expression = Regex::new(r"(?i)([0-9][0-9,]*)\s*(?:条记录|條記錄|records?)").ok()?;
    if let Some(captures) = record_expression.captures(&text) {
        return captures
            .get(1)?
            .as_str()
            .replace(',', "")
            .parse::<u32>()
            .ok();
    }
    let summary_expression = Regex::new(r"([0-9][0-9,]*)\s*\|").ok()?;
    if let Some(captures) = summary_expression.captures(&text) {
        return captures
            .get(1)?
            .as_str()
            .replace(',', "")
            .parse::<u32>()
            .ok();
    }
    if Regex::new(r"(?i)no records?|没有记录|沒有記錄")
        .ok()?
        .is_match(&text)
    {
        return Some(0);
    }

    let document = Html::parse_document(html);
    let table_selector = Selector::parse("table").ok()?;
    let row_selector = Selector::parse("tr").ok()?;
    let table = document.select(&table_selector).next_back()?;
    let row_count = table.select(&row_selector).count();
    (row_count > 0).then(|| row_count.saturating_sub(1) as u32)
}

fn looks_like_login_page(html: &str, final_url: &Url) -> bool {
    if final_url.path().to_ascii_lowercase().contains("login.php") {
        return true;
    }
    let lower = html.to_ascii_lowercase();
    (lower.contains("type=\"password\"") || lower.contains("type='password'"))
        && (lower.contains("action=\"login.php")
            || lower.contains("action='login.php")
            || lower.contains("name=\"login\"")
            || lower.contains("name='login'"))
        && extract_current_user(html).is_none()
}

fn looks_like_cloudflare_challenge(html: &str) -> bool {
    let lower = html.to_ascii_lowercase();
    lower.contains("cf-chl-")
        || lower.contains("challenge-platform")
        || lower.contains("just a moment...")
        || lower.contains("cloudflare ray id")
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn detect_download_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &[
            "50%DL",
            "50% DL",
            "0.5X",
            "DOWNLOAD 50%",
            "50%DOWN",
            "半价",
            "五折",
        ],
    ) {
        Some(0.5)
    } else if contains_any(
        upper,
        &["30%DL", "30% DL", "0.3X", "DOWNLOAD 30%", "30%DOWN", "七折"],
    ) {
        Some(0.3)
    } else {
        None
    }
}

fn detect_upload_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &[
            "2XUP",
            "2X UP",
            "2XUPLOAD",
            "UPLOAD 200%",
            "UP 200%",
            "双倍上传",
        ],
    ) {
        Some(2.0)
    } else if contains_any(upper, &["0XUP", "UPLOAD 0%", "UP 0%", "零上传", "不计上传"]) {
        Some(0.0)
    } else {
        None
    }
}

fn detect_free_end_timestamp(html: &str) -> Option<i64> {
    const KEYWORDS: &[&str] = &[
        "free结束",
        "free 到期",
        "free截止",
        "free until",
        "promotion until",
        "限时免费",
        "優惠到期",
        "促销结束",
    ];

    let lower = html.to_lowercase();
    for keyword in KEYWORDS {
        let keyword_lower = keyword.to_lowercase();
        if let Some(pos) = lower.find(&keyword_lower) {
            let end = (pos + 160).min(html.len());
            if let Some(timestamp) = extract_datetime_to_utc8(&html[pos..end]) {
                return Some(timestamp);
            }
        }
    }

    extract_datetime_to_utc8(html)
}

fn extract_datetime_to_utc8(text: &str) -> Option<i64> {
    let tz = FixedOffset::east_opt(8 * 3600)?;
    for window in text.as_bytes().windows(19) {
        // 窗口可能切在多字节 UTF-8 字符中间，此时跳过而不是中止整个扫描。
        let Ok(candidate) = std::str::from_utf8(window) else {
            continue;
        };
        if looks_like_datetime(candidate)
            && let Ok(naive) = NaiveDateTime::parse_from_str(candidate, "%Y-%m-%d %H:%M:%S")
            && let Some(datetime) = tz.from_local_datetime(&naive).single()
        {
            return Some(datetime.timestamp());
        }
    }
    None
}

fn looks_like_datetime(value: &str) -> bool {
    value.len() == 19
        && value.chars().enumerate().all(|(index, ch)| match index {
            4 | 7 => ch == '-',
            10 => ch == ' ',
            13 | 16 => ch == ':',
            _ => ch.is_ascii_digit(),
        })
}

fn extract_size_value(text: &str) -> Option<u64> {
    let mut num_start = None;
    let mut num_end = None;

    for (i, ch) in text.char_indices() {
        if num_start.is_none() {
            if ch.is_ascii_digit() {
                num_start = Some(i);
            }
        } else if !ch.is_ascii_digit() && ch != '.' {
            num_end = Some(i);
            break;
        }
    }

    let start = num_start?;
    let end = num_end.unwrap_or(text.len());
    let number = text[start..end].trim();
    let unit_text = text[end..].trim();

    size_from_parts(number, unit_text)
}

fn size_from_parts(number: &str, unit: &str) -> Option<u64> {
    let number: f64 = number.replace(',', "").parse().ok()?;
    if !number.is_finite() || number < 0.0 {
        return None;
    }
    let normalized_unit = unit.trim().to_ascii_lowercase();
    let power = match normalized_unit.as_str() {
        value if value.starts_with("zb") || value.starts_with("zib") => 7,
        value if value.starts_with("eb") || value.starts_with("eib") => 6,
        value if value.starts_with("pb") || value.starts_with("pib") => 5,
        value if value.starts_with("tb") || value.starts_with("tib") => 4,
        value if value.starts_with("gb") || value.starts_with("gib") => 3,
        value if value.starts_with("mb") || value.starts_with("mib") => 2,
        value if value.starts_with("kb") || value.starts_with("kib") => 1,
        _ => 0,
    };
    let bytes = number * 1024_f64.powi(power);
    Some(bytes.min(u64::MAX as f64) as u64)
}

#[cfg(test)]
mod tests {
    use reqwest::Client;
    use reqwest::header::{COOKIE, HeaderMap, HeaderValue};

    use super::{
        NexusPhpAdapter, SiteAuth, extract_current_user, extract_visible_text,
        parse_labeled_integer, parse_labeled_number, parse_labeled_size, parse_seeding_ajax_count,
    };

    #[test]
    fn custom_headers_are_applied_without_overriding_authentication() {
        let mut custom = HeaderMap::new();
        custom.insert("x-browser-profile", HeaderValue::from_static("desktop"));
        custom.insert(COOKIE, HeaderValue::from_static("stale=1"));
        let adapter = NexusPhpAdapter::new(
            "https://tracker.example".to_string(),
            SiteAuth::Cookie {
                cookie: "uid=1".to_string(),
            },
            custom,
            Client::new(),
        );

        let headers = adapter.build_headers();
        assert_eq!(headers["x-browser-profile"], "desktop");
        assert_eq!(headers[COOKIE], "uid=1");
    }

    #[test]
    fn detects_free_and_hr_from_detail_html() {
        let attrs = NexusPhpAdapter::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>2XUP</span><span>H&amp;R</span></body></html>"#,
        );

        assert!(attrs.free);
        assert!(attrs.hit_and_run);
        assert_eq!(attrs.download_volume_factor, Some(0.0));
        assert_eq!(attrs.upload_volume_factor, Some(2.0));
    }

    #[test]
    fn detects_free_end_time_from_detail_html() {
        let attrs = NexusPhpAdapter::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>Free到期：2026-04-16 12:30:00</span></body></html>"#,
        );

        assert!(attrs.free_end_timestamp.is_some());
    }

    #[test]
    fn parses_pt_depiler_style_nexusphp_user_pages() {
        let html = r#"
            <html><body>
              <div id="info_block">
                <a class="User_Name" href="/userdetails.php?id=42"><b>Alice</b></a>
              </div>
              <table>
                <tr><td class="rowhead">传输</td><td>上传量 1.5 TiB 下载量 256 GiB 分享率 6.0</td></tr>
                <tr><td class="rowhead">魔力值</td><td>12,345.67</td></tr>
                <tr><td class="rowhead">当前做种</td><td>88</td></tr>
              </table>
            </body></html>
        "#;
        let identity = extract_current_user(html).unwrap();
        assert_eq!(identity.uid, "42");
        assert_eq!(identity.username.as_deref(), Some("Alice"));
        assert_eq!(identity.href, "/userdetails.php?id=42");

        let text = extract_visible_text(html);
        assert_eq!(
            parse_labeled_size(&text, &["上传量"]),
            Some(1_649_267_441_664)
        );
        assert_eq!(
            parse_labeled_size(&text, &["下载量"]),
            Some(274_877_906_944)
        );
        assert_eq!(parse_labeled_number(&text, &["分享率"]), Some(6.0));
        assert_eq!(parse_labeled_number(&text, &["魔力值"]), Some(12_345.67));
        assert_eq!(parse_labeled_integer(&text, &["当前做种"]), Some(88));
    }

    #[test]
    fn parses_english_and_traditional_transfer_labels() {
        let text = "Transfers Uploaded: 2.25 TB Downloaded: 512.5 GB 分享率: 4.49";
        assert_eq!(
            parse_labeled_size(text, &["Uploaded"]),
            Some(2_473_901_162_496)
        );
        assert_eq!(
            parse_labeled_size(text, &["Downloaded"]),
            Some(550_292_684_800)
        );

        let traditional = "傳送 上傳量 10 GiB 下載量 2 GiB";
        assert_eq!(
            parse_labeled_size(traditional, &["上傳量"]),
            Some(10_737_418_240)
        );
        assert_eq!(
            parse_labeled_size(traditional, &["下載量"]),
            Some(2_147_483_648)
        );
    }

    #[test]
    fn parses_nexusphp_seeding_ajax_summaries_and_tables() {
        assert_eq!(
            parse_seeding_ajax_count("<div><b>1,234</b> 条记录</div>"),
            Some(1234)
        );
        assert_eq!(
            parse_seeding_ajax_count(
                "<div>56 | 1.2 TiB</div><table><tr><th>标题</th></tr></table>"
            ),
            Some(56)
        );
        assert_eq!(parse_seeding_ajax_count("<div>No records.</div>"), Some(0));
        assert_eq!(
            parse_seeding_ajax_count(
                "<table><tr><th>标题</th></tr><tr><td>A</td></tr><tr><td>B</td></tr></table>"
            ),
            Some(2)
        );
    }
}
