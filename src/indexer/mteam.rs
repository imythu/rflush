use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use reqwest::header::{ACCEPT, HeaderMap, HeaderValue, LOCATION};
use reqwest::{Client, Method, StatusCode, Url};
use serde_json::{Value, json};

use crate::site::{SiteAuth, browser_request_header_map};

use super::access::OriginAccessGate;
use super::{
    IndexerAdapter, IndexerCapabilities, IndexerError, IndexerFuture, SearchRequest, SearchResult,
    endpoint_url, ensure_result_site, http_error, normalize_base_url, parse_json_or_rate_limit,
    rate_limit_error_from_body, rate_limit_error_from_json, read_torrent_response,
    resolve_same_origin_url, same_origin,
};

const MTEAM_DEFAULT_API: &str = "https://api.m-team.cc";
const MAX_DOWNLOAD_REDIRECTS: usize = 5;

pub struct MTeamIndexer {
    site_id: i64,
    site_name: String,
    base_url: Url,
    api_key: HeaderValue,
    request_headers: HeaderMap,
    client: Client,
    access_gate: Arc<OriginAccessGate>,
}

impl MTeamIndexer {
    pub(crate) fn new(
        site_id: i64,
        site_name: String,
        base_url: &str,
        auth: SiteAuth,
        request_headers: HeaderMap,
        client: Client,
        access_gate: Arc<OriginAccessGate>,
    ) -> Result<Self, IndexerError> {
        let base_url = if base_url.trim().is_empty() {
            normalize_base_url(MTEAM_DEFAULT_API)?
        } else {
            normalize_base_url(base_url)?
        };
        let SiteAuth::ApiKey { api_key } = auth else {
            return Err(IndexerError::Configuration(
                "M-Team search requires an API key".to_string(),
            ));
        };
        let api_key = api_key.trim();
        if api_key.is_empty() {
            return Err(IndexerError::Configuration(
                "M-Team API key cannot be empty".to_string(),
            ));
        }
        let mut api_key = HeaderValue::from_str(api_key).map_err(|_| {
            IndexerError::Configuration(
                "M-Team API key contains invalid header characters".to_string(),
            )
        })?;
        api_key.set_sensitive(true);

        Ok(Self {
            site_id,
            site_name: site_name.trim().to_string(),
            base_url,
            api_key,
            request_headers,
            client,
            access_gate,
        })
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = self.request_headers.clone();
        if !headers.contains_key(ACCEPT) {
            headers.insert(ACCEPT, HeaderValue::from_static("application/json, */*"));
        }
        headers.insert("x-api-key", self.api_key.clone());
        headers
    }

    fn download_headers(&self, url: &Url) -> HeaderMap {
        if same_origin(&self.base_url, url) {
            return self.headers();
        }

        browser_request_header_map(&self.request_headers)
    }

    async fn search_api(&self, request: &SearchRequest) -> Result<Vec<SearchResult>, IndexerError> {
        let url = endpoint_url(&self.base_url, "/api/torrent/search")?;
        let request = self.client.post(url).headers(self.headers()).json(&json!({
            "visible": 1,
            "pageNumber": request.page,
            "pageSize": request.page_size,
            "keyword": request.query,
        }));
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, request, &self.base_url)
            .await?;
        let status = response.status();
        if !same_origin(&self.base_url, response.url()) {
            return Err(IndexerError::UnsafeUrl(
                "M-Team search redirected to another origin".to_string(),
            ));
        }

