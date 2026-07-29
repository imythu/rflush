use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use chrono::{DateTime, FixedOffset, NaiveDateTime, TimeZone, Utc};
use reqwest::header::{ACCEPT, AUTHORIZATION, COOKIE, HeaderMap, HeaderValue};
use reqwest::{Client, StatusCode, Url};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use tokio::sync::Mutex;

use crate::site::SiteAuth;

use super::access::OriginAccessGate;
use super::{
    IndexerAdapter, IndexerCapabilities, IndexerError, IndexerFuture, SearchRequest, SearchResult,
    endpoint_url, ensure_result_site, http_error, normalize_base_url, parse_json_or_rate_limit,
    rate_limit_error_from_body, rate_limit_error_from_json, read_torrent_response,
    resolve_same_origin_url, response_is_authentication_page, same_origin,
};

enum NexusMode {
    Api {
        authorization: HeaderValue,
    },
    Html {
        cookie: HeaderValue,
        passkey: Option<String>,
    },
}

pub struct NexusPhpIndexer {
    site_id: i64,
    site_name: String,
    base_url: Url,
    mode: NexusMode,
    client: Client,
    access_gate: Arc<OriginAccessGate>,
    // Signed links are deliberately transient and never enter SearchResult or persistence.
    api_download_urls: Mutex<HashMap<String, Url>>,
}

impl NexusPhpIndexer {
    pub(crate) fn new(
        site_id: i64,
        site_name: String,
        base_url: &str,
        auth: SiteAuth,
        client: Client,
        access_gate: Arc<OriginAccessGate>,
    ) -> Result<Self, IndexerError> {
        let base_url = normalize_base_url(base_url)?;
        let mode = match auth {
            SiteAuth::ApiKey { api_key } => {
                let api_key = api_key.trim();
                if api_key.is_empty() {
                    return Err(IndexerError::Configuration(
                        "NexusPHP API key cannot be empty".to_string(),
                    ));
                }
                let mut authorization = HeaderValue::from_str(&format!("Bearer {api_key}"))
                    .map_err(|_| {
                        IndexerError::Configuration(
                            "NexusPHP API key contains invalid header characters".to_string(),
                        )
                    })?;
                authorization.set_sensitive(true);
                NexusMode::Api { authorization }
            }
            SiteAuth::Cookie { cookie } => {
                let cookie = cookie.trim();
                if cookie.is_empty() {
                    return Err(IndexerError::Configuration(
                        "NexusPHP cookie cannot be empty".to_string(),
                    ));
                }
                let mut cookie = HeaderValue::from_str(cookie).map_err(|_| {
                    IndexerError::Configuration(
                        "NexusPHP cookie contains invalid header characters".to_string(),
                    )
                })?;
                cookie.set_sensitive(true);
                NexusMode::Html {
                    cookie,
                    passkey: None,
                }
            }
            SiteAuth::CookiePasskey { cookie, passkey } => {
                let cookie_value = cookie.trim();
                if cookie_value.is_empty() {
                    return Err(IndexerError::Configuration(
                        "NexusPHP cookie cannot be empty".to_string(),
                    ));
                }
                let mut cookie = HeaderValue::from_str(cookie_value).map_err(|_| {
                    IndexerError::Configuration(
                        "NexusPHP cookie contains invalid header characters".to_string(),
                    )
                })?;
                cookie.set_sensitive(true);
                let passkey = passkey.trim();
                let passkey = (!passkey.is_empty()).then(|| passkey.to_string());
                NexusMode::Html { cookie, passkey }
            }
            SiteAuth::Passkey { .. } => {
                return Err(IndexerError::Configuration(
                    "NexusPHP search requires an API key or cookie".to_string(),
                ));
            }
        };

        Ok(Self {
            site_id,
            site_name: site_name.trim().to_string(),
            base_url,
            mode,
            client,
            access_gate,
            api_download_urls: Mutex::new(HashMap::new()),
        })
    }

