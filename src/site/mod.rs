pub mod factory;
pub mod mteam;
pub mod nexusphp;

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

/// PT 站点认证配置
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "auth_type")]
pub enum SiteAuth {
    #[serde(rename = "cookie")]
    Cookie { cookie: String },
    #[serde(rename = "passkey")]
    Passkey { passkey: String },
    #[serde(rename = "cookie_passkey")]
    CookiePasskey { cookie: String, passkey: String },
    #[serde(rename = "api_key")]
    ApiKey { api_key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SiteType {
    NexusPhp,
    MTeam,
}

impl SiteType {
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "nexusphp" | "nexus_php" => Some(SiteType::NexusPhp),
            "mteam" | "m_team" => Some(SiteType::MTeam),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UserStats {
    pub username: String,
    pub uploaded: u64,
    pub downloaded: u64,
    pub ratio: Option<f64>,
    pub bonus: Option<f64>,
    pub seeding_count: Option<u32>,
    pub leeching_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteRecord {
    pub id: i64,
    pub name: String,
    pub site_type: String,
    pub base_url: String,
    pub auth_config: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TorrentAttributes {
    /// 是否为免费种。
    pub free: bool,
    /// 是否为免费且双倍上传的种子。
    pub two_x_free: bool,
    /// 是否命中 H&R 规则。
    pub hit_and_run: bool,
    /// 做种数。
    pub seeder_count: Option<i32>,
    /// Free 促销结束时间的 Unix 时间戳（秒）。
    pub free_end_timestamp: Option<i64>,
    /// 下载系数，`0.0` 表示免费，`1.0` 表示原价下载。
    pub download_volume_factor: Option<f64>,
    /// 上传系数，`1.0` 表示原价上传，`2.0` 表示双倍上传。
    pub upload_volume_factor: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteTestResult {
    pub success: bool,
    pub message: String,
    pub user_stats: Option<UserStats>,
}

/// 站点适配器 trait
pub trait SiteAdapter: Send + Sync {
    fn test_connection(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<SiteTestResult, String>> + Send + '_>>;

    fn get_user_stats(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<UserStats, String>> + Send + '_>>;

    fn get_torrent_attributes(
        &self,
        detail_url: &str,
    ) -> Pin<Box<dyn Future<Output = Result<TorrentAttributes, String>> + Send + '_>>;
}
