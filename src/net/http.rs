use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use reqwest::header::{ACCEPT, ACCEPT_ENCODING, HeaderMap, HeaderName, HeaderValue, USER_AGENT};
use reqwest::redirect::Policy;
use reqwest::{Client, Proxy, StatusCode};
use tracing::{debug, warn};

use crate::logging::current_task_context;
use crate::net::rate_limiter::{RateLimitPolicy, SharedRateLimiter};

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
const REDIRECT_LIMIT: usize = 20;
const RATE_LIMIT_MESSAGE: &str = "\u{8acb}\u{6c42}\u{904e}\u{65bc}\u{983b}\u{7e41}";

pub struct AppHttpClient {
    inner: Client,
    rate_limiter: Arc<SharedRateLimiter>,
    policy: RateLimitPolicy,
}

pub struct AppResponse {
    pub status: StatusCode,
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
        proxy: Option<&str>,
    ) -> Result<Self, reqwest::Error> {
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_USER_AGENT));
        headers.insert(ACCEPT, HeaderValue::from_static("*/*"));
        headers.insert(
            ACCEPT_ENCODING,
            HeaderValue::from_static("gzip, deflate, br, zstd"),
        );

        let mut builder = Client::builder()
            .default_headers(headers)
            .redirect(Policy::limited(REDIRECT_LIMIT))
            .http1_only()
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(60))
            .pool_max_idle_per_host(4)
            .tcp_keepalive(Duration::from_secs(30));

        if let Some(proxy) = proxy.and_then(|value| {
            let value = value.trim();
            (!value.is_empty()).then_some(value)
        }) {
            builder = builder.proxy(Proxy::all(proxy)?);
        }

        let inner = builder.build()?;

        Ok(Self {
            inner,
            rate_limiter,
            policy,
        })
    }

    /// 执行 GET，并允许附加已校验的请求头。
    /// 额外 header 会覆盖客户端的同名默认 header。
    pub async fn get_with_header_map(
        &self,
        purpose: &str,
        url: &str,
        extra_headers: &HeaderMap,
    ) -> Result<AppResponse, HttpError> {
        let key = extract_rate_limit_key(url).map_err(HttpError::InvalidUrl)?;
        self.rate_limiter.acquire(&key, self.policy).await;

        let response = match self
            .inner
            .get(url)
            .headers(extra_headers.clone())
            .send()
            .await
        {
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

        Ok(AppResponse { status, body })
    }

    /// 执行 GET，并允许附加自定义请求头（如 Cookie）。
    /// 额外 header 会覆盖同名的默认 header。
    pub async fn get_with_headers(
        &self,
        purpose: &str,
        url: &str,
        extra_headers: &[(&str, &str)],
    ) -> Result<AppResponse, HttpError> {
        let mut headers = HeaderMap::with_capacity(extra_headers.len());
        for (name, value) in extra_headers {
            headers.insert(
                HeaderName::from_bytes(name.as_bytes())
                    .unwrap_or_else(|_| HeaderName::from_static("x-unknown")),
                HeaderValue::from_bytes(value.as_bytes())
                    .unwrap_or_else(|_| HeaderValue::from_static("")),
            );
        }
        self.get_with_header_map(purpose, url, &headers).await
    }
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

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::time::Duration;

    use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;

    use super::AppHttpClient;
    use crate::net::rate_limiter::{RateLimitPolicy, SharedRateLimiter};

    #[tokio::test]
    async fn header_map_is_sent_on_direct_get_requests() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = Vec::new();
            loop {
                let mut chunk = [0u8; 1024];
                let count = stream.read(&mut chunk).await.unwrap();
                if count == 0 {
                    break;
                }
                request.extend_from_slice(&chunk[..count]);
                if request.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            stream
                .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok")
                .await
                .unwrap();
            String::from_utf8(request).unwrap()
        });

        let policy = RateLimitPolicy::new(10, Duration::from_secs(1), Duration::from_secs(1));
        let client = AppHttpClient::new(Arc::new(SharedRateLimiter::new()), policy, None).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(USER_AGENT, HeaderValue::from_static("Configured Browser"));
        headers.insert("x-site-header", HeaderValue::from_static("configured"));

        let response = client
            .get_with_header_map("test", &format!("http://{address}/rss"), &headers)
            .await
            .unwrap();
        assert_eq!(response.body.as_ref(), b"ok");

        let request = server.await.unwrap().to_ascii_lowercase();
        assert!(request.contains("user-agent: configured browser\r\n"));
        assert!(request.contains("x-site-header: configured\r\n"));
    }
}