    fn headers(&self) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(
            ACCEPT,
            HeaderValue::from_static("application/json, text/html;q=0.9, */*;q=0.8"),
        );
        match &self.mode {
            NexusMode::Api { authorization } => {
                headers.insert(AUTHORIZATION, authorization.clone());
            }
            NexusMode::Html { cookie, .. } => {
                headers.insert(COOKIE, cookie.clone());
            }
        }
        headers
    }

    async fn search_api(&self, request: &SearchRequest) -> Result<Vec<SearchResult>, IndexerError> {
        let url = endpoint_url(&self.base_url, "/api/v1/torrents")?;
        let request = self.client.get(url).headers(self.headers()).query(&[
            ("page", request.page.to_string()),
            ("per_page", request.page_size.to_string()),
            (
                "include_fields[torrent]",
                "download_url,active_status".to_string(),
            ),
            ("filter[title]", request.query.clone()),
        ]);
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, request, &self.base_url)
            .await?;
        let status = response.status();
        let final_url = response.url().clone();
        if !same_origin(&self.base_url, &final_url) {
            return Err(IndexerError::UnsafeUrl(
                "NexusPHP API redirected to another origin".to_string(),
            ));
        }

        let body = response.text().await.map_err(http_error)?;
        if let Some(error) = rate_limit_error_from_body(&body) {
            return Err(error);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "NexusPHP API returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "NexusPHP API returned HTTP {status}"
            )));
        }
        if response_is_authentication_page(&final_url, &body) {
            return Err(IndexerError::AuthenticationExpired(
                "NexusPHP API credentials are invalid or expired".to_string(),
            ));
        }
        let json = parse_json_or_rate_limit(&body, "NexusPHP API returned invalid JSON")?;
        ensure_nexus_success(&json)?;
        let parsed = parse_api_results(&json, self.site_id, &self.site_name, &self.base_url)?;

        let mut download_urls = self.api_download_urls.lock().await;
        if download_urls.len() > 2048 {
            download_urls.clear();
        }
        let mut results = Vec::with_capacity(parsed.len());
        for (result, download_url) in parsed {
            if let Some(download_url) = download_url {
                download_urls.insert(result.torrent_id.clone(), download_url);
            }
            results.push(result);
        }
        Ok(results)
    }

    async fn search_html(
        &self,
        request: &SearchRequest,
    ) -> Result<Vec<SearchResult>, IndexerError> {
        let url = endpoint_url(&self.base_url, "/torrents.php")?;
        let request = self.client.get(url).headers(self.headers()).query(&[
            ("search", request.query.clone()),
            ("notnewword", "1".to_string()),
            ("page", request.page.saturating_sub(1).to_string()),
        ]);
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, request, &self.base_url)
            .await?;
        let status = response.status();
        let final_url = response.url().clone();
        if !same_origin(&self.base_url, &final_url) {
            return Err(IndexerError::UnsafeUrl(
                "NexusPHP search redirected to another origin".to_string(),
            ));
        }
        let body = response.text().await.map_err(http_error)?;
        if let Some(error) = rate_limit_error_from_body(&body) {
            return Err(error);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "NexusPHP returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "NexusPHP returned HTTP {status}"
            )));
        }
        if response_is_authentication_page(&final_url, &body) {
            return Err(IndexerError::AuthenticationExpired(
                "NexusPHP cookie is invalid or expired".to_string(),
            ));
        }
        parse_html_results(&body, self.site_id, &self.site_name, &self.base_url)
    }

    async fn fetch_api_download_url(
        &self,
        torrent_id: &str,
        force_refresh: bool,
    ) -> Result<Url, IndexerError> {
        if force_refresh {
            self.api_download_urls.lock().await.remove(torrent_id);
        } else if let Some(url) = self.api_download_urls.lock().await.get(torrent_id).cloned() {
            return Ok(url);
        }

        // Resolve a fresh signed URL from the API so persisted ID-only locators survive restarts.
        let url = endpoint_url(&self.base_url, "/api/v1/torrents")?;
        let request = self.client.get(url).headers(self.headers()).query(&[
            ("per_page", "1"),
            ("include_fields[torrent]", "download_url"),
            ("filter[id]", torrent_id),
        ]);
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, request, &self.base_url)
            .await?;
        let status = response.status();
        let final_url = response.url().clone();
        if !same_origin(&self.base_url, &final_url) {
            return Err(IndexerError::UnsafeUrl(
                "NexusPHP API redirected to another origin".to_string(),
            ));
        }
        let body = response.text().await.map_err(http_error)?;
        if let Some(error) = rate_limit_error_from_body(&body) {
            return Err(error);
        }
        if matches!(status, StatusCode::UNAUTHORIZED | StatusCode::FORBIDDEN) {
            return Err(IndexerError::AuthenticationExpired(format!(
                "NexusPHP API returned HTTP {status}"
            )));
        }
        if !status.is_success() {
            return Err(IndexerError::Http(format!(
                "NexusPHP API returned HTTP {status}"
            )));
        }
        if response_is_authentication_page(&final_url, &body) {
            return Err(IndexerError::AuthenticationExpired(
                "NexusPHP API credentials are invalid or expired".to_string(),
            ));
        }
        let json = parse_json_or_rate_limit(&body, "NexusPHP API returned invalid JSON")?;
        ensure_nexus_success(&json)?;

        if let Some(raw_url) = api_items(&json)?
            .iter()
            .find(|item| value_string(item.get("id")).as_deref() == Some(torrent_id))
            .and_then(|item| item.get("download_url"))
            .and_then(Value::as_str)
        {
            let url = resolve_same_origin_url(&self.base_url, raw_url)?;
            self.api_download_urls
                .lock()
                .await
                .insert(torrent_id.to_string(), url.clone());
            return Ok(url);
        }

        // Several NexusPHP API deployments accept Bearer authentication on this legacy route.
        let mut fallback = endpoint_url(&self.base_url, "/download.php")?;
        fallback.query_pairs_mut().append_pair("id", torrent_id);
        Ok(fallback)
    }

    async fn download_url(&self, url: Url) -> Result<Vec<u8>, IndexerError> {
        let url = resolve_same_origin_url(&self.base_url, url.as_str())?;
        let request = self.client.get(url).headers(self.headers());
        let response = self
            .access_gate
            .send_with_same_origin_redirects(&self.client, request, &self.base_url)
            .await?;
        read_torrent_response(response, &self.base_url).await
    }

    async fn fetch(&self, result: &SearchResult) -> Result<Vec<u8>, IndexerError> {
        let torrent_id = ensure_result_site(result, self.site_id)?;
        match &self.mode {
            NexusMode::Api { .. } => {
                let url = self.fetch_api_download_url(&torrent_id, false).await?;
                match self.download_url(url.clone()).await {
                    Ok(body) => Ok(body),
                    Err(error)
                        if matches!(
                            &error,
                            IndexerError::AuthenticationExpired(_)
                                | IndexerError::Http(_)
                                | IndexerError::InvalidTorrent(_)
                        ) =>
                    {
                        let refreshed = self.fetch_api_download_url(&torrent_id, true).await?;
                        if refreshed == url {
                            return Err(error);
                        }
                        self.download_url(refreshed).await
                    }
                    Err(error) => Err(error),
                }
            }
            NexusMode::Html { passkey, .. } => {
                let mut url = endpoint_url(&self.base_url, "/download.php")?;
                url.query_pairs_mut().append_pair("id", &torrent_id);
                if let Some(passkey) = passkey {
                    url.query_pairs_mut().append_pair("passkey", passkey);
                }
                self.download_url(url).await
            }
        }
    }
}

