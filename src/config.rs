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
    pub cloakbrowser: CloakBrowserConfig,
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
