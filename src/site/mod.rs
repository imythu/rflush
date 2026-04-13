pub mod nexusphp;
pub mod mteam;

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
    pub fn as_str(self) -> &'static str {
        match self {
            SiteType::NexusPhp => "nexusphp",
            SiteType::MTeam => "mteam",
        }
    }

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
        SiteType::MTeam => Box::new(mteam::MTeamClient::new(
            base_url.to_string(),
            auth.clone(),
        )),
    }
}

pub fn find_site_for_url<'a>(sites: &'a [SiteRecord], target_url: &str) -> Option<&'a SiteRecord> {
    let target_host = extract_host(target_url)?;
    sites.iter().find(|site| {
        extract_host(&site.base_url)
            .is_some_and(|site_host| host_matches(&site_host, &target_host))
    })
}

fn extract_host(url: &str) -> Option<String> {
    let without_scheme = url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(url)
        .trim_start_matches('/');
    let host_port = without_scheme
        .split(['/', '?', '#'])
        .next()
        .unwrap_or("")
        .trim();
    if host_port.is_empty() {
        return None;
    }
    Some(
        host_port
            .split(':')
            .next()
            .unwrap_or(host_port)
            .to_ascii_lowercase(),
    )
}

fn host_matches(site_host: &str, target_host: &str) -> bool {
    site_host == target_host
        || site_host.strip_prefix("www.") == Some(target_host)
        || target_host.strip_prefix("www.") == Some(site_host)
        || site_host.ends_with(&format!(".{target_host}"))
        || target_host.ends_with(&format!(".{site_host}"))
        || root_domain(site_host) == root_domain(target_host)
}

fn root_domain(host: &str) -> String {
    let parts = host.split('.').collect::<Vec<_>>();
    if parts.len() >= 2 {
        format!("{}.{}", parts[parts.len() - 2], parts[parts.len() - 1])
    } else {
        host.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::{SiteRecord, find_site_for_url};

    #[test]
    fn matches_site_by_host_suffix() {
        let sites = vec![SiteRecord {
            id: 1,
            name: "mteam".to_string(),
            site_type: "mteam".to_string(),
            base_url: "https://api.m-team.cc".to_string(),
            auth_config: "{}".to_string(),
            created_at: "2026-01-01T00:00:00+00:00".to_string(),
            updated_at: "2026-01-01T00:00:00+00:00".to_string(),
        }];

        let site = find_site_for_url(&sites, "https://kp.m-team.cc/detail/123")
            .expect("site should match");
        assert_eq!(site.name, "mteam");
    }
}
