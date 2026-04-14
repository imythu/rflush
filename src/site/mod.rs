pub mod mteam;
pub mod nexusphp;

use serde::{Deserialize, Serialize};

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
    pub free: bool,
    pub two_x_free: bool,
    pub hit_and_run: bool,
    pub peer_count: Option<i32>,
    pub download_volume_factor: Option<f64>,
    pub upload_volume_factor: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SiteTestResult {
    pub success: bool,
    pub message: String,
    pub user_stats: Option<UserStats>,
}

use std::future::Future;
use std::pin::Pin;

/// 站点客户端 trait — 面向接口，可扩展不同类型站点
pub trait SiteClient: Send + Sync {
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

/// 根据站点类型和配置创建对应客户端
pub fn create_site_client(
    site_type: SiteType,
    base_url: &str,
    auth: &SiteAuth,
) -> Box<dyn SiteClient> {
    match site_type {
        SiteType::NexusPhp => Box::new(nexusphp::NexusPhpClient::new(
            base_url.to_string(),
            auth.clone(),
        )),
        SiteType::MTeam => Box::new(mteam::MTeamClient::new(base_url.to_string(), auth.clone())),
    }
}