        let body = response.text().await.map_err(http_error)?;
        if let Some(error) = rate_limit_error_from_body(&body) {
            return Err(error);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "M-Team API returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "M-Team API returned HTTP {status}"
            )));
        }
        let json = parse_json_or_rate_limit(&body, "M-Team API returned invalid JSON")?;
        ensure_mteam_success(&json)?;
        parse_search_results(&json, self.site_id, &self.site_name, &self.base_url)
    }

    async fn generate_download_url(&self, torrent_id: &str) -> Result<Url, IndexerError> {
        let url = endpoint_url(&self.base_url, "/api/torrent/genDlToken")?;
        let form = reqwest::multipart::Form::new().text("id", torrent_id.to_string());
        let request = self
            .client
            .post(url)
            .headers(self.headers())
            .multipart(form);
        let response = self
            .access_gate
            .send_with_same_origin_redirects_rebuilding(
                &self.client,
                request,
                &self.base_url,
                |method, url| {
                    if method != Method::POST {
                        return None;
                    }
                    let form = reqwest::multipart::Form::new().text("id", torrent_id.to_string());
                    Some(
                        self.client
                            .post(url.clone())
                            .headers(self.headers())
                            .multipart(form),
                    )
                },
            )
            .await?;
        let status = response.status();
        if !same_origin(&self.base_url, response.url()) {
            return Err(IndexerError::UnsafeUrl(
                "M-Team token endpoint redirected to another origin".to_string(),
            ));
        }
        let body = response.text().await.map_err(http_error)?;
        if let Some(error) = rate_limit_error_from_body(&body) {
            return Err(error);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "M-Team API returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "M-Team API returned HTTP {status}"
            )));
        }
        let json = parse_json_or_rate_limit(&body, "M-Team API returned invalid JSON")?;
        ensure_mteam_success(&json)?;
        let raw_url = json
            .get("data")
            .and_then(|data| {
                data.as_str().or_else(|| {
                    data.get("url")
                        .or_else(|| data.get("downloadUrl"))
                        .or_else(|| data.get("download_url"))
                        .and_then(Value::as_str)
                })
            })
            .filter(|url| !url.trim().is_empty())
            .ok_or_else(|| {
                IndexerError::Parse("M-Team token response is missing download URL".to_string())
            })?;
        resolve_same_origin_url(&self.base_url, raw_url)
    }

    async fn fetch(&self, result: &SearchResult) -> Result<Vec<u8>, IndexerError> {
        let torrent_id = ensure_result_site(result, self.site_id)?;
        let mut url = self.generate_download_url(&torrent_id).await?;

        for redirect_count in 0..=MAX_DOWNLOAD_REDIRECTS {
            if !is_allowed_download_url(&self.base_url, &url) {
                return Err(IndexerError::UnsafeUrl(
                    "M-Team download URL uses an untrusted origin".to_string(),
                ));
            }
            let response = self
                .access_gate
                .send(
                    self.client
                        .get(url.clone())
                        .headers(self.download_headers(&url)),
                )
                .await?;
            if !response.status().is_redirection() {
                let final_url = response.url().clone();
                return read_torrent_response(response, &final_url).await;
            }
            if redirect_count == MAX_DOWNLOAD_REDIRECTS {
                return Err(IndexerError::Http(
                    "M-Team download exceeded the redirect limit".to_string(),
                ));
            }
            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    IndexerError::Http("M-Team download redirect has no location".to_string())
                })?;
            url = response.url().join(location).map_err(|_| {
                IndexerError::UnsafeUrl("M-Team download redirect is invalid".to_string())
            })?;
        }

        unreachable!("redirect loop always returns")
    }
}

fn is_allowed_download_url(base_url: &Url, candidate: &Url) -> bool {
    if same_origin(base_url, candidate) {
        return true;
    }
    candidate.scheme() == "https"
        && candidate.username().is_empty()
        && candidate.password().is_none()
        && candidate.port().is_none()
        && candidate.host_str().is_some_and(|host| {
            host.eq_ignore_ascii_case("halomt.com")
                || host
                    .to_ascii_lowercase()
                    .strip_suffix(".halomt.com")
                    .is_some_and(|prefix| !prefix.is_empty())
        })
}

impl IndexerAdapter for MTeamIndexer {
    fn site_id(&self) -> i64 {
        self.site_id
    }

    fn site_name(&self) -> &str {
        &self.site_name
    }

    fn capabilities(&self) -> IndexerCapabilities {
        IndexerCapabilities {
            search: true,
            fetch_torrent: true,
            api_search: true,
            html_search: false,
        }
    }

    fn search<'a>(&'a self, request: &'a SearchRequest) -> IndexerFuture<'a, Vec<SearchResult>> {
        Box::pin(async move {
            let request = request.normalized()?;
            self.search_api(&request).await
        })
    }

    fn fetch_torrent<'a>(&'a self, result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
        Box::pin(async move { self.fetch(result).await })
    }
}

fn ensure_mteam_success(json: &Value) -> Result<(), IndexerError> {
    if let Some(error) = rate_limit_error_from_json(json) {
        return Err(error);
    }
    let code = json.get("code");
    let success = code.and_then(Value::as_i64) == Some(0)
        || code.and_then(Value::as_u64) == Some(0)
        || code
            .and_then(Value::as_str)
            .is_some_and(|value| value == "0" || value.eq_ignore_ascii_case("success"));
    if success {
        return Ok(());
    }
    let Some(code) = code else {
        return Err(IndexerError::Parse(
            "M-Team API response is missing code".to_string(),
        ));
    };
    let message = json
        .get("message")
        .or_else(|| json.get("msg"))
        .and_then(Value::as_str)
        .unwrap_or("request failed");
    let code_is_auth = code.as_u64().is_some_and(|code| matches!(code, 401 | 403))
        || code.as_i64().is_some_and(|code| matches!(code, 401 | 403))
        || code
            .as_str()
            .is_some_and(|code| matches!(code, "401" | "403"));
    let lower = message.to_ascii_lowercase();
    if code_is_auth
        || lower.contains("unauth")
        || lower.contains("api key")
        || lower.contains("invalid key")
    {
        return Err(IndexerError::AuthenticationExpired(
            "M-Team API key is invalid or expired".to_string(),
        ));
    }
    Err(IndexerError::Api(message.chars().take(512).collect()))
}

