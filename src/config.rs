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
    #[serde(default)]
    pub lightpanda: LightpandaConfig,
    #[serde(default)]
    pub cloakbrowser: CloakBrowserConfig,
    #[serde(default = "default_tag_rule_scan_interval_mins")]
    pub tag_rule_scan_interval_mins: u64,
    #[serde(default)]
    pub ocr_api_key: Option<String>,
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
            lightpanda: LightpandaConfig::default(),
            cloakbrowser: CloakBrowserConfig::default(),
            tag_rule_scan_interval_mins: default_tag_rule_scan_interval_mins(),
            ocr_api_key: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LightpandaConfig {
    #[serde(default)]
    pub endpoint: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
    #[serde(default = "default_lightpanda_region")]
    pub region: String,
    #[serde(default = "default_lightpanda_browser")]
    pub browser: String,
    #[serde(default = "default_lightpanda_proxy")]
    pub proxy: Option<String>,
    #[serde(default)]
    pub country: Option<String>,
}

impl Default for LightpandaConfig {
    fn default() -> Self {
        Self {
            endpoint: None,
            token: None,
            region: default_lightpanda_region(),
            browser: default_lightpanda_browser(),
            proxy: default_lightpanda_proxy(),
            country: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloakBrowserConfig {
    #[serde(default)]
    pub license_key: Option<String>,
    #[serde(default)]
    pub headless: bool,
    #[serde(default = "default_true")]
    pub humanize: bool,
    #[serde(default = "default_cloakbrowser_human_preset")]
    pub human_preset: String,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default = "default_true")]
    pub geoip: bool,
}

impl Default for CloakBrowserConfig {
    fn default() -> Self {
        Self {
            license_key: None,
            headless: false,
            humanize: true,
            human_preset: default_cloakbrowser_human_preset(),
            proxy: None,
            geoip: true,
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

fn default_lightpanda_region() -> String {
    "euwest".to_string()
}

fn default_lightpanda_browser() -> String {
    "lightpanda".to_string()
}

fn default_lightpanda_proxy() -> Option<String> {
    Some("fast_dc".to_string())
}

fn default_cloakbrowser_human_preset() -> String {
    "careful".to_string()
}

const fn default_tag_rule_scan_interval_mins() -> u64 {
    7
}
