pub mod scheduler;

use serde::{Deserialize, Serialize};

/// 标签匹配规则的匹配条件
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagMatchCriteria {
    /// 匹配模式: "prefix", "suffix", "contains", "exact", "regex"
    pub match_type: String,
    /// 匹配值
    pub pattern: String,
}

/// 标签规则（数据库记录）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRuleRecord {
    pub id: i64,
    pub name: String,
    pub tag_name: String,
    /// 匹配规则 JSON 数组
    pub match_rules: String,
    pub enabled: bool,
    /// 生效的下载器 ID 列表 JSON 数组，null 表示所有
    pub downloader_ids: Option<String>,
    /// 该标签当前关联的种子数
    pub tagged_torrent_count: i64,
    /// 该标签关联种子的总体积（字节）
    pub tagged_total_size: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// 创建/更新标签规则的请求体
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TagRuleRequest {
    pub name: String,
    pub tag_name: String,
    pub match_rules: Vec<TagMatchCriteria>,
    pub enabled: Option<bool>,
    /// 生效的下载器 ID 列表，None 表示所有
    pub downloader_ids: Option<Vec<i64>>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagRuleTrackerOption {
    pub domain: String,
    pub torrent_count: usize,
    pub downloader_ids: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct TagRuleTrackerDiscovery {
    pub trackers: Vec<TagRuleTrackerOption>,
    pub failed_downloaders: Vec<String>,
}

pub(crate) fn extract_tracker_domain(url: &str) -> Option<String> {
    let value = url.trim();
    if value.is_empty() || value.starts_with("**") {
        return None;
    }

    let parsed = reqwest::Url::parse(value)
        .or_else(|_| reqwest::Url::parse(&format!("http://{value}")))
        .ok()?;
    let domain = parsed.host_str()?.trim_end_matches('.').to_lowercase();
    (!domain.is_empty()).then_some(domain)
}

#[cfg(test)]
mod tests {
    use super::extract_tracker_domain;

    #[test]
    fn tracker_domain_parser_removes_credentials_ports_paths_and_case() {
        assert_eq!(
            extract_tracker_domain("HTTPS://user:pass@KP.M-TEAM.XYZ:443/announce?passkey=secret"),
            Some("kp.m-team.xyz".to_string())
        );
        assert_eq!(
            extract_tracker_domain("tracker.example.org/announce"),
            Some("tracker.example.org".to_string())
        );
        assert_eq!(
            extract_tracker_domain("udp://[2001:db8::1]:6969/announce"),
            Some("[2001:db8::1]".to_string())
        );
    }

    #[test]
    fn tracker_domain_parser_ignores_empty_and_qbittorrent_pseudo_trackers() {
        assert_eq!(extract_tracker_domain(""), None);
        assert_eq!(extract_tracker_domain("** [DHT] **"), None);
        assert_eq!(extract_tracker_domain("not a tracker"), None);
    }
}
