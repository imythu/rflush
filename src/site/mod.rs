pub mod factory;
pub mod mteam;
pub mod nexusphp;
pub mod u2_shoutbox;

use serde::{Deserialize, Serialize};
use std::future::Future;
use std::pin::Pin;

use reqwest::header::{HeaderMap, HeaderName, HeaderValue};

const MAX_REQUEST_HEADERS: usize = 64;
const MAX_REQUEST_HEADER_NAME_BYTES: usize = 128;
const MAX_REQUEST_HEADER_VALUE_BYTES: usize = 8 * 1024;
const MAX_REQUEST_HEADERS_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SiteRequestHeader {
    pub name: String,
    pub value: String,
}

pub fn default_site_request_headers() -> Vec<SiteRequestHeader> {
    [
        (
            "Accept",
            "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,image/apng,*/*;q=0.8,application/signed-exchange;v=b3;q=0.7",
        ),
        ("Accept-Encoding", "gzip, deflate, br, zstd"),
        (
            "Accept-Language",
            "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7,zh-TW;q=0.6",
        ),
        ("DNT", "1"),
        (
            "User-Agent",
            "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/151.0.0.0 Safari/537.36",
        ),
        (
            "sec-ch-ua",
            "\"Not=A?Brand\";v=\"99\", \"Google Chrome\";v=\"151\", \"Chromium\";v=\"151\"",
        ),
        ("sec-ch-ua-arch", "\"x86\""),
        ("sec-ch-ua-bitness", "\"64\""),
        ("sec-ch-ua-full-version", "\"151.0.7922.109\""),
        (
            "sec-ch-ua-full-version-list",
            "\"Not=A?Brand\";v=\"99.0.0.0\", \"Google Chrome\";v=\"151.0.7922.109\", \"Chromium\";v=\"151.0.7922.109\"",
        ),
        ("sec-ch-ua-mobile", "?0"),
        ("sec-ch-ua-model", "\"\""),
        ("sec-ch-ua-platform", "\"Windows\""),
        ("sec-ch-ua-platform-version", "\"19.0.0\""),
    ]
    .into_iter()
    .map(|(name, value)| SiteRequestHeader {
        name: name.to_string(),
        value: value.to_string(),
    })
    .collect()
}

pub fn normalize_site_request_headers(
    headers: Vec<SiteRequestHeader>,
) -> Result<Vec<SiteRequestHeader>, String> {
    if headers.len() > MAX_REQUEST_HEADERS {
        return Err(format!("自定义请求头最多允许 {MAX_REQUEST_HEADERS} 项"));
    }

    let mut normalized = Vec::with_capacity(headers.len());
    let mut parsed = HeaderMap::new();
    let mut total_bytes = 0usize;
    for (index, header) in headers.into_iter().enumerate() {
        let name = header.name.trim();
        let value = header.value.trim();
        if name.is_empty() {
            return Err(format!("第 {} 个请求头名称不能为空", index + 1));
        }
        if name.len() > MAX_REQUEST_HEADER_NAME_BYTES {
            return Err(format!("请求头 {name} 的名称过长"));
        }
        if value.len() > MAX_REQUEST_HEADER_VALUE_BYTES {
            return Err(format!("请求头 {name} 的值过长"));
        }

        let parsed_name = HeaderName::from_bytes(name.as_bytes())
            .map_err(|_| format!("请求头名称无效: {name}"))?;
        if is_managed_request_header(&parsed_name) {
            return Err(format!("请求头 {name} 由 HTTP 客户端管理，不能自定义"));
        }
        if parsed.contains_key(&parsed_name) {
            return Err(format!("请求头名称不能重复: {name}"));
        }

        let mut parsed_value =
            HeaderValue::from_str(value).map_err(|_| format!("请求头 {name} 的值包含无效字符"))?;
        parsed_value.set_sensitive(true);
        parsed.insert(parsed_name, parsed_value);

        total_bytes = total_bytes.saturating_add(name.len() + value.len());
        if total_bytes > MAX_REQUEST_HEADERS_BYTES {
            return Err("自定义请求头总大小不能超过 64 KiB".to_string());
        }
        normalized.push(SiteRequestHeader {
            name: name.to_string(),
            value: value.to_string(),
        });
    }
    Ok(normalized)
}

pub fn parse_site_request_headers(raw: &str) -> Result<Vec<SiteRequestHeader>, String> {
    let headers: Vec<SiteRequestHeader> =
        serde_json::from_str(raw).map_err(|error| format!("请求头配置解析失败: {error}"))?;
    normalize_site_request_headers(headers)
}

