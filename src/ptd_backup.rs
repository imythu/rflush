use std::collections::BTreeMap;
use std::io::{Cursor, Write};

use chrono::{DateTime, Duration, Utc};
use reqwest::{Client, Method, RequestBuilder, Url};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;
use tracing::{info, warn};
use zip::write::SimpleFileOptions;

use crate::db::{Database, PtdBackupConfig};
use crate::net::client_factory;
use crate::site::{SiteStatsRecord, SiteWithStats};

const MIN_BACKUP_INTERVAL_HOURS: u64 = 1;
const MAX_BACKUP_INTERVAL_HOURS: u64 = 24 * 30;
const PTD_SUCCESS_STATUS: u8 = 3;
static BACKUP_LOCK: Mutex<()> = Mutex::const_new(());

#[derive(Debug, Clone, Serialize)]
pub struct PtdBackupRunResult {
    pub filename: String,
    pub site_count: usize,
    pub size: usize,
    pub backed_up_at: String,
}

#[derive(Debug, Serialize)]
struct PtdManifestFile {
    name: &'static str,
    hash: String,
}

#[derive(Debug, Serialize)]
struct PtdManifest {
    time: i64,
    version: String,
    encryption: bool,
    files: BTreeMap<&'static str, PtdManifestFile>,
}

pub fn validate_config(config: &mut PtdBackupConfig) -> Result<(), String> {
    if config.webdav_url.trim().is_empty() {
        if config.enabled {
            return Err("启用蜂巢 PTD 备份前必须填写 WebDAV 地址".to_string());
        }
        config.webdav_url.clear();
    } else {
        config.webdav_url = normalize_webdav_url(&config.webdav_url)?;
    }
    config.username = config.username.trim().to_string();
    if !(MIN_BACKUP_INTERVAL_HOURS..=MAX_BACKUP_INTERVAL_HOURS)
        .contains(&config.backup_interval_hours)
    {
        return Err(format!(
            "备份周期必须在 {MIN_BACKUP_INTERVAL_HOURS} 到 {MAX_BACKUP_INTERVAL_HOURS} 小时之间"
        ));
    }

    Ok(())
}

pub fn ptd_site_id(site: &SiteWithStats) -> Option<&'static str> {
    if matches!(site.site_type.as_str(), "mteam" | "m_team") {
        return Some("mteam");
    }

    Url::parse(&site.base_url)
        .ok()
        .and_then(|url| url.host_str().map(str::to_ascii_lowercase))
        .and_then(|host| crate::ptd_sites::site_id_for_host(&host))
}

pub async fn test_webdav(config: &PtdBackupConfig, proxy: Option<&str>) -> Result<(), String> {
    let url = normalize_webdav_url(&config.webdav_url)?;
    let client = build_client(config.use_proxy.then_some(proxy).flatten())?;
    let method = Method::from_bytes(b"PROPFIND").map_err(|error| error.to_string())?;
    let request = client
        .request(method, &url)
        .header("Depth", "0")
        .header("Content-Type", "application/xml; charset=utf-8")
        .body(
            r#"<?xml version="1.0" encoding="utf-8" ?><d:propfind xmlns:d="DAV:"><d:prop><d:resourcetype/></d:prop></d:propfind>"#,
        );
    let response = with_basic_auth(request, config)
        .send()
        .await
        .map_err(|error| format!("连接 WebDAV 失败: {error}"))?;
    if response.status().is_success() || response.status().as_u16() == 207 {
        return Ok(());
    }
    Err(format!("WebDAV 返回 HTTP {}", response.status()))
}

pub async fn backup_now(db: &Database) -> Result<PtdBackupRunResult, String> {
    let _guard = BACKUP_LOCK.lock().await;
    backup_locked(db).await
}

pub async fn backup_if_due(db: &Database) {
    let Ok(config) = db.get_ptd_backup_config().await else {
        warn!("failed to load Hive PTD backup config");
        return;
    };
    if !config.enabled || !is_due(&config) {
        return;
    }

    let Ok(_guard) = BACKUP_LOCK.try_lock() else {
        return;
    };
    let Ok(current) = db.get_ptd_backup_config().await else {
        return;
    };
    if !current.enabled || !is_due(&current) {
        return;
    }
    match backup_locked(db).await {
        Ok(result) => info!(
            filename = %result.filename,
            site_count = result.site_count,
            "Hive PTD automatic backup completed"
        ),
        Err(error) => warn!(%error, "Hive PTD automatic backup failed"),
    }
}

async fn backup_locked(db: &Database) -> Result<PtdBackupRunResult, String> {
    let result = perform_backup(db).await;
    if let Err(error) = result.as_ref() {
        let _ = db.record_ptd_backup_error(error).await;
    }
    result
}

