use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GlobalConfig {
    pub log_level: Option<String>,
    #[serde(default)]
    pub proxy: Option<String>,
    #[serde(default = "default_true")]
    pub use_proxy_for_lightpanda: bool,
    #[serde(default)]
    pub lightpanda: LightpandaConfig,
    #[serde(default)]
    pub browserless: BrowserlessConfig,
    #[serde(default = "default_tag_rule_scan_interval_mins")]
    pub tag_rule_scan_interval_mins: u64,
    #[serde(default)]
    pub ocr_api_key: Option<String>,
}

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            log_level: Some("info".to_string()),
            proxy: None,
            use_proxy_for_lightpanda: true,
            lightpanda: LightpandaConfig::default(),
            browserless: BrowserlessConfig::default(),
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

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BrowserlessConfig {
    #[serde(default)]
    pub address: Option<String>,
    #[serde(default)]
    pub token: Option<String>,
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

const fn default_tag_rule_scan_interval_mins() -> u64 {
    7
}
