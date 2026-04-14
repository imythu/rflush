use std::sync::Arc;

use reqwest::Client;
use reqwest::header::{COOKIE, USER_AGENT};
use reqwest::multipart;
use serde_json::Value;
use tokio::sync::Mutex;
use tracing::{debug, warn};

use super::{AddTorrentOptions, DownloaderClient, DownloaderTestResult, TorrentInfo};
use std::future::Future;
use std::pin::Pin;

const BROWSER_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";

pub struct QBittorrentClient {
    base_url: String,
    username: String,
    password: String,
    client: Client,
    cookie: Arc<Mutex<Option<String>>>,
}

impl QBittorrentClient {
    pub fn new(base_url: String, username: String, password: String) -> Self {
        let client = Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .expect("failed to build reqwest client");
        Self {
            base_url: base_url.trim_end_matches('/').to_string(),
            username,
            password,
            client,
            cookie: Arc::new(Mutex::new(None)),
        }
    }

    async fn login(&self) -> Result<String, String> {
        let url = format!("{}/api/v2/auth/login", self.base_url);
        debug!("qBittorrent login: {}", url);

        let resp = self
            .client
            .post(&url)
            .header(USER_AGENT, BROWSER_UA)
            .form(&[
                ("username", self.username.as_str()),
                ("password", self.password.as_str()),
            ])
            .send()
            .await
            .map_err(|e| format!("连接失败: {}", e))?;

        let cookie_header = resp
            .headers()
            .get_all("set-cookie")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .find(|s| s.contains("SID="))
            .map(|s| s.split(';').next().unwrap_or(s).to_string())
            .ok_or_else(|| {
                let body = "用户名或密码错误".to_string();
                body
            })?;

        let mut lock = self.cookie.lock().await;
        *lock = Some(cookie_header.clone());
        Ok(cookie_header)
    }

    async fn ensure_cookie(&self) -> Result<String, String> {
        let lock = self.cookie.lock().await;
        if let Some(c) = lock.as_ref() {
            return Ok(c.clone());
        }
        drop(lock);
        self.login().await
    }

    async fn api_get(&self, path: &str) -> Result<String, String> {
        let cookie = self.ensure_cookie().await?;
        let url = format!("{}{}", self.base_url, path);

        let resp = self
            .client
            .get(&url)
            .header(USER_AGENT, BROWSER_UA)
            .header(COOKIE, &cookie)
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if resp.status().as_u16() == 403 {
            // Cookie 过期，重新登录
            let new_cookie = self.login().await?;
            let resp = self
                .client
                .get(&url)
                .header(USER_AGENT, BROWSER_UA)
                .header(COOKIE, &new_cookie)
                .send()
                .await
                .map_err(|e| format!("请求失败: {}", e))?;
            return resp
                .text()
                .await
                .map_err(|e| format!("读取响应失败: {}", e));
        }

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        resp.text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))
    }

    async fn api_post_form(&self, path: &str, params: &[(&str, &str)]) -> Result<String, String> {
        let cookie = self.ensure_cookie().await?;
        let url = format!("{}{}", self.base_url, path);

        let resp = self
            .client
            .post(&url)
            .header(USER_AGENT, BROWSER_UA)
            .header(COOKIE, &cookie)
            .form(params)
            .send()
            .await
            .map_err(|e| format!("请求失败: {}", e))?;

        if resp.status().as_u16() == 403 {
            let new_cookie = self.login().await?;
            let resp = self
                .client
                .post(&url)
                .header(USER_AGENT, BROWSER_UA)
                .header(COOKIE, &new_cookie)
                .form(params)
                .send()
                .await
                .map_err(|e| format!("请求失败: {}", e))?;
            return resp
                .text()
                .await
                .map_err(|e| format!("读取响应失败: {}", e));
        }

        if !resp.status().is_success() {
            return Err(format!("HTTP {}", resp.status()));
        }

        resp.text()
            .await
            .map_err(|e| format!("读取响应失败: {}", e))
    }

    async fn api_post_multipart(
        &self,
        path: &str,
        form: multipart::Form,
    ) -> Result<String, String> {
        let cookie = self.ensure_cookie().await?;
        let url = format!("{}{}", self.base_url, path);
        debug!("qBittorrent POST multipart: {}", url);

        let resp = self
            .client
            .post(&url)
            .header(USER_AGENT, BROWSER_UA)
            .header(COOKIE, &cookie)
            .multipart(form)
            .send()
            .await
            .map_err(|e| {
                warn!(
                    "qBittorrent multipart request failed: url={} err={}",
                    url, e
                );
                format!("请求失败: url={} err={}", url, e)
            })?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            warn!(
                "qBittorrent multipart request failed: url={} status={} body={}",
                url,
                status,
                truncate_for_log(&body, 300)
            );
            return Err(format!(
                "HTTP {} url={} body={}",
                status,
                url,
                truncate_for_log(&body, 300)
            ));
        }

        let body = resp
            .text()
            .await
            .map_err(|e| format!("读取响应失败: url={} err={}", url, e))?;
        debug!(
            "qBittorrent multipart request succeeded: url={} body={}",
            url,
            truncate_for_log(&body, 120)
        );
        Ok(body)
    }
}

