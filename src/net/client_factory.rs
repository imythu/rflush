use std::sync::Once;
use std::time::Duration;

use reqwest::header::{HeaderMap, HeaderValue, USER_AGENT};
use reqwest::{Client, Proxy};
use serde::Serialize;
use tracing::warn;

const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/145.0.0.0 Safari/537.36";
static MISSING_PROXY_WARNING: Once = Once::new();

/// 代理测试结果
#[derive(Debug, Clone, Serialize)]
pub struct ProxyTestResult {
    pub success: bool,
    pub status_code: Option<u16>,
    pub elapsed_ms: u64,
    pub message: String,
}

/// 构建一个配置了浏览器 UA 和超时的 reqwest Client Builder 基础模板。
fn base_builder() -> reqwest::ClientBuilder {
    let mut headers = HeaderMap::new();
    headers.insert(USER_AGENT, HeaderValue::from_static(BROWSER_USER_AGENT));
    Client::builder()
        .default_headers(headers)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
}

/// 构建一个 reqwest::Client，可选带代理。
pub fn build_client(proxy: Option<&str>) -> Result<Client, reqwest::Error> {
    let mut builder = base_builder();
    if let Some(proxy_url) = proxy.map(str::trim).filter(|v| !v.is_empty()) {
        builder = builder.proxy(Proxy::all(proxy_url)?);
    }
    builder.build()
}

/// 根据 (全局代理地址, use_proxy 开关) 决定返回带代理还是直连的 Client。
///
/// - `use_proxy=true` 且 `proxy` 有值 → 返回带代理 Client
/// - `use_proxy=true` 且 `proxy` 为空 → 每个进程 warn 一次 + 返回直连 Client（静默降级）
/// - `use_proxy=false` → 返回直连 Client
pub fn resolve_client(proxy: Option<&str>, use_proxy: bool) -> Result<Client, reqwest::Error> {
    let effective_proxy = proxy.map(str::trim).filter(|v| !v.is_empty());
    if use_proxy {
        if let Some(url) = effective_proxy {
            return build_client(Some(url));
        }
        MISSING_PROXY_WARNING.call_once(|| {
            warn!("站点标记了使用代理，但全局代理地址未配置，将直连访问");
        });
    }
    build_client(None)
}

/// 代理连通性测试：用指定代理 GET 一个 URL，返回结果。
pub async fn test_proxy(proxy: &str, test_url: &str) -> ProxyTestResult {
    let proxy = proxy.trim();
    if proxy.is_empty() {
        return ProxyTestResult {
            success: false,
            status_code: None,
            elapsed_ms: 0,
            message: "代理地址不能为空".to_string(),
        };
    }

    let client = match build_client(Some(proxy)) {
        Ok(c) => c,
        Err(e) => {
            return ProxyTestResult {
                success: false,
                status_code: None,
                elapsed_ms: 0,
                message: format!("创建代理客户端失败: {}", e),
            };
        }
    };

    let start = std::time::Instant::now();
    match client.get(test_url).send().await {
        Ok(resp) => {
            let elapsed = start.elapsed().as_millis() as u64;
            let status = resp.status();
            ProxyTestResult {
                success: status.is_success() || status.is_redirection(),
                status_code: Some(status.as_u16()),
                elapsed_ms: elapsed,
                message: format!("HTTP {}", status),
            }
        }
        Err(e) => {
            let elapsed = start.elapsed().as_millis() as u64;
            ProxyTestResult {
                success: false,
                status_code: None,
                elapsed_ms: elapsed,
                message: format!("请求失败: {}", e),
            }
        }
    }
}
