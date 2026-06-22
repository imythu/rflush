use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub global: GlobalConfig,
    #[serde(default)]
    pub rss: Vec<RssConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    #[serde(default)]
    pub download_rate_limit: DownloadRateLimit,
    #[serde(default = "default_retry_interval_secs")]
    pub retry_interval_secs: u64,
    pub log_level: Option<String>,
    #[serde(default = "default_max_concurrent_downloads")]
    pub max_concurrent_downloads: usize,
    #[serde(default = "default_max_concurrent_rss_fetches")]
    pub max_concurrent_rss_fetches: usize,
    #[serde(default = "default_throttle_interval_secs")]
    pub throttle_interval_secs: u64,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default = "default_true")]
    pub use_proxy_for_lightpanda: bool,
    #[serde(default = "default_tag_rule_scan_interval_mins")]
    pub tag_rule_scan_interval_mins: u64,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            download_rate_limit: DownloadRateLimit::default(),
            retry_interval_secs: default_retry_interval_secs(),
            log_level: Some("info".to_string()),
            max_concurrent_downloads: default_max_concurrent_downloads(),
            max_concurrent_rss_fetches: default_max_concurrent_rss_fetches(),
            throttle_interval_secs: default_throttle_interval_secs(),
            proxy: None,
            use_proxy_for_lightpanda: true,
            tag_rule_scan_interval_mins: default_tag_rule_scan_interval_mins(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRateLimit {
    #[serde(default = "default_download_rate_limit_requests")]
    pub requests: u32,
    #[serde(default = "default_download_rate_limit_interval")]
    pub interval: u64,
    #[serde(default)]
    pub unit: TimeUnit,
}

impl Default for DownloadRateLimit {
    fn default() -> Self {
        Self {
            requests: default_download_rate_limit_requests(),
            interval: default_download_rate_limit_interval(),
            unit: TimeUnit::default(),
        }
    }
}

impl DownloadRateLimit {
    pub fn interval_duration(&self) -> Duration {
        self.unit.duration(self.interval)
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TimeUnit {
    #[default]
    Second,
    Minute,
    Hour,
}

impl TimeUnit {
    pub fn duration(self, interval: u64) -> Duration {
        match self {
            TimeUnit::Second => Duration::from_secs(interval),
            TimeUnit::Minute => Duration::from_secs(interval.saturating_mul(60)),
            TimeUnit::Hour => Duration::from_secs(interval.saturating_mul(60 * 60)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssConfig {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub downloader_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RssSubscription {
    pub id: i64,
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub downloader_id: Option<i64>,
    pub created_at: String,
    pub updated_at: String,
}

const fn default_download_rate_limit_requests() -> u32 {
    2
}

const fn default_download_rate_limit_interval() -> u64 {
    1
}

const fn default_retry_interval_secs() -> u64 {
    5
}

const fn default_max_concurrent_downloads() -> usize {
    32
}

const fn default_max_concurrent_rss_fetches() -> usize {
    8
}

const fn default_throttle_interval_secs() -> u64 {
    30
}

const fn default_true() -> bool {
    true
}

const fn default_tag_rule_scan_interval_mins() -> u64 {
    7
}