impl DownloaderClient for QBittorrentClient {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>> {
        Box::pin(async move {
            self.login().await?;
            let version = self
                .api_get("/api/v2/app/version")
                .await
                .unwrap_or_default();
            let free_space = self.get_free_space(None).await.ok();
            Ok(DownloaderTestResult {
                success: true,
                message: format!("连接成功，版本: {}", version.trim()),
                version: Some(version.trim().to_string()),
                free_space,
            })
        })
    }

    fn add_torrent(
        &self,
        torrent_data: Vec<u8>,
        filename: &str,
        options: &AddTorrentOptions,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let filename = filename.to_string();
        let options = options.clone();
        Box::pin(async move {
            debug!(
                "qBittorrent add_torrent: filename={} size={} save_path={:?} tags={:?} category={:?} paused={} dl_limit={:?} ul_limit={:?}",
                filename,
                torrent_data.len(),
                options.save_path,
                options.tags,
                options.category,
                options.paused,
                options.download_limit,
                options.upload_limit
            );
            let file_part = multipart::Part::bytes(torrent_data)
                .file_name(filename)
                .mime_str("application/x-bittorrent")
                .map_err(|e| format!("构造请求失败: {}", e))?;

            let mut form = multipart::Form::new().part("torrents", file_part);

            if let Some(path) = &options.save_path {
                form = form.text("savepath", path.clone());
            }
            if let Some(tags) = &options.tags {
                form = form.text("tags", tags.clone());
            }
            if let Some(category) = &options.category {
                form = form.text("category", category.clone());
            }
            if let Some(dl) = options.download_limit {
                form = form.text("dlLimit", dl.to_string());
            }
            if let Some(ul) = options.upload_limit {
                form = form.text("upLimit", ul.to_string());
            }
            if options.paused {
                form = form.text("paused", "true".to_string());
            }

            self.api_post_multipart("/api/v2/torrents/add", form)
                .await?;
            Ok(())
        })
    }

    fn list_torrents(
        &self,
        tag: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>> {
        let tag = tag.map(|t| t.to_string());
        Box::pin(async move {
            let path = match &tag {
                Some(t) => format!("/api/v2/torrents/info?tag={}", urlencoding::encode(t)),
                None => "/api/v2/torrents/info".to_string(),
            };

            let text = self.api_get(&path).await?;
            let items: Vec<Value> =
                serde_json::from_str(&text).map_err(|_| "解析种子列表失败".to_string())?;

            let mut result = Vec::with_capacity(items.len());
            for item in &items {
                result.push(TorrentInfo {
                    hash: item["hash"].as_str().unwrap_or_default().to_string(),
                    name: item["name"].as_str().unwrap_or_default().to_string(),
                    size: item["size"].as_i64().unwrap_or(0),
                    uploaded: item["uploaded"].as_i64().unwrap_or(0),
                    downloaded: item["downloaded"].as_i64().unwrap_or(0),
                    upload_speed: item["upspeed"].as_i64().unwrap_or(0),
                    download_speed: item["dlspeed"].as_i64().unwrap_or(0),
                    ratio: item["ratio"].as_f64().unwrap_or(0.0),
                    state: item["state"].as_str().unwrap_or_default().to_string(),
                    added_on: item["added_on"].as_i64().unwrap_or(0),
                    completion_on: item["completion_on"].as_i64().unwrap_or(0),
                    num_seeds: item["num_seeds"].as_i64().unwrap_or(0) as i32,
                    num_leechs: item["num_leechs"].as_i64().unwrap_or(0) as i32,
                    save_path: item["save_path"]
                        .as_str()
                        .or_else(|| item["content_path"].as_str())
                        .unwrap_or_default()
                        .to_string(),
                    tags: item["tags"].as_str().unwrap_or_default().to_string(),
                    category: item["category"].as_str().unwrap_or_default().to_string(),
                    time_active: item["time_active"].as_i64().unwrap_or(0),
                    last_activity: item["last_activity"].as_i64().unwrap_or(0),
                });
            }

            Ok(result)
        })
    }

    fn delete_torrent(
        &self,
        hash: &str,
        delete_files: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.api_post_form(
                "/api/v2/torrents/delete",
                &[
                    ("hashes", hash.as_str()),
                    ("deleteFiles", if delete_files { "true" } else { "false" }),
                ],
            )
            .await?;
            Ok(())
        })
    }

    fn get_free_space(
        &self,
        path: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>> {
        let _path = path.map(|p| p.to_string());
        Box::pin(async move {
            let query = "/api/v2/sync/maindata?rid=0";
            let text = self.api_get(query).await?;
            let json: Value =
                serde_json::from_str(&text).map_err(|_| "解析响应失败".to_string())?;

            let free = json
                .get("server_state")
                .and_then(|s| s.get("free_space_on_disk"))
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            Ok(free)
        })
    }
}

fn truncate_for_log(input: &str, max_len: usize) -> String {
    if input.len() <= max_len {
        input.to_string()
    } else {
        format!("{}...", &input[..max_len])
    }
}
