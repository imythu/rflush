use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::Client;
use serde_json::Value;
use tracing::debug;

use super::{SiteAuth, SiteClient, SiteTestResult, TorrentAttributes, UserStats};
use std::pin::Pin;
use std::future::Future;

const MTEAM_DEFAULT_API: &str = "https://api.m-team.cc";
const BROWSER_UA: &str =
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
/// M-Team 详情请求间最小间隔（毫秒），防止 API 限流
const MTEAM_REQUEST_INTERVAL_MS: u64 = 4000;

pub struct MTeamClient {
    base_url: String,
    api_key: String,
    client: Client,
}

impl MTeamClient {
    pub fn new(base_url: String, auth: SiteAuth) -> Self {
        let api_key = match &auth {
            SiteAuth::ApiKey { api_key } => api_key.clone(),
            _ => String::new(),
        };
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to build reqwest client");
        let url = if base_url.is_empty() {
            MTEAM_DEFAULT_API.to_string()
        } else {
            base_url.trim_end_matches('/').to_string()
        };
        Self {
            base_url: url,
            api_key,
            client,
        }
    }

    fn build_headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_UA));
        if let Ok(val) = HeaderValue::from_str(&self.api_key) {
            headers.insert("x-api-key", val);
        }
        headers
    }

    async fn api_post(&self, path: &str, body: Option<&Value>) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        debug!("M-Team API POST: {}", url);

        let mut req = self.client.post(&url).headers(self.build_headers());
        if let Some(body) = body {
            req = req.json(body);
        }

        let resp = req.send().await.map_err(|e| format!("请求失败: {}", e))?;

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        let text = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))?;

        let json: Value =
            serde_json::from_str(&text).map_err(|_| "响应不是有效JSON".to_string())?;

        let code = json
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if code != "0" && code != "SUCCESS" {
            let msg = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!("API错误 code={}: {}", code, msg));
        }

        Ok(json)
    }

    /// 使用 form data 发送 POST 请求（M-Team 部分接口需要）
    async fn api_post_form(&self, path: &str, form: &[(&str, &str)]) -> Result<Value, String> {
        let url = format!("{}{}", self.base_url, path);
        debug!("M-Team API POST form: {} {:?}", url, form);

        let resp = self.client.post(&url)
            .headers(self.build_headers())
            .form(form)
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

        let code = json
            .get("code")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown");
        if code != "0" && code != "SUCCESS" {
            let msg = json
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown error");
            return Err(format!("API错误 code={}: {}", code, msg));
        }

        Ok(json)
    }

    fn extract_torrent_id(detail_url: &str) -> Option<String> {
        let mut current = String::new();
        let mut last = None;
        for ch in detail_url.chars() {
            if ch.is_ascii_digit() {
                current.push(ch);
            } else if !current.is_empty() {
                last = Some(current.clone());
                current.clear();
            }
        }
        if !current.is_empty() {
            last = Some(current);
        }
        last
    }

    fn parse_discount(discount: Option<&str>) -> (Option<f64>, Option<f64>, bool) {
        match discount.unwrap_or_default() {
            "FREE" => (Some(0.0), Some(1.0), false),
            "FREE_2XUP" | "TWOFREE" => (Some(0.0), Some(2.0), false),
            "PERCENT_50" => (Some(0.5), Some(1.0), false),
            "PERCENT_50_2XUP" => (Some(0.5), Some(2.0), false),
            "PERCENT_70" => (Some(0.3), Some(1.0), false),
            "PERCENT_70_2XUP" => (Some(0.3), Some(2.0), false),
            "NORMAL" | "" => (Some(1.0), Some(1.0), false),
            other => {
                debug!("未识别的 M-Team 促销类型: {:?}", other);
                (Some(1.0), Some(1.0), false)
            }
        }
    }
}

impl SiteClient for MTeamClient {
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
        let json = self.api_post("/api/member/profile", None).await?;
        let data = json
            .get("data")
            .ok_or_else(|| "响应缺少 data 字段".to_string())?;

