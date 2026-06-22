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