async fn perform_backup(db: &Database) -> Result<PtdBackupRunResult, String> {
    let mut config = db
        .get_ptd_backup_config()
        .await
        .map_err(|error| error.to_string())?;
    let sites = db
        .list_sites_with_stats()
        .await
        .map_err(|error| error.to_string())?;
    validate_config(&mut config)?;
    if config.webdav_url.is_empty() {
        return Err("请先配置 WebDAV 地址".to_string());
    }

    let now = Utc::now();
    let (archive, site_count) = build_ptd_archive(&sites, now)?;
    if site_count == 0 {
        return Err("没有可备份的已识别 PTD 站点用户信息，请检查站点域名并刷新统计".to_string());
    }

    let filename = format!("PTD_backup_{}.zip", now.format("%Y%m%dT%H%M"));
    let upload_url = webdav_file_url(&config.webdav_url, &filename)?;
    let settings = db.get_settings().await.map_err(|error| error.to_string())?;
    let proxy = config
        .use_proxy
        .then_some(settings.proxy.as_deref())
        .flatten();
    let client = build_client(proxy)?;
    let archive_size = archive.len();
    let request = client
        .put(upload_url)
        .header("Content-Type", "application/zip")
        .body(archive);
    let response = match with_basic_auth(request, &config).send().await {
        Ok(response) => response,
        Err(error) => {
            return Err(format!("上传 WebDAV 失败: {error}"));
        }
    };
    if !response.status().is_success() {
        return Err(format!("上传 WebDAV 失败: HTTP {}", response.status()));
    }

    let backed_up_at = now.to_rfc3339();
    db.record_ptd_backup_success(&filename, &backed_up_at)
        .await
        .map_err(|error| error.to_string())?;
    Ok(PtdBackupRunResult {
        filename,
        site_count,
        size: archive_size,
        backed_up_at,
    })
}

fn build_ptd_archive(
    sites: &[SiteWithStats],
    now: DateTime<Utc>,
) -> Result<(Vec<u8>, usize), String> {
    let mut user_info = Map::new();
    for site in sites {
        let Some(stats) = site.stats.as_ref().filter(|stats| has_user_stats(stats)) else {
            continue;
        };
        let Some(ptd_site_id) = ptd_site_id(site) else {
            continue;
        };
        let update_at = stats
            .updated_at
            .as_deref()
            .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
            .map(|value| value.with_timezone(&Utc))
            .unwrap_or(now);
        let snapshot = ptd_user_snapshot(ptd_site_id, stats, update_at.timestamp_millis());
        user_info.insert(
            ptd_site_id.to_string(),
            json!({ update_at.format("%Y-%m-%d").to_string(): snapshot }),
        );
    }

    let site_count = user_info.len();
    let user_info_bytes = serde_json::to_vec(&Value::Object(user_info))
        .map_err(|error| format!("生成 userInfo.json 失败: {error}"))?;
    let user_info_hash = format!("{:x}", md5::compute(&user_info_bytes));
    let manifest = PtdManifest {
        time: now.timestamp_millis(),
        version: format!("PT-Depiler (rflush {})", env!("CARGO_PKG_VERSION")),
        encryption: false,
        files: BTreeMap::from([(
            "userInfo",
            PtdManifestFile {
                name: "userInfo.json",
                hash: user_info_hash,
            },
        )]),
    };
    let manifest_bytes = serde_json::to_vec(&manifest)
        .map_err(|error| format!("生成 manifest.json 失败: {error}"))?;

    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(9));
    zip.start_file("userInfo.json", options)
        .map_err(|error| format!("创建 PTD ZIP 失败: {error}"))?;
    zip.write_all(&user_info_bytes)
        .map_err(|error| format!("写入 userInfo.json 失败: {error}"))?;
    zip.start_file("manifest.json", options)
        .map_err(|error| format!("创建 PTD ZIP 失败: {error}"))?;
    zip.write_all(&manifest_bytes)
        .map_err(|error| format!("写入 manifest.json 失败: {error}"))?;
    let archive = zip
        .finish()
        .map_err(|error| format!("完成 PTD ZIP 失败: {error}"))?
        .into_inner();
    Ok((archive, site_count))
}

