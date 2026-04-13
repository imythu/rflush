use reqwest::header::{COOKIE, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::Client;
use scraper::{Html, Selector};
use serde_json::Value;
use tracing::debug;

use super::{SiteAuth, SiteClient, SiteTestResult, TorrentAttributes, UserStats};
use std::pin::Pin;
use std::future::Future;

const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

pub struct NexusPhpClient {
    base_url: String,
    auth: SiteAuth,
    client: Client,
}

impl NexusPhpClient {
    pub fn new(base_url: String, auth: SiteAuth) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            auth,
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
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_UA));
        if let Some(cookie) = self.cookie_value() {
            if let Ok(val) = HeaderValue::from_str(cookie) {
                headers.insert(COOKIE, val);
            }
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

        let text = resp.text().await.map_err(|e| format!("读取响应失败: {}", e))?;
        let json: Value =
            serde_json::from_str(&text).map_err(|_| "响应不是有效JSON".to_string())?;

        let data = json.get("data").unwrap_or(&json);

        let username = data
            .get("username")
            .or_else(|| data.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let uploaded = data
            .get("uploaded")
            .or_else(|| data.get("upload"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let downloaded = data
            .get("downloaded")
            .or_else(|| data.get("download"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let ratio = data.get("ratio").and_then(|v| v.as_f64());
        let bonus = data
            .get("bonus")
            .or_else(|| data.get("seedbonus"))
            .and_then(|v| v.as_f64());

        Ok(UserStats {
            username,
            uploaded,
            downloaded,
            ratio,
            bonus,
            seeding_count: None,
            leeching_count: None,
        })
    }

    async fn fetch_user_info_html(&self) -> Result<UserStats, String> {
        let url = format!("{}/index.php", self.base_url);
        debug!("NexusPHP HTML request: {}", url);

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

        let html = resp.text().await.map_err(|e| format!("读取响应失败: {}", e))?;

        if html.contains("login.php") && !html.contains("userdetails") {
            return Err("Cookie 无效或已过期".to_string());
        }

        let username = extract_between(&html, "class=\"User_Name\">", "<")
            .or_else(|| extract_between(&html, "class=\"username\">", "<"))
            .unwrap_or_else(|| "unknown".to_string());

        let uploaded = parse_size_from_html(&html, &["上传量", "Uploaded", "上傳量"]);
        let downloaded = parse_size_from_html(&html, &["下载量", "Downloaded", "下載量"]);

        Ok(UserStats {
            username,
            uploaded,
            downloaded,
            ratio: None,
            bonus: None,
            seeding_count: None,
            leeching_count: None,
        })
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

        let html = resp.text().await.map_err(|e| format!("读取响应失败: {}", e))?;
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
            &["2XFREE", "2X FREE", "FREE 2XUP", "FREE,2XUP", "TWOUPFREE", "PRO_FREE2UP"],
        );
        let has_free = has_two_x_free
            || contains_any(
                &upper,
                &["FREELEECH", "FREE LEECH", " FREE ", "PRO_FREE", " 免费 ", " FREE<", ">FREE "],
            );
        let hit_and_run = contains_any(
            &upper,
            &["H&R", "HIT AND RUN", "HIT&RUN", "HR:", "HNR", "HITRUN", "HITANDRUN", " HR "],
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

        TorrentAttributes {
            free: has_free || download_volume_factor == Some(0.0),
            two_x_free: has_two_x_free,
            hit_and_run,
            peer_count: None,
            download_volume_factor,
            upload_volume_factor,
        }
    }
}

impl SiteClient for NexusPhpClient {
    fn test_connection(&self) -> Pin<Box<dyn Future<Output = Result<SiteTestResult, String>> + Send + '_>> {
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

    fn get_user_stats(&self) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>> {
        Box::pin(async move {
            match self.fetch_user_info_api().await {
                Ok(stats) => Ok(stats),
                Err(_) => self.fetch_user_info_html().await,
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

fn extract_between(text: &str, start: &str, end: &str) -> Option<String> {
    let start_idx = text.find(start)? + start.len();
    let remaining = &text[start_idx..];
    let end_idx = remaining.find(end)?;
    Some(remaining[..end_idx].to_string())
}

fn parse_size_from_html(html: &str, keywords: &[&str]) -> u64 {
    for keyword in keywords {
        if let Some(pos) = html.find(keyword) {
            let after = &html[pos..];
            if let Some(size) = extract_size_value(after) {
                return size;
            }
        }
    }
    0
}

fn contains_any(haystack: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| haystack.contains(needle))
}

fn detect_download_factor(upper: &str) -> Option<f64> {
    if contains_any(
        upper,
        &["50%DL", "50% DL", "0.5X", "DOWNLOAD 50%", "50%DOWN", "半价", "五折"],
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
        &["2XUP", "2X UP", "2XUPLOAD", "UPLOAD 200%", "UP 200%", "双倍上传"],
    ) {
        Some(2.0)
    } else if contains_any(
        upper,
        &["0XUP", "UPLOAD 0%", "UP 0%", "零上传", "不计上传"],
    ) {
        Some(0.0)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::NexusPhpClient;

    #[test]
    fn detects_free_and_hr_from_detail_html() {
        let attrs = NexusPhpClient::detect_torrent_attributes(
            r#"<html><body><span>FREE</span><span>2XUP</span><span>H&amp;R</span></body></html>"#,
        );

        assert!(attrs.free);
        assert!(attrs.hit_and_run);
        assert_eq!(attrs.download_volume_factor, Some(0.0));
        assert_eq!(attrs.upload_volume_factor, Some(2.0));
    }
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
    let num: f64 = text[start..end].trim().parse().ok()?;
    let unit_text = text[end..].trim();

    let multiplier: u64 = if unit_text.starts_with("TB") || unit_text.starts_with("TiB") {
        1024 * 1024 * 1024 * 1024
    } else if unit_text.starts_with("GB") || unit_text.starts_with("GiB") {
        1024 * 1024 * 1024
    } else if unit_text.starts_with("MB") || unit_text.starts_with("MiB") {
        1024 * 1024
    } else if unit_text.starts_with("KB") || unit_text.starts_with("KiB") {
        1024
    } else {
        1
    };

    Some((num * multiplier as f64) as u64)
}
