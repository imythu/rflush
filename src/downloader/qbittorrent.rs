use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

use qbit_rs::model::{AddTorrentArg, GetTorrentListArg, TorrentSource, TorrentFile};
use qbit_rs::Qbit;
use reqwest::Client;
use tracing::debug;

use super::{AddTorrentOptions, DownloaderClient, DownloaderTestResult, TorrentInfo};

pub struct QBittorrentClient {
    qb: Qbit,
}

impl QBittorrentClient {
    pub fn new(
        base_url: String,
        username: String,
        password: String,
        proxy: Option<&str>,
    ) -> Result<Self, String> {
        let mut builder = Client::builder()
            .timeout(Duration::from_secs(30))
            .redirect(reqwest::redirect::Policy::none());
        if let Some(proxy_url) = proxy.map(str::trim).filter(|v| !v.is_empty()) {
            let proxy = reqwest::Proxy::all(proxy_url)
                .map_err(|e| format!("无效的代理地址 '{}': {}", proxy_url, e))?;
            builder = builder.proxy(proxy);
        }
        let client = builder
            .build()
            .map_err(|e| format!("构建 qBittorrent HTTP 客户端失败: {}", e))?;

        let endpoint = base_url.trim_end_matches('/');
        let credential = qbit_rs::model::Credential::new(username, password);
        let qb = Qbit::new_with_client(endpoint, credential, client);

        Ok(Self { qb })
    }
}

impl DownloaderClient for QBittorrentClient {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>> {
        Box::pin(async move {
            self.qb.login(false).await.map_err(|e| format!("登录失败: {}", e))?;
            let version = self
                .qb
                .get_version()
                .await
                .map_err(|e| format!("获取版本失败: {}", e))?;
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
                "qBittorrent add_torrent: filename={} size={} save_path={:?} tags={:?} category={:?} paused={}",
                filename,
                torrent_data.len(),
                options.save_path,
                options.tags,
                options.category,
                options.paused,
            );

            let arg = AddTorrentArg {
                source: TorrentSource::TorrentFiles {
                    torrents: vec![TorrentFile {
                        filename,
                        data: torrent_data,
                    }],
                },
                savepath: options.save_path,
                tags: options.tags,
                category: options.category,
                download_limit: options.download_limit,
                up_limit: options.upload_limit,
                ratio_limit: options.ratio_limit,
                seeding_time_limit: options.inactive_seeding_time_limit,
                paused: if options.paused {
                    Some("true".to_string())
                } else {
                    None
                },
                ..Default::default()
            };

            self.qb
                .add_torrent(arg)
                .await
                .map_err(|e| format!("添加种子失败: {}", e))?;
            Ok(())
        })
    }

    fn list_torrents(
        &self,
        tag: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>> {
        let tag = tag.map(|t| t.to_string());
        Box::pin(async move {
            let arg = GetTorrentListArg {
                tag,
                ..Default::default()
            };
            let torrents = self
                .qb
                .get_torrent_list(arg)
                .await
                .map_err(|e| format!("获取种子列表失败: {}", e))?;
            Ok(torrents.into_iter().map(TorrentInfo::from).collect())
        })
    }

    fn list_torrents_by_hashes<'a>(
        &'a self,
        hashes: &'a [String],
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + 'a>> {
        Box::pin(async move {
            if hashes.is_empty() {
                return Ok(Vec::new());
            }
            let joined_hashes = hashes.join("|");
            let arg = GetTorrentListArg {
                hashes: Some(joined_hashes),
                ..Default::default()
            };
            let torrents = self
                .qb
                .get_torrent_list(arg)
                .await
                .map_err(|e| format!("获取种子列表失败: {}", e))?;
            Ok(torrents.into_iter().map(TorrentInfo::from).collect())
        })
    }

    fn pause_torrent(
        &self,
        hash: &str,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .stop_torrents(vec![hash])
                .await
                .map_err(|e| format!("暂停种子失败: {}", e))?;
            Ok(())
        })
    }

    fn delete_torrent(
        &self,
        hash: &str,
        delete_files: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        let hash = hash.to_string();
        Box::pin(async move {
            self.qb
                .delete_torrents(vec![hash], delete_files)
                .await
                .map_err(|e| format!("删除种子失败: {}", e))?;
            Ok(())
        })
    }

    fn get_free_space(
        &self,
        _path: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>> {
        Box::pin(async move {
            let sync_data = self
                .qb
                .sync(None)
                .await
                .map_err(|e| format!("获取同步数据失败: {}", e))?;

            let free = sync_data
                .server_state
                .as_ref()
                .and_then(|s| s.get("free_space_on_disk"))
                .and_then(|v| {
                    // serde_value::Value -> u64
                    serde_json::to_value(v).ok()?.as_u64()
                })
                .unwrap_or(0);

            Ok(free)
        })
    }
}

impl From<qbit_rs::model::Torrent> for TorrentInfo {
    fn from(t: qbit_rs::model::Torrent) -> Self {
        TorrentInfo {
            hash: t.hash.unwrap_or_default(),
            name: t.name.unwrap_or_default(),
            size: t.size.unwrap_or(0),
            uploaded: t.uploaded.unwrap_or(0),
            downloaded: t.downloaded.unwrap_or(0),
            upload_speed: t.upspeed.unwrap_or(0),
            download_speed: t.dlspeed.unwrap_or(0),
            ratio: t.ratio.unwrap_or(0.0),
            state: t
                .state
                .and_then(|s| serde_json::to_value(&s).ok())
                .and_then(|v| v.as_str().map(|s| s.to_string()))
                .unwrap_or_default(),
            added_on: t.added_on.unwrap_or(0),
            completion_on: t.completion_on.unwrap_or(0),
            num_seeds: t.num_seeds.unwrap_or(0) as i32,
            num_leechs: t.num_leechs.unwrap_or(0) as i32,
            save_path: t
                .save_path
                .clone()
                .or_else(|| t.content_path.clone())
                .unwrap_or_default(),
            tags: t.tags.unwrap_or_default(),
            category: t.category.unwrap_or_default(),
            time_active: t.time_active.unwrap_or(0),
            last_activity: t.last_activity.unwrap_or(0),
        }
    }
}
