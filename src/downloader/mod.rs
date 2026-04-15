pub mod factory;
pub mod qbittorrent;

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

/// 下载器类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloaderType {
    QBittorrent,
}

impl DownloaderType {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "qbittorrent" | "qb" => Some(DownloaderType::QBittorrent),
            _ => None,
        }
    }
}

/// 下载器数据库记录
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloaderRecord {
    pub id: i64,
    pub name: String,
    pub downloader_type: String,
    pub url: String,
    pub username: String,
    pub password: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 添加种子的选项
#[derive(Debug, Clone, Default)]
pub struct AddTorrentOptions {
    pub save_path: Option<String>,
    pub tags: Option<String>,
    pub category: Option<String>,
    pub download_limit: Option<i64>,
    pub upload_limit: Option<i64>,
    pub paused: bool,
}

/// 下载器中的种子信息
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TorrentInfo {
    pub hash: String,
    pub name: String,
    pub size: i64,
    pub uploaded: i64,
    pub downloaded: i64,
    pub upload_speed: i64,
    pub download_speed: i64,
    pub ratio: f64,
    pub state: String,
    pub added_on: i64,
    pub completion_on: i64,
    pub num_seeds: i32,
    pub num_leechs: i32,
    pub save_path: String,
    pub tags: String,
    pub category: String,
    pub time_active: i64,
    pub last_activity: i64,
}

/// 下载器测试结果
#[derive(Debug, Clone, Serialize)]
pub struct DownloaderTestResult {
    pub success: bool,
    pub message: String,
    pub version: Option<String>,
    pub free_space: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DownloaderSpaceStats {
    pub free_space: u64,
    pub pending_download_bytes: u64,
    pub effective_free_space: u64,
    pub torrent_count: usize,
    pub incomplete_count: usize,
}

/// 下载器客户端 trait — 通用接口，可扩展支持不同下载器
pub trait DownloaderClient: Send + Sync {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderTestResult, String>> + Send + '_>>;

    fn add_torrent(
        &self,
        torrent_data: Vec<u8>,
        filename: &str,
        options: &AddTorrentOptions,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn list_torrents(
        &self,
        tag: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<TorrentInfo>, String>> + Send + '_>>;

    fn delete_torrent(
        &self,
        hash: &str,
        delete_files: bool,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>>;

    fn get_free_space(
        &self,
        path: Option<&str>,
    ) -> Pin<Box<dyn Future<Output = Result<u64, String>> + Send + '_>>;

    fn get_effective_free_space<'a>(
        &'a self,
        path: Option<&'a str>,
        torrents: &'a [TorrentInfo],
    ) -> Pin<Box<dyn Future<Output = Result<DownloaderSpaceStats, String>> + Send + 'a>> {
        Box::pin(async move {
            let free_space = self.get_free_space(path).await?;
            let pending_download_bytes = calculate_pending_download_bytes(torrents);
            let incomplete_count = torrents
                .iter()
                .filter(|torrent| torrent.completion_on <= 0 && torrent.downloaded < torrent.size)
                .count();

            Ok(DownloaderSpaceStats {
                free_space,
                pending_download_bytes,
                effective_free_space: free_space.saturating_sub(pending_download_bytes),
                torrent_count: torrents.len(),
                incomplete_count,
            })
        })
    }
}

pub fn calculate_pending_download_bytes(torrents: &[TorrentInfo]) -> u64 {
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