impl IndexerAdapter for NexusPhpIndexer {
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
            api_search: matches!(&self.mode, NexusMode::Api { .. }),
            html_search: matches!(&self.mode, NexusMode::Html { .. }),
        }
    }

    fn search<'a>(&'a self, request: &'a SearchRequest) -> IndexerFuture<'a, Vec<SearchResult>> {
        Box::pin(async move {
            let request = request.normalized()?;
            match &self.mode {
                NexusMode::Api { .. } => self.search_api(&request).await,
                NexusMode::Html { .. } => self.search_html(&request).await,
            }
        })
    }

    fn fetch_torrent<'a>(&'a self, result: &'a SearchResult) -> IndexerFuture<'a, Vec<u8>> {
        Box::pin(async move { self.fetch(result).await })
    }
}

fn ensure_nexus_success(json: &Value) -> Result<(), IndexerError> {
    if let Some(error) = rate_limit_error_from_json(json) {
        return Err(error);
    }
    let Some(ret) = json.get("ret") else {
        return Err(IndexerError::Parse(
            "NexusPHP API response is missing ret".to_string(),
        ));
    };
    let success = ret.as_i64() == Some(0)
        || ret.as_u64() == Some(0)
        || ret
            .as_str()
            .is_some_and(|value| value == "0" || value.eq_ignore_ascii_case("success"));
    if success {
        return Ok(());
    }
    let message = json
        .get("msg")
        .or_else(|| json.get("message"))
        .and_then(Value::as_str)
        .unwrap_or("request failed");
    let lower = message.to_ascii_lowercase();
    if lower.contains("unauth")
        || lower.contains("api key")
        || lower.contains("token") && lower.contains("expired")
        || lower.contains("login")
    {
        return Err(IndexerError::AuthenticationExpired(
            "NexusPHP API credentials are invalid or expired".to_string(),
        ));
    }
    Err(IndexerError::Api(message.chars().take(512).collect()))
}