fn ptd_user_snapshot(site_id: &str, stats: &SiteStatsRecord, update_at: i64) -> Value {
    let mut snapshot = Map::from_iter([
        ("status".to_string(), json!(PTD_SUCCESS_STATUS)),
        ("updateAt".to_string(), json!(update_at)),
        ("site".to_string(), json!(site_id)),
        (
            "name".to_string(),
            json!(stats.username.as_deref().unwrap_or_default()),
        ),
        (
            "uploaded".to_string(),
            json!(stats.uploaded.unwrap_or_default()),
        ),
        (
            "downloaded".to_string(),
            json!(stats.downloaded.unwrap_or_default()),
        ),
        ("messageCount".to_string(), json!(0)),
    ]);
    if let Some(uid) = stats.uid.as_deref() {
        snapshot.insert("id".to_string(), json!(uid));
    }
    if let Some(ratio) = stats.ratio.filter(|value| value.is_finite()) {
        snapshot.insert("ratio".to_string(), json!(ratio));
    }
    if let Some(bonus) = stats.bonus.filter(|value| value.is_finite()) {
        snapshot.insert("bonus".to_string(), json!(bonus));
    }
    if let Some(seeding) = stats.seeding_count {
        snapshot.insert("seeding".to_string(), json!(seeding));
    }
    if let Some(leeching) = stats.leeching_count {
        snapshot.insert("leeching".to_string(), json!(leeching));
    }
    Value::Object(snapshot)
}

fn has_user_stats(stats: &SiteStatsRecord) -> bool {
    stats
        .username
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
        && stats.uploaded.is_some()
        && stats.downloaded.is_some()
}

fn normalize_webdav_url(value: &str) -> Result<String, String> {
    let mut url = Url::parse(value.trim()).map_err(|_| "WebDAV 地址无效".to_string())?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err("WebDAV 地址仅支持 HTTP 或 HTTPS".to_string());
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err("请使用独立的用户名和密码字段，不要在 WebDAV 地址中包含凭据".to_string());
    }
    url.set_query(None);
    url.set_fragment(None);
    url.path_segments_mut()
        .map_err(|_| "WebDAV 地址不能作为目录使用".to_string())?
        .pop_if_empty()
        .push("");
    Ok(url.to_string())
}

fn webdav_file_url(base: &str, filename: &str) -> Result<Url, String> {
    let base = normalize_webdav_url(base)?;
    Url::parse(&base)
        .and_then(|url| url.join(filename))
        .map_err(|_| "无法生成 WebDAV 上传地址".to_string())
}

fn build_client(proxy: Option<&str>) -> Result<Client, String> {
    client_factory::build_client(proxy).map_err(|error| format!("创建 WebDAV 客户端失败: {error}"))
}

fn with_basic_auth(request: RequestBuilder, config: &PtdBackupConfig) -> RequestBuilder {
    if config.username.is_empty() && config.password.is_empty() {
        request
    } else {
        request.basic_auth(&config.username, Some(&config.password))
    }
}

fn is_due(config: &PtdBackupConfig) -> bool {
    let Some(last_backup_at) = config
        .last_backup_at
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
    else {
        return true;
    };
    Utc::now()
        >= last_backup_at.with_timezone(&Utc)
            + Duration::hours(config.backup_interval_hours.min(i64::MAX as u64) as i64)
}

#[cfg(test)]
mod tests {
    use std::io::Read;

    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    use super::*;
    use crate::site::UserStats;

    fn sample_site() -> SiteWithStats {
        SiteWithStats {
            id: 9,
            name: "M-Team".to_string(),
            site_type: "mteam".to_string(),
            base_url: "https://api.m-team.cc".to_string(),
            auth_config: String::new(),
            request_headers: "[]".to_string(),
            use_proxy: false,
            created_at: String::new(),
            updated_at: String::new(),
            stats: Some(SiteStatsRecord {
                site_id: 9,
                uid: Some("123".to_string()),
                username: Some("alice".to_string()),
                uploaded: Some(1024),
                downloaded: Some(512),
                ratio: Some(2.0),
                bonus: Some(88.5),
                seeding_count: Some(7),
                leeching_count: Some(1),
                updated_at: Some("2026-09-04T12:30:00Z".to_string()),
                last_checked_at: "2026-09-04T12:30:00Z".to_string(),
                last_error: None,
            }),
        }
    }