pub fn site_request_header_map(headers: &[SiteRequestHeader]) -> Result<HeaderMap, String> {
    let mut map = HeaderMap::new();
    for header in headers {
        let name = HeaderName::from_bytes(header.name.as_bytes())
            .map_err(|_| format!("请求头名称无效: {}", header.name))?;
        let mut value = HeaderValue::from_str(&header.value)
            .map_err(|_| format!("请求头 {} 的值包含无效字符", header.name))?;
        value.set_sensitive(true);
        map.insert(name, value);
    }
    Ok(map)
}

/// Keep only browser fingerprint headers that are safe to forward to another origin.
pub fn browser_request_header_map(headers: &HeaderMap) -> HeaderMap {
    headers
        .iter()
        .filter(|(name, _)| is_browser_request_header(name.as_str()))
        .map(|(name, value)| (name.clone(), value.clone()))
        .collect()
}

fn is_browser_request_header(name: &str) -> bool {
    matches!(
        name,
        "accept"
            | "accept-encoding"
            | "accept-language"
            | "cache-control"
            | "dnt"
            | "pragma"
            | "upgrade-insecure-requests"
            | "user-agent"
    ) || name.starts_with("sec-ch-ua")
        || name.starts_with("sec-fetch-")
}

fn is_managed_request_header(name: &HeaderName) -> bool {
    matches!(
        name.as_str(),
        "connection"
            | "content-length"
            | "host"
            | "keep-alive"
            | "proxy-connection"
            | "te"
            | "trailer"
            | "transfer-encoding"
            | "upgrade"
    )
}

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
    pub uid: Option<String>,
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
    pub request_headers: String,
    pub use_proxy: bool,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteStatsRecord {
    pub site_id: i64,
    pub uid: Option<String>,
    pub username: Option<String>,
    pub uploaded: Option<u64>,
    pub downloaded: Option<u64>,
    pub ratio: Option<f64>,
    pub bonus: Option<f64>,
    pub seeding_count: Option<u32>,
    pub leeching_count: Option<u32>,
    pub updated_at: Option<String>,
    pub last_checked_at: String,
    pub last_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SiteWithStats {
    pub id: i64,
    pub name: String,
    pub site_type: String,
    pub base_url: String,
    pub auth_config: String,
    pub request_headers: String,
    pub use_proxy: bool,
    pub created_at: String,
    pub updated_at: String,
    pub stats: Option<SiteStatsRecord>,
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
    /// 下载数（leechers）。
    pub leecher_count: Option<i32>,
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

#[cfg(test)]
mod tests {
    use super::{
        SiteRequestHeader, browser_request_header_map, default_site_request_headers,
        normalize_site_request_headers, site_request_header_map,
    };

    fn header(name: &str, value: &str) -> SiteRequestHeader {
        SiteRequestHeader {
            name: name.to_string(),
            value: value.to_string(),
        }
    }

    #[test]
    fn browser_defaults_are_valid_and_exclude_connection_specific_values() {
        let defaults = default_site_request_headers();
        let map = site_request_header_map(&defaults).unwrap();

        assert_eq!(map["dnt"], "1");
        assert!(map["accept"].to_str().unwrap().contains("text/html"));
        assert!(map["user-agent"].to_str().unwrap().contains("Chrome/151"));
        assert!(!map.contains_key("cookie"));
        assert!(!map.contains_key("host"));
        assert!(!map.contains_key("connection"));
    }

    #[test]
    fn request_header_validation_rejects_duplicates_managed_headers_and_newlines() {
        assert!(
            normalize_site_request_headers(vec![header("X-Test", "one"), header("x-test", "two")])
                .unwrap_err()
                .contains("不能重复")
        );
        assert!(
            normalize_site_request_headers(vec![header("Host", "tracker.example")])
                .unwrap_err()
                .contains("HTTP 客户端管理")
        );
        assert!(
            normalize_site_request_headers(vec![header("X-Test", "one\r\ntwo")])
                .unwrap_err()
                .contains("无效字符")
        );
    }

    #[test]
    fn browser_header_filter_drops_private_cross_origin_values() {
        let headers = site_request_header_map(&[
            header("User-Agent", "Configured Browser"),
            header("sec-ch-ua", "Chromium"),
            header("X-Private-Token", "secret"),
        ])
        .unwrap();

        let filtered = browser_request_header_map(&headers);

        assert_eq!(filtered["user-agent"], "Configured Browser");
        assert_eq!(filtered["sec-ch-ua"], "Chromium");
        assert!(!filtered.contains_key("x-private-token"));
    }
}
