use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, HeaderMap, HeaderValue, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{Client, StatusCode};
use tracing::{debug, warn};

use crate::logging::current_task_context;
use crate::net::rate_limiter::{RateLimitPolicy, SharedRateLimiter};

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
const REDIRECT_LIMIT: usize = 20;
const EXPIRED_MESSAGE: &str = "連結不可用！ 超出有效期";
const RATE_LIMIT_MESSAGE: &str = "\u{8acb}\u{6c42}\u{904e}\u{65bc}\u{983b}\u{7e41}";

pub struct AppHttpClient {
    inner: Client,
    rate_limiter: Arc<SharedRateLimiter>,
    policy: RateLimitPolicy,
}

pub struct AppResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

#[derive(Debug, thiserror::Error)]
pub enum HttpError {
    #[error("{0}")]
    InvalidUrl(String),
    #[error("transport error: {source}")]
    Transport {
        #[source]
        source: reqwest::Error,
    },
    #[error("rate limited by remote server (key={key})")]
    RateLimited { key: String },
}

impl AppHttpClient {
    pub fn new(
        rate_limiter: Arc<SharedRateLimiter>,
        policy: RateLimitPolicy,
    ) -> Result<Self, reqwest::Error> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br, zstd"),
        );

        let inner = Client::builder()
            .default_headers(headers)
            .redirect(Policy::limited(REDIRECT_LIMIT))
            .http1_only()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30))
            .build()?;

        Ok(Self {
            inner,
            rate_limiter,
            policy,
        })
    }

    pub async fn get(&self, purpose: &str, url: &str) -> Result<AppResponse, HttpError> {
        let key = extract_rate_limit_key(url).map_err(HttpError::InvalidUrl)?;
        self.rate_limiter.acquire(&key, self.policy).await;

        let response = match self.inner.get(url).send().await {
            Ok(resp) => {
                let status = resp.status();
                if status.is_success() {
                    debug!(
                        task = %current_task_context(),
                        "HTTP {} ok: purpose=\"{}\" url={}",
                        status.as_u16(),
                        purpose,
                        url
                    );
                } else {
                    warn!(
                        task = %current_task_context(),
                        "HTTP {} error: purpose=\"{}\" url={}",
                        status.as_u16(),
                        purpose,
                        url
                    );
                }
                resp
            }
            Err(error) => {
                warn!(
                    task = %current_task_context(),
                    "HTTP transport error: purpose=\"{}\" url={} detail={}",
                    purpose,
                    url,
                    error
                );
                return Err(HttpError::Transport { source: error });
            }
        };

        let status = response.status();
        let headers = response.headers().clone();
        let body = response.bytes().await.map_err(|error| {
            warn!(
                task = %current_task_context(),
                "HTTP body read failed: purpose=\"{}\" url={} status={} detail={}",
                purpose,
                url,
                status,
                error
            );
            HttpError::Transport { source: error }
        })?;

        if is_rate_limited_json(&body) {
            warn!(
                task = %current_task_context(),
                "HTTP rate limited: purpose=\"{}\" url={} status={} — throttling key={}",
                purpose,
                url,
                status,
                key
            );
            self.rate_limiter.throttle(&key, self.policy).await;
            return Err(HttpError::RateLimited { key });
        }

        Ok(AppResponse {
            status,
            headers,
            body,
        })
    }
}

pub fn is_expired_response(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    value
        .get("message")
        .and_then(|m| m.as_str())
        .is_some_and(|m| m.contains(EXPIRED_MESSAGE))
}

pub fn parse_api_error_response(body: &[u8]) -> Option<String> {
    let text = std::str::from_utf8(body).ok()?;
    let value = serde_json::from_str::<serde_json::Value>(text).ok()?;
    let code = value.get("code")?.as_i64()?;
    let message = value.get("message")?.as_str()?;
    if code == 0 {
        return None;
    }
    Some(format!(
        "remote API error code={} message={}",
        code, message
    ))
}

fn extract_rate_limit_key(url: &str) -> Result<String, String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid URL '{}': {}", url, e))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("URL has no host: {}", url))?;
    let mut key = format!("{}://{}", parsed.scheme(), host);
    if let Some(port) = parsed.port() {
        key.push(':');
        key.push_str(&port.to_string());
    }
    Ok(key)
}

fn is_rate_limited_json(body: &[u8]) -> bool {
    let Ok(text) = std::str::from_utf8(body) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else {
        return false;
    };
    let code = value.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
    let message = value
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or_default();
    code == 1 && message.contains(RATE_LIMIT_MESSAGE)
}