    #[test]
    fn archive_matches_ptd_user_info_and_manifest_layout() {
        let now = DateTime::parse_from_rfc3339("2026-09-04T13:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let (archive, count) = build_ptd_archive(&[sample_site()], now).unwrap();
        assert_eq!(count, 1);

        let mut zip = zip::ZipArchive::new(Cursor::new(archive)).unwrap();
        let mut user_info = String::new();
        zip.by_name("userInfo.json")
            .unwrap()
            .read_to_string(&mut user_info)
            .unwrap();
        let user_info: Value = serde_json::from_str(&user_info).unwrap();
        assert_eq!(user_info["mteam"]["2026-09-04"]["site"], "mteam");
        assert_eq!(user_info["mteam"]["2026-09-04"]["status"], 3);
        assert_eq!(user_info["mteam"]["2026-09-04"]["uploaded"], 1024);

        let user_info_bytes = serde_json::to_vec(&user_info).unwrap();
        let mut manifest = String::new();
        zip.by_name("manifest.json")
            .unwrap()
            .read_to_string(&mut manifest)
            .unwrap();
        let manifest: Value = serde_json::from_str(&manifest).unwrap();
        assert_eq!(manifest["encryption"], false);
        assert_eq!(manifest["files"]["userInfo"]["name"], "userInfo.json");
        assert_eq!(
            manifest["files"]["userInfo"]["hash"],
            format!("{:x}", md5::compute(user_info_bytes))
        );
    }

    #[test]
    fn ptd_site_id_comes_from_the_official_catalog() {
        let mut site = sample_site();
        assert_eq!(ptd_site_id(&site), Some("mteam"));

        site.site_type = "nexusphp".to_string();
        site.base_url = "https://chdbits.co/".to_string();
        assert_eq!(ptd_site_id(&site), Some("chdbits"));

        site.base_url = "https://tracker.invalid/".to_string();
        assert_eq!(ptd_site_id(&site), None);
    }

    #[test]
    fn archive_skips_sites_that_ptd_does_not_recognize() {
        let mut site = sample_site();
        site.site_type = "nexusphp".to_string();
        site.base_url = "https://tracker.invalid/".to_string();
        let (_, count) = build_ptd_archive(&[site], Utc::now()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn webdav_paths_keep_existing_percent_encoding_and_append_filename() {
        let normalized = normalize_webdav_url("https://dav.example/蜂巢 PTD").unwrap();
        assert_eq!(normalized, "https://dav.example/%E8%9C%82%E5%B7%A2%20PTD/");
        assert_eq!(
            webdav_file_url(&normalized, "PTD_backup_20260904T1200.zip")
                .unwrap()
                .as_str(),
            "https://dav.example/%E8%9C%82%E5%B7%A2%20PTD/PTD_backup_20260904T1200.zip"
        );
    }

    #[tokio::test]
    async fn backup_now_uploads_zip_with_basic_auth_and_records_success() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            let mut content_length = None;
            let mut header_end = None;
            loop {
                let mut buffer = [0u8; 4096];
                let count = stream.read(&mut buffer).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..count]);
                if header_end.is_none()
                    && let Some(position) = request.windows(4).position(|part| part == b"\r\n\r\n")
                {
                    let end = position + 4;
                    let headers = String::from_utf8_lossy(&request[..end]);
                    content_length = headers.lines().find_map(|line| {
                        line.split_once(':').and_then(|(name, value)| {
                            name.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse::<usize>().ok())
                                .flatten()
                        })
                    });
                    header_end = Some(end);
                }
                if let (Some(end), Some(length)) = (header_end, content_length)
                    && request.len() >= end + length
                {
                    break;
                }
            }
            stream
                .write_all(
                    b"HTTP/1.1 201 Created\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                )
                .await
                .unwrap();
            request
        });

        let temp = tempfile::tempdir().unwrap();
        let db = Database::open(temp.path()).await.unwrap();
        let site_id = db
            .create_site(
                "M-Team",
                "mteam",
                "https://api.m-team.cc",
                r#"{"auth_type":"api_key","api_key":"test"}"#,
                "[]",
                false,
            )
            .await
            .unwrap();
        db.upsert_site_stats_success(
            site_id,
            &UserStats {
                uid: Some("123".to_string()),
                username: "alice".to_string(),
                uploaded: 1024,
                downloaded: 512,
                ratio: Some(2.0),
                bonus: Some(88.5),
                seeding_count: Some(7),
                leeching_count: Some(1),
            },
            "2026-09-04T12:30:00Z",
        )
        .await
        .unwrap();
        let mut config = db.get_ptd_backup_config().await.unwrap();
        config.webdav_url = format!("http://{address}/dav");
        config.username = "alice".to_string();
        config.password = "secret".to_string();
        db.update_ptd_backup_config(&config).await.unwrap();

        let result = backup_now(&db).await.unwrap();
        assert_eq!(result.site_count, 1);
        assert!(result.filename.starts_with("PTD_backup_"));
        let request = server.await.unwrap();
        let header_end = request
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        let headers = String::from_utf8_lossy(&request[..header_end]).to_ascii_lowercase();
        assert!(headers.starts_with("put /dav/ptd_backup_"));
        assert!(headers.contains("authorization: basic ywxpy2u6c2vjcmv0"));
        assert!(request[header_end..].starts_with(b"PK"));

        let saved = db.get_ptd_backup_config().await.unwrap();
        assert_eq!(
            saved.last_backup_filename.as_deref(),
            Some(result.filename.as_str())
        );
        assert!(saved.last_error.is_none());
    }
}