fn api_items(json: &Value) -> Result<Vec<&Value>, IndexerError> {
    let candidates = [
        json.pointer("/data/data"),
        json.pointer("/data/torrents"),
        json.get("data"),
        Some(json),
    ];
    candidates
        .into_iter()
        .flatten()
        .find_map(Value::as_array)
        .map(|items| items.iter().collect())
        .ok_or_else(|| {
            IndexerError::Parse("NexusPHP API response is missing result list".to_string())
        })
}

fn parse_api_results(
    json: &Value,
    site_id: i64,
    site_name: &str,
    base_url: &Url,
) -> Result<Vec<(SearchResult, Option<Url>)>, IndexerError> {
    let mut results = Vec::new();
    for item in api_items(json)? {
        let torrent_id = value_string(item.get("id"))
            .ok_or_else(|| IndexerError::Parse("NexusPHP result is missing id".to_string()))?;
        let title = item
            .get("name")
            .or_else(|| item.get("title"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        if title.trim().is_empty() {
            continue;
        }

        let download_url = item
            .get("download_url")
            .and_then(Value::as_str)
            .filter(|url| !url.trim().is_empty())
            .and_then(|url| resolve_same_origin_url(base_url, url).ok());
        let detail_url = item
            .get("detail_url")
            .or_else(|| item.get("details_url"))
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| default_detail_url(base_url, &torrent_id).ok());

        let result = SearchResult {
            site_id,
            source_site: site_name.to_string(),
            torrent_id: torrent_id.clone(),
            title,
            detail_url,
            download_locator: Some(torrent_id),
            magnet: item
                .get("magnet")
                .and_then(Value::as_str)
                .map(str::to_string),
            size: value_u64(item.get("size")).unwrap_or(0),
            seeders: value_u64(item.get("seeders"))
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            leechers: value_u64(item.get("leechers"))
                .unwrap_or(0)
                .min(u32::MAX as u64) as u32,
            publish_time: parse_datetime_value(
                item.get("added")
                    .or_else(|| item.get("created_at"))
                    .or_else(|| item.get("publish_time")),
            ),
        }
        .sanitized_for_base(base_url)?;
        results.push((result, download_url));
    }
    Ok(results)
}

fn parse_html_results(
    html: &str,
    site_id: i64,
    site_name: &str,
    base_url: &Url,
) -> Result<Vec<SearchResult>, IndexerError> {
    let document = Html::parse_document(html);
    let torrent_rows = Selector::parse("table.torrents tr")
        .map_err(|_| IndexerError::Parse("invalid NexusPHP row selector".to_string()))?;
    let all_rows = Selector::parse("tr")
        .map_err(|_| IndexerError::Parse("invalid NexusPHP fallback selector".to_string()))?;
    let rows: Vec<ElementRef<'_>> = document.select(&torrent_rows).collect();
    let rows: Vec<ElementRef<'_>> = if rows.is_empty() {
        document.select(&all_rows).collect()
    } else {
        rows
    };

    let detail_links =
        Selector::parse("a[href*='details.php?id='], a[href^='/details/'], a[href*='/detail/']")
            .map_err(|_| IndexerError::Parse("invalid NexusPHP link selector".to_string()))?;
    let cells = Selector::parse("td")
        .map_err(|_| IndexerError::Parse("invalid NexusPHP cell selector".to_string()))?;
    let titled = Selector::parse("[title]")
        .map_err(|_| IndexerError::Parse("invalid NexusPHP time selector".to_string()))?;

    let mut results = Vec::new();
    let mut seen = HashSet::new();
    for row in rows {
        let mut selected: Option<(String, String, String)> = None;
        for link in row.select(&detail_links) {
            let Some(href) = link.value().attr("href") else {
                continue;
            };
            let Some(torrent_id) = extract_torrent_id(href, base_url) else {
                continue;
            };
            let text = normalize_text(link.text());
            let title = link
                .value()
                .attr("title")
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .unwrap_or(&text)
                .to_string();
            if selected.is_none() || !title.is_empty() {
                selected = Some((torrent_id, title, href.to_string()));
            }
            if selected
                .as_ref()
                .is_some_and(|(_, title, _)| !title.is_empty())
            {
                break;
            }
        }
        let Some((torrent_id, title, href)) = selected else {
            continue;
        };
        if title.trim().is_empty() {
            continue;
        }
        if !seen.insert(torrent_id.clone()) {
            continue;
        }

        let cell_text: Vec<String> = row
            .select(&cells)
            .map(|cell| normalize_text(cell.text()))
            .collect();
        let (size_index, size) = cell_text
            .iter()
            .enumerate()
            .find_map(|(index, text)| parse_size(text).map(|size| (index, size)))
            .unwrap_or((usize::MAX, 0));
        let seeders = size_index
            .checked_add(1)
            .and_then(|index| cell_text.get(index))
            .and_then(|text| parse_count(text))
            .unwrap_or(0);
        let leechers = size_index
            .checked_add(2)
            .and_then(|index| cell_text.get(index))
            .and_then(|text| parse_count(text))
            .unwrap_or(0);
        let publish_time = row
            .select(&titled)
            .filter_map(|element| element.value().attr("title"))
            .find_map(parse_datetime)
            .or_else(|| cell_text.iter().find_map(|text| parse_datetime(text)));

        let detail_url = resolve_same_origin_url(base_url, &href)
            .ok()
            .map(|url| url.to_string())
            .or_else(|| default_detail_url(base_url, &torrent_id).ok());
        let result = SearchResult {
            site_id,
            source_site: site_name.to_string(),
            torrent_id: torrent_id.clone(),
            title,
            detail_url,
            download_locator: Some(torrent_id),
            magnet: None,
            size,
            seeders,
            leechers,
            publish_time,
        }
        .sanitized_for_base(base_url)?;
        results.push(result);
    }
    Ok(results)
}