fn parse_search_results(
    json: &Value,
    site_id: i64,
    site_name: &str,
    base_url: &Url,
) -> Result<Vec<SearchResult>, IndexerError> {
    let items = json
        .pointer("/data/data")
        .or_else(|| json.pointer("/data/torrents"))
        .or_else(|| json.get("data"))
        .and_then(Value::as_array)
        .ok_or_else(|| IndexerError::Parse("M-Team response is missing result list".to_string()))?;

    let mut results = Vec::with_capacity(items.len());
    for item in items {
        let torrent_id = value_string(item.get("id"))
            .ok_or_else(|| IndexerError::Parse("M-Team result is missing id".to_string()))?;
        let title = item
            .get("name")
            .or_else(|| item.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if title.trim().is_empty() {
            continue;
        }
        let status = item.get("status").unwrap_or(item);
        let result = SearchResult {
            site_id,
            source_site: site_name.to_string(),
            torrent_id: torrent_id.clone(),
            title,
            // The public M-Team UI commonly uses a different origin from the API. Do not invent
            // a cross-origin detail URL that could later receive credentials.
            detail_url: None,
            download_locator: Some(torrent_id),
            magnet: item
                .get("magnet")
                .and_then(Value::as_str)
                .map(str::to_string),
            size: value_u64(item.get("size")).unwrap_or(0),
            seeders: value_u64(status.get("seeders"))
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            leechers: value_u64(status.get("leechers").or_else(|| status.get("leecher")))
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            publish_time: parse_datetime_value(
                item.get("createdDate")
                    .or_else(|| item.get("created_at"))
                    .or_else(|| item.get("publishTime")),
            ),
        }
        .sanitized_for_base(base_url)?;
        results.push(result);
    }
    Ok(results)
}

fn value_string(value: Option<&Value>) -> Option<String> {
    value.and_then(|value| {
        value
            .as_str()
            .map(str::to_string)
            .or_else(|| value.as_u64().map(|value| value.to_string()))
            .or_else(|| value.as_i64().map(|value| value.to_string()))
    })
}

fn value_u64(value: Option<&Value>) -> Option<u64> {
    value.and_then(|value| {
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
            .or_else(|| {
                value
                    .as_str()
                    .and_then(|value| value.replace(',', "").parse().ok())
            })
    })
}

fn parse_datetime_value(value: Option<&Value>) -> Option<DateTime<Utc>> {
    value.and_then(|value| {
        value
            .as_str()
            .and_then(parse_datetime)
            .or_else(|| {
                value
                    .as_i64()
                    .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
            })
            .or_else(|| {
                value
                    .as_u64()
                    .and_then(|timestamp| i64::try_from(timestamp).ok())
                    .and_then(|timestamp| DateTime::from_timestamp(timestamp, 0))
            })
    })
}

fn parse_datetime(raw: &str) -> Option<DateTime<Utc>> {
    let raw = raw.trim();
    if let Ok(value) = DateTime::parse_from_rfc3339(raw) {
        return Some(value.with_timezone(&Utc));
    }
    let timezone = FixedOffset::east_opt(8 * 60 * 60)?;
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(raw, format) {
            return timezone
                .from_local_datetime(&value)
                .single()
                .map(|value| value.with_timezone(&Utc));
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use reqwest::header::{HeaderMap, HeaderValue};
    use reqwest::{Client, Url};

    use crate::indexer::IndexerError;
    use crate::indexer::access::{OriginAccessGate, default_indexer_access_policy};
    use crate::site::SiteAuth;

    use super::{
        MTeamIndexer, ensure_mteam_success, is_allowed_download_url, parse_search_results,
    };

    #[test]
    fn search_headers_include_custom_values_and_protect_api_key() {
        let mut custom = HeaderMap::new();
        custom.insert("x-browser-profile", HeaderValue::from_static("desktop"));
        custom.insert("x-api-key", HeaderValue::from_static("stale"));
        let indexer = MTeamIndexer::new(
            1,
            "M-Team".to_string(),
            "https://api.m-team.cc",
            SiteAuth::ApiKey {
                api_key: "current".to_string(),
            },
            custom,
            Client::new(),
            Arc::new(OriginAccessGate::new(default_indexer_access_policy())),
        )
        .unwrap();

        let headers = indexer.headers();
        assert_eq!(headers["x-browser-profile"], "desktop");
        assert_eq!(headers["x-api-key"], "current");
    }

    #[test]
    fn cross_origin_download_only_receives_browser_fingerprint_headers() {
        let mut custom = HeaderMap::new();
        custom.insert("user-agent", HeaderValue::from_static("Custom Browser"));
        custom.insert("sec-ch-ua", HeaderValue::from_static("Chromium"));
        custom.insert("x-private-token", HeaderValue::from_static("secret"));
        let indexer = MTeamIndexer::new(
            1,
            "M-Team".to_string(),
            "https://api.m-team.cc",
            SiteAuth::ApiKey {
                api_key: "current".to_string(),
            },
            custom,
            Client::new(),
            Arc::new(OriginAccessGate::new(default_indexer_access_policy())),
        )
        .unwrap();

        let headers = indexer.download_headers(&Url::parse("https://halomt.com/file").unwrap());
        assert_eq!(headers["user-agent"], "Custom Browser");
        assert_eq!(headers["sec-ch-ua"], "Chromium");
        assert!(!headers.contains_key("x-private-token"));
        assert!(!headers.contains_key("x-api-key"));
    }

    #[test]
    fn allows_only_mteam_api_and_official_download_origins() {
        let base = Url::parse("https://api.m-team.cc").unwrap();
        for allowed in [
            "https://api.m-team.cc/api/rss/dlv2?token=secret",
            "https://halomt.com/file.torrent",
            "https://fr1.halomt.com/file.torrent",
        ] {
            assert!(is_allowed_download_url(
                &base,
                &Url::parse(allowed).unwrap()
            ));
        }
        for rejected in [
            "http://fr1.halomt.com/file.torrent",
            "https://fr1.halomt.com:444/file.torrent",
            "https://user@fr1.halomt.com/file.torrent",
            "https://evilhalomt.com/file.torrent",
            "https://halomt.com.evil.example/file.torrent",
        ] {
            assert!(!is_allowed_download_url(
                &base,
                &Url::parse(rejected).unwrap()
            ));
        }
    }

    #[test]
    fn parses_mteam_string_and_numeric_fields() {
        let fixture = serde_json::json!({
            "code": "0",
            "message": "SUCCESS",
            "data": {
                "pageNumber": 1,
                "pageSize": 30,
                "data": [{
                    "id": "1165802",
                    "name": "Series.S03E05.1080p.BluRay",
                    "size": "3221225472",
                    "createdDate": "2026-07-15 09:15:00",
                    "status": { "seeders": "42", "leechers": 7 }
                }]
            }
        });
        ensure_mteam_success(&fixture).unwrap();
        let base = Url::parse("https://api.m-team.cc").unwrap();
        let results = parse_search_results(&fixture, 9, "M-Team", &base).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].torrent_id, "1165802");
        assert_eq!(results[0].size, 3_221_225_472);
        assert_eq!(results[0].seeders, 42);
        assert_eq!(results[0].leechers, 7);
        assert!(results[0].publish_time.is_some());
        assert_eq!(results[0].download_locator.as_deref(), Some("1165802"));
    }

    #[test]
    fn rejects_mteam_api_error() {
        let fixture = serde_json::json!({ "code": "401", "message": "invalid key" });
        let error = ensure_mteam_success(&fixture).unwrap_err();
        assert_eq!(error.code(), "authentication_expired");

        let numeric = serde_json::json!({ "code": 403, "message": "forbidden" });
        assert_eq!(
            ensure_mteam_success(&numeric).unwrap_err().code(),
            "authentication_expired"
        );
        assert!(ensure_mteam_success(&serde_json::json!({ "data": {} })).is_err());
    }

    #[test]
    fn recognizes_mteam_rate_limit_responses() {
        for fixture in [
            serde_json::json!({ "code": 429, "message": "try later" }),
            serde_json::json!({ "code": "429", "message": "try later" }),
            serde_json::json!({ "code": 1, "message": "請求過於頻繁，請稍後再試" }),
            serde_json::json!({ "code": 1, "message": "Too Many Requests" }),
        ] {
            assert!(matches!(
                ensure_mteam_success(&fixture),
                Err(IndexerError::RateLimited(_))
            ));
        }

        assert!(matches!(
            ensure_mteam_success(&serde_json::json!({
                "code": 1,
                "message": "torrent is unavailable"
            })),
            Err(IndexerError::Api(_))
        ));
    }
}
