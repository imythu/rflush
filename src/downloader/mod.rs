pub mod qbittorrent;

use serde::{Deserialize, Serialize};

/// 下载器类型
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DownloaderType {
    QBittorrent,
}

impl DownloaderType {
    pub fn as_str(self) -> &'static str {
        match self {
            DownloaderType::QBittorrent => "qbittorrent",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
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

use std::future::Future;
use std::pin::Pin;

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
}

pub fn create_downloader_client(
    downloader_type: DownloaderType,
    url: &str,
    username: &str,
    password: &str,
) -> Box<dyn DownloaderClient> {
    match downloader_type {
        DownloaderType::QBittorrent => Box::new(qbittorrent::QBittorrentClient::new(
            url.to_string(),
            username.to_string(),
            password.to_string(),
        )),
    }
}