fn default_detail_url(base_url: &Url, torrent_id: &str) -> Result<String, IndexerError> {
    let mut url = endpoint_url(base_url, "/details.php")?;
    url.query_pairs_mut().append_pair("id", torrent_id);
    Ok(url.to_string())
}

fn extract_torrent_id(raw: &str, base_url: &Url) -> Option<String> {
    let url = Url::parse(raw).or_else(|_| base_url.join(raw)).ok()?;
    if let Some(id) = url.query_pairs().find_map(|(key, value)| {
        (key.eq_ignore_ascii_case("id") || key.eq_ignore_ascii_case("torrent_id"))
            .then(|| value.into_owned())
    }) {
        return (!id.is_empty()).then_some(id);
    }
    url.path_segments()?
        .rev()
        .find(|segment| !segment.is_empty() && segment.chars().all(|ch| ch.is_ascii_digit()))
        .map(str::to_string)
}

fn normalize_text<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
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
            .or_else(|| value.as_str().and_then(parse_size))
    })
}

fn parse_count(raw: &str) -> Option<u32> {
    let digits: String = raw.chars().filter(char::is_ascii_digit).collect();
    digits.parse().ok()
}

fn parse_size(raw: &str) -> Option<u64> {
    let compact = raw.trim().replace(',', "");
    let split = compact.find(|ch: char| !(ch.is_ascii_digit() || ch == '.'))?;
    let amount: f64 = compact[..split].parse().ok()?;
    let unit = compact[split..]
        .trim_start()
        .split_whitespace()
        .next()?
        .to_ascii_lowercase();
    let multiplier = match unit.as_str() {
        "b" | "byte" | "bytes" => 1_u64,
        "kb" | "kib" => 1024,
        "mb" | "mib" => 1024 * 1024,
        "gb" | "gib" => 1024 * 1024 * 1024,
        "tb" | "tib" => 1024_u64.pow(4),
        "pb" | "pib" => 1024_u64.pow(5),
        _ => return None,
    };
    if !amount.is_finite() || amount.is_sign_negative() {
        return None;
    }
    Some((amount * multiplier as f64).min(u64::MAX as f64) as u64)
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
    for format in ["%Y-%m-%d %H:%M:%S", "%Y-%m-%d %H:%M", "%Y-%m-%d"] {
        if let Ok(value) = NaiveDateTime::parse_from_str(raw, format) {
            return timezone
                .from_local_datetime(&value)
                .single()
                .map(|value| value.with_timezone(&Utc));
        }
        if format == "%Y-%m-%d"
            && let Ok(date) = chrono::NaiveDate::parse_from_str(raw, format)
            && let Some(value) = date.and_hms_opt(0, 0, 0)
        {
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
    use reqwest::Url;

    use crate::indexer::IndexerError;

    use super::{ensure_nexus_success, parse_api_results, parse_html_results};

    #[test]
    fn parses_nexus_api_fixture_without_persisting_download_secret() {
        let json = serde_json::json!({
            "ret": 0,
            "data": {
                "meta": { "current_page": 1 },
                "data": [{
                    "id": 123,
                    "name": "Show.S02E03.1080p.WEB-DL",
                    "size": 2147483648_u64,
                    "seeders": 17,
                    "leechers": "4",
                    "added": "2026-07-15 12:30:00",
                    "download_url": "https://tracker.example/download.php?id=123&passkey=secret-value"
                }]
            }
        });
        let base = Url::parse("https://tracker.example").unwrap();
        let parsed = parse_api_results(&json, 7, "Tracker", &base).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0.torrent_id, "123");
        assert_eq!(parsed[0].0.seeders, 17);
        assert_eq!(parsed[0].0.leechers, 4);
        assert!(
            parsed[0]
                .1
                .as_ref()
                .unwrap()
                .as_str()
                .contains("secret-value")
        );

        let serialized = serde_json::to_string(&parsed[0].0).unwrap();
        assert!(!serialized.contains("secret-value"));
        assert_eq!(parsed[0].0.download_locator.as_deref(), Some("123"));
    }

    #[test]
    fn parses_default_nexus_html_fixture() {
        let html = r#"
            <html><body><table class="torrents"><tbody>
              <tr><th>Type</th><th>Name</th><th>Comments</th><th>Added</th><th>Size</th><th>Seeders</th><th>Leechers</th></tr>
              <tr>
                <td>TV</td>
                <td><table class="torrentname"><tr><td><a href="details.php?id=456&amp;hit=1"><b>Series.S01E08.2160p</b></a></td></tr></table></td>
                <td>2</td><td><span title="2026-07-14 08:10:00">1 day</span></td>
                <td>1.5 GiB</td><td><a>31</a></td><td><a>6</a></td>
              </tr>
            </tbody></table></body></html>
        "#;
        let base = Url::parse("https://tracker.example").unwrap();
        let results = parse_html_results(html, 8, "HTML Tracker", &base).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].torrent_id, "456");
        assert_eq!(results[0].title, "Series.S01E08.2160p");
        assert_eq!(results[0].size, 1_610_612_736);
        assert_eq!(results[0].seeders, 31);
        assert_eq!(results[0].leechers, 6);
        assert!(results[0].publish_time.is_some());
        assert_eq!(results[0].download_locator.as_deref(), Some("456"));
    }

    #[test]
    fn ignores_cross_origin_detail_links() {
        let html = r#"<table class="torrents"><tr><td><a href="https://evil.example/details.php?id=9">Release</a></td><td>1 GiB</td><td>1</td><td>0</td></tr></table>"#;
        let base = Url::parse("https://tracker.example").unwrap();
        let results = parse_html_results(html, 1, "Tracker", &base).unwrap();
        assert_eq!(results.len(), 1);
        assert!(
            results[0]
                .detail_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://tracker.example/"))
        );
    }

    #[test]
    fn rejects_nexus_schema_drift_instead_of_returning_empty_results() {
        let base = Url::parse("https://tracker.example").unwrap();
        let missing_ret = serde_json::json!({ "data": { "data": [] } });
        assert!(ensure_nexus_success(&missing_ret).is_err());

        let missing_list = serde_json::json!({ "ret": 0, "data": { "meta": {} } });
        assert!(parse_api_results(&missing_list, 1, "Tracker", &base).is_err());
    }

    #[test]
    fn recognizes_only_explicit_nexus_rate_limit_messages() {
        for message in [
            "Too Many Requests",
            "request rate limit exceeded",
            "请求过于频繁，请稍后再试",
            "請求過於頻繁，請稍後再試",
        ] {
            let json = serde_json::json!({ "ret": 1, "msg": message });
            assert!(matches!(
                ensure_nexus_success(&json),
                Err(IndexerError::RateLimited(_))
            ));
        }
        assert!(matches!(
            ensure_nexus_success(&serde_json::json!({
                "code": 1,
                "message": "請求過於頻繁"
            })),
            Err(IndexerError::RateLimited(_))
        ));
        assert!(matches!(
            ensure_nexus_success(&serde_json::json!({
                "code": "429",
                "message": "try later"
            })),
            Err(IndexerError::RateLimited(_))
        ));

        let unrelated = serde_json::json!({
            "ret": 1,
            "msg": "download is unavailable because the user's ratio is too low"
        });
        assert!(matches!(
            ensure_nexus_success(&unrelated),
            Err(IndexerError::Api(_))
        ));
    }
}
