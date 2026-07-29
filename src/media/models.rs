use super::tmdb::TmdbGenre;
use serde::{Deserialize, Serialize};

pub const MEDIA_DOWNLOAD_MAX_ATTEMPTS: u32 = 5;
pub const MEDIA_DOWNLOAD_RECONCILIATION_GRACE_SECONDS: i64 = 30;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaSettings {
    pub tmdb_token: Option<String>,
    pub tmdb_language: String,
    pub scan_interval_mins: u64,
    pub max_search_queries: usize,
    pub search_concurrency: usize,
    pub updated_at: String,
}

impl Default for MediaSettings {
    fn default() -> Self {
        Self {
            tmdb_token: None,
            tmdb_language: "zh-CN".to_string(),
            scan_interval_mins: 30,
            max_search_queries: 8,
            search_concurrency: 4,
            updated_at: String::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityProfileRecord {
    pub id: i64,
    pub name: String,
    pub resolution_order: Vec<String>,
    pub allowed_resolutions: Vec<String>,
    pub blocked_resolutions: Vec<String>,
    pub source_order: Vec<String>,
    pub allowed_sources: Vec<String>,
    pub codec_order: Vec<String>,
    pub blocked_codecs: Vec<String>,
    pub allow_unknown_quality: bool,
    pub minimum_score: i32,
    pub min_seeders: u32,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QualityProfileRequest {
    pub name: String,
    #[serde(default)]
    pub resolution_order: Vec<String>,
    #[serde(default)]
    pub allowed_resolutions: Vec<String>,
    #[serde(default)]
    pub blocked_resolutions: Vec<String>,
    #[serde(default)]
    pub source_order: Vec<String>,
    #[serde(default)]
    pub allowed_sources: Vec<String>,
    #[serde(default)]
    pub codec_order: Vec<String>,
    #[serde(default)]
    pub blocked_codecs: Vec<String>,
    #[serde(default)]
    pub allow_unknown_quality: bool,
    #[serde(default = "default_minimum_score")]
    pub minimum_score: i32,
    #[serde(default = "default_min_seeders")]
    pub min_seeders: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionRecord {
    pub id: i64,
    pub tmdb_id: i64,
    pub media_type: String,
    pub tmdb_is_animation: bool,
    pub tmdb_genres: Vec<TmdbGenre>,
    pub title: String,
    pub original_title: Option<String>,
    pub aliases: Vec<String>,
    pub year: Option<u32>,
    pub poster_path: Option<String>,
    pub season: Option<u32>,
    pub next_episode: Option<u32>,
    pub start_episode: Option<u32>,
    pub absolute_episode: Option<u32>,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    pub site_ids: Vec<i64>,
    pub save_path: Option<String>,
    pub enabled: bool,
    pub next_run_at: String,
    pub lease_owner: Option<String>,
    pub lease_until: Option<String>,
    pub version: i64,
    pub last_status: Option<String>,
    pub last_error: Option<String>,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewSubscription {
    pub tmdb_id: i64,
    pub media_type: String,
    pub tmdb_is_animation: bool,
    pub tmdb_genres: Vec<TmdbGenre>,
    pub title: String,
    pub original_title: Option<String>,
    #[serde(default)]
    pub aliases: Vec<String>,
    pub year: Option<u32>,
    pub poster_path: Option<String>,
    pub season: Option<u32>,
    pub start_episode: Option<u32>,
    pub absolute_episode: Option<u32>,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    #[serde(default)]
    pub site_ids: Vec<i64>,
    pub save_path: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateSubscription {
    pub season: Option<u32>,
    pub next_episode: Option<u32>,
    pub absolute_episode: Option<u32>,
    pub quality_profile_id: i64,
    pub downloader_id: i64,
    #[serde(default)]
    pub site_ids: Vec<i64>,
    pub save_path: Option<String>,
    pub enabled: bool,
    #[serde(default)]
    pub reset_download_history: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubscriptionTargetRecord {
    pub id: i64,
    pub subscription_id: i64,
    pub target_key: String,
    pub season: Option<u32>,
    pub episode: Option<u32>,
    pub absolute_episode: Option<u32>,
    pub air_date: Option<String>,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MediaDownloadCoverage {
    pub season: Option<u32>,
    pub episodes: Vec<u32>,
    pub absolute_episodes: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NewMediaDownload {
    pub subscription_id: Option<i64>,
    pub target_key: String,
    pub dedupe_key: String,
    pub site_id: Option<i64>,
    pub downloader_id: Option<i64>,
    pub source_site: String,
    pub downloader_name: String,
    pub torrent_id: String,
    pub title: String,
    pub size: u64,
    pub release_json: String,
    pub decision_json: String,
    pub profile_snapshot_json: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDownloadRecord {
    pub id: i64,
    pub subscription_id: Option<i64>,
    pub target_key: String,
    pub dedupe_key: String,
    pub site_id: Option<i64>,
    pub downloader_id: Option<i64>,
    pub source_site: String,
    pub downloader_name: String,
    pub torrent_id: String,
    pub title: String,
    pub size: u64,
    pub release_json: String,
    pub decision_json: String,
    pub profile_snapshot_json: String,
    pub infohash: Option<String>,
    pub status: String,
    pub attempts: u32,
    pub next_attempt_at: Option<String>,
    pub lease_owner: Option<String>,
    pub lease_until: Option<String>,
    pub version: i64,
    pub last_error: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDownloadDeletion {
    pub deleted_id: i64,
    pub subscription_id: Option<i64>,
    pub target_reopened: bool,
}

#[derive(Debug, Clone)]
pub enum MediaDownloadDeleteOutcome {
    Deleted {
        download: MediaDownloadRecord,
        target_reopened: bool,
    },
    NotFound,
    VersionChanged,
    DownloadActive,
    SubscriptionActive,
    RelocationActive,
}

pub fn target_key(
    media_type: &str,
    tmdb_id: i64,
    season: Option<u32>,
    episode: Option<u32>,
    absolute_episode: Option<u32>,
) -> String {
    if media_type.eq_ignore_ascii_case("movie") {
        return format!("movie:{tmdb_id}");
    }
    if let Some(absolute) = absolute_episode {
        return format!("tv:{tmdb_id}:abs{absolute:04}");
    }
    format!(
        "tv:{tmdb_id}:s{:02}e{:02}",
        season.unwrap_or(1),
        episode.unwrap_or(1)
    )
}

pub fn qb_media_category(target_key: &str) -> &'static str {
    if target_key.starts_with("movie:") {
        "电影"
    } else if target_key.contains(":abs") {
        "动漫"
    } else {
        "电视剧"
    }
}

/// Resolve the archive category from both the target numbering and the selected profile.
/// Some animation series use ordinary SxxExx numbering, so `:abs` alone is insufficient.
pub fn media_download_category(target_key: &str, tmdb_is_animation: bool) -> &'static str {
    let category = qb_media_category(target_key);
    if category != "电视剧" {
        return category;
    }
    if tmdb_is_animation {
        "动漫"
    } else {
        "电视剧"
    }
}

fn default_true() -> bool {
    true
}

fn default_minimum_score() -> i32 {
    80
}

fn default_min_seeders() -> u32 {
    1
}

#[cfg(test)]
mod tests {
    use super::{media_download_category, qb_media_category, target_key};

    #[test]
    fn target_keys_never_depend_on_nullable_database_columns() {
        assert_eq!(target_key("movie", 11, None, None, None), "movie:11");
        assert_eq!(target_key("tv", 22, Some(2), Some(3), None), "tv:22:s02e03");
        assert_eq!(target_key("tv", 33, None, None, Some(123)), "tv:33:abs0123");
    }

    #[test]
    fn qb_categories_follow_media_target_kind() {
        assert_eq!(qb_media_category("movie:11"), "电影");
        assert_eq!(qb_media_category("tv:22:s02e03"), "电视剧");
        assert_eq!(qb_media_category("tv:33:abs0123"), "动漫");
        assert_eq!(qb_media_category("manual:1:2"), "电视剧");
        assert_eq!(media_download_category("tv:22:s01e01", true), "动漫");
        assert_eq!(media_download_category("tv:22:s01e01", false), "电视剧");
    }
}