        let username = data
            .get("username")
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();

        let member_count = data.get("memberCount").unwrap_or(data);

        let uploaded = member_count
            .get("uploaded")
            .and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| v.as_u64())
            })
            .unwrap_or(0);

        let downloaded = member_count
            .get("downloaded")
            .and_then(|v| {
                v.as_str()
                    .and_then(|s| s.parse::<u64>().ok())
                    .or_else(|| v.as_u64())
            })
            .unwrap_or(0);

        let ratio = member_count.get("shareRate").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64())
        });

        let bonus = data.get("bonus").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<f64>().ok())
                .or_else(|| v.as_f64())
        });

        let seeding_count = member_count.get("seeding").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| v.as_u64().map(|n| n as u32))
        });

        let leeching_count = member_count.get("leeching").and_then(|v| {
            v.as_str()
                .and_then(|s| s.parse::<u32>().ok())
                .or_else(|| v.as_u64().map(|n| n as u32))
        });

        Ok(UserStats {
            username,
            uploaded,
            downloaded,
            ratio,
            bonus,
            seeding_count,
            leeching_count,
        })
        })
    }

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>> {
        let detail_url = detail_url.to_string();
        Box::pin(async move {
            let torrent_id = Self::extract_torrent_id(&detail_url)
                .ok_or_else(|| format!("无法从链接提取 M-Team 种子 ID: {detail_url}"))?;

            // 重试循环：遇到"請求過於頻繁"时自动退避重试
            let max_retries = 3;
            let mut last_err = String::new();
            for attempt in 0..=max_retries {
                if attempt > 0 {
                    let backoff_ms = 10000 * attempt as u64;
                    debug!("M-Team 重试 {}/{} 等待 {}ms: {}", attempt, max_retries, backoff_ms, &detail_url);
                    tokio::time::sleep(std::time::Duration::from_millis(backoff_ms)).await;
                } else {
                    // 首次请求前也加延迟，防止并发触发限流
                    let delay_ms = MTEAM_REQUEST_INTERVAL_MS + simple_random_ms(4000);
                    tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                }

                let form = [("id", torrent_id.as_str())];
                match self.api_post_form("/api/torrent/detail", &form).await {
                    Ok(json) => {
                        let data = json
                            .get("data")
                            .ok_or_else(|| "响应缺少 data 字段".to_string())?;
                        let status = data.get("status").unwrap_or(data);
                        let discount = status.get("discount").and_then(|v| v.as_str());
                        let (download_volume_factor, upload_volume_factor, hit_and_run) =
                            Self::parse_discount(discount);
                        let peer_count = status.get("seeders").and_then(|v| {
                            v.as_str()
                                .and_then(|s| s.parse::<i32>().ok())
                                .or_else(|| v.as_i64().map(|n| n as i32))
                        });

                        return Ok(TorrentAttributes {
                            free: download_volume_factor == Some(0.0),
                            two_x_free: download_volume_factor == Some(0.0)
                                && upload_volume_factor.is_some_and(|factor| factor >= 2.0),
                            hit_and_run,
                            peer_count,
                            download_volume_factor,
                            upload_volume_factor,
                        });
                    }
                    Err(e) => {
                        if e.contains("頻繁") {
                            last_err = e;
                            debug!("M-Team 限流，准备重试 {}/{}: {}", attempt + 1, max_retries, &detail_url);
                            continue;
                        }
                        return Err(e);
                    }
                }
            }
            Err(last_err)
        })
    }
}

/// 简单的伪随机数生成，基于当前时间纳秒，返回 [0, max_ms) 范围内的毫秒数
fn simple_random_ms(max_ms: u64) -> u64 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // 混入当前线程地址作为额外熵源
    let seed = nanos ^ (nanos >> 32);
    if max_ms == 0 { 0 } else { seed % max_ms }
}
