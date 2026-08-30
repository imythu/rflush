pub mod scheduler;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tungstenite::{Message, connect};

use crate::config::{CloakBrowserConfig, GlobalConfig, LightpandaConfig};
use crate::site::{SiteAuth, SiteRecord};

pub const SIGN_IN_BROWSER_LIGHTPANDA: &str = "lightpanda";
pub const SIGN_IN_BROWSER_CLOAKBROWSER: &str = "cloakbrowser";

static CLOAKBROWSER_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInTaskRecord {
    pub id: i64,
    pub name: String,
    pub site_id: i64,
    pub cron_expression: String,
    pub browser: String,
    pub sign_in_method: String,
    pub enabled: bool,
    pub last_status: Option<String>,
    pub last_message: Option<String>,
    pub last_run_at: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInTaskRequest {
    pub name: String,
    pub site_id: i64,
    pub cron_expression: String,
    pub browser: Option<String>,
    pub sign_in_method: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInRecord {
    pub id: i64,
    pub task_id: i64,
    pub site_id: i64,
    pub site_name: String,
    pub started_at: String,
    pub finished_at: String,
    pub status: String,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInResult {
    pub status: String,
    pub message: String,
    pub started_at: String,
    pub finished_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BrowserProbeResult {
    pub success: bool,
    pub url: String,
    pub message: String,
    pub title: Option<String>,
}

pub async fn probe_browser_1_1_1_1(
    browser: String,
    settings: GlobalConfig,
) -> Result<BrowserProbeResult, String> {
    match browser.as_str() {
        SIGN_IN_BROWSER_LIGHTPANDA => {
            let endpoint =
                build_lightpanda_endpoint(&settings.lightpanda, settings.use_proxy_for_lightpanda)?;
            tokio::task::spawn_blocking(move || {
                run_cdp_probe(endpoint, "https://1.1.1.1", "Lightpanda")
            })
            .await
            .map_err(|e| format!("Lightpanda 探测任务 join 失败: {}", e))?
        }
        SIGN_IN_BROWSER_CLOAKBROWSER => tokio::task::spawn_blocking(move || {
            with_cloakbrowser(settings.cloakbrowser, |endpoint| {
                run_cdp_probe(endpoint, "https://1.1.1.1", "CloakBrowser")
            })
        })
        .await
        .map_err(|e| format!("CloakBrowser 探测任务 join 失败: {}", e))?,
        _ => Err(format!("未知签到浏览器: {}", browser)),
    }
}

pub async fn execute_task(
    _base_dir: std::path::PathBuf,
    task: SignInTaskRecord,
    site: SiteRecord,
    settings: GlobalConfig,
) -> Result<SignInResult, String> {
    if site.site_type != "nexusphp" && site.site_type != "nexus_php" {
        return Err("自动签到目前仅支持 NexusPHP 站点".to_string());
    }

    let auth = serde_json::from_str::<SiteAuth>(&site.auth_config)
        .map_err(|e| format!("认证配置解析失败: {}", e))?;
    let cookie = match auth {
        SiteAuth::Cookie { cookie } | SiteAuth::CookiePasskey { cookie, .. } => cookie,
        _ => return Err("NexusPHP 自动签到需要 Cookie 认证".to_string()),
    };
    if cookie.trim().is_empty() {
        return Err("Cookie 不能为空".to_string());
    }

    let base_url = site.base_url.trim_end_matches('/').to_string();
    let started_at = Utc::now().to_rfc3339();
    let output = match task.browser.as_str() {
        SIGN_IN_BROWSER_LIGHTPANDA => {
            let endpoint =
                build_lightpanda_endpoint(&settings.lightpanda, settings.use_proxy_for_lightpanda)?;
            run_cdp_sign_in(
                endpoint,
                base_url,
                cookie,
                task.sign_in_method,
                settings.ocr_api_key,
            )
            .await?
        }
        SIGN_IN_BROWSER_CLOAKBROWSER => {
            let config = settings.cloakbrowser;
            let sign_in_method = task.sign_in_method;
            let ocr_api_key = settings.ocr_api_key;
            tokio::task::spawn_blocking(move || {
                with_cloakbrowser(config, |endpoint| {
                    run_cdp_sign_in_blocking(
                        endpoint,
                        base_url,
                        cookie,
                        sign_in_method,
                        ocr_api_key,
                    )
                })
            })
            .await
            .map_err(|e| format!("CloakBrowser 签到任务 join 失败: {}", e))??
        }
        _ => return Err(format!("未知签到浏览器: {}", task.browser)),
    };
    let finished_at = Utc::now().to_rfc3339();

    Ok(SignInResult {
        status: output.status,
        message: output.message,
        started_at,
        finished_at,
    })
}

fn run_cdp_probe(
    endpoint: String,
    url: &str,
    browser_name: &str,
) -> Result<BrowserProbeResult, String> {
    let mut client = CdpClient::connect(endpoint)?;
    let session_id = create_target_session(&mut client)?;

    let _ = client.call("Page.enable", json!({}), Some(&session_id));
    let navigate = client.call("Page.navigate", json!({ "url": url }), Some(&session_id));
    if let Err(error) = navigate {
        return Ok(BrowserProbeResult {
            success: false,
            url: url.to_string(),
            message: format!("Page.navigate 失败: {}", error),
            title: None,
        });
    }

    thread::sleep(Duration::from_secs(2));
    let title = client
        .call(
            "Runtime.evaluate",
            json!({
                "expression": "document.title",
                "returnByValue": true
            }),
            Some(&session_id),
        )
        .ok()
        .and_then(|value| {
            value
                .get("result")
                .and_then(|v| v.get("value"))
                .and_then(Value::as_str)
                .map(str::to_string)
        });

    Ok(BrowserProbeResult {
        success: true,
        url: url.to_string(),
        message: format!("{} 已成功导航到 1.1.1.1", browser_name),
        title,
    })
}

fn create_target_session(client: &mut CdpClient) -> Result<String, String> {
    let target = client.call(
        "Target.createTarget",
        json!({
            "url": "about:blank",
            "newWindow": false,
            "background": false
        }),
        None,
    )?;
    let target_id = target
        .get("targetId")
        .and_then(Value::as_str)
        .ok_or_else(|| "CDP 未返回 targetId".to_string())?
        .to_string();
    let attached = client.call(
        "Target.attachToTarget",
        json!({
            "targetId": target_id,
            "flatten": true
        }),
        None,
    )?;
    let session_id = attached
        .get("sessionId")
        .and_then(Value::as_str)
        .ok_or_else(|| "CDP 未返回 sessionId".to_string())?
        .to_string();
    Ok(session_id)
}

struct CdpClient {
    socket: tungstenite::WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>,
    next_id: u64,
}

impl CdpClient {
    fn connect(endpoint: String) -> Result<Self, String> {
        let (socket, _) =
            connect(endpoint.as_str()).map_err(|e| format!("连接浏览器 CDP 失败: {}", e))?;
        Ok(Self { socket, next_id: 0 })
    }

    fn call(
        &mut self,
        method: &str,
        params: Value,
        session_id: Option<&str>,
    ) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        let mut request = json!({
            "id": id,
            "method": method,
            "params": params,
        });
        if let Some(session_id) = session_id {
            request["sessionId"] = json!(session_id);
        }

        self.socket
            .send(Message::Text(request.to_string().into()))
            .map_err(|e| format!("发送 CDP 指令失败: {}", e))?;

        loop {
            let message = self
                .socket
                .read()
                .map_err(|e| format!("读取 CDP 响应失败: {}", e))?;
            let Message::Text(text) = message else {
                continue;
            };
            let value: Value =
                serde_json::from_str(&text).map_err(|e| format!("解析 CDP 响应失败: {}", e))?;
            if value.get("id").and_then(Value::as_u64) != Some(id) {
                continue;
            }
            if let Some(error) = value.get("error") {
                return Err(error.to_string());
            }
            return Ok(value.get("result").cloned().unwrap_or_else(|| json!({})));
        }
    }
}

impl Drop for CdpClient {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}

async fn run_cdp_sign_in(
    endpoint: String,
    base_url: String,
    cookie: String,
    sign_in_method: String,
    ocr_api_key: Option<String>,
) -> Result<SignInOutput, String> {
    tokio::task::spawn_blocking(move || {
        run_cdp_sign_in_blocking(endpoint, base_url, cookie, sign_in_method, ocr_api_key)
    })
    .await
    .map_err(|e| format!("签到任务 join 失败: {}", e))?
}

fn run_cdp_sign_in_blocking(
    endpoint: String,
    base_url: String,
    cookie: String,
    sign_in_method: String,
    ocr_api_key: Option<String>,
) -> Result<SignInOutput, String> {
    let mut client = CdpClient::connect(endpoint)?;
    let session_id = create_target_session(&mut client)?;

    client.call("Page.enable", json!({}), Some(&session_id))?;
    client.call("Runtime.enable", json!({}), Some(&session_id))?;
    client.call("Network.enable", json!({}), Some(&session_id))?;
    set_cookies_via_cdp(&mut client, &session_id, &base_url, &cookie)?;

    let index_url = format!("{}/index.php", base_url);
    navigate_via_cdp(&mut client, &session_id, &index_url, "打开首页失败")?;
    wait_for_cloudflare(&mut client, &session_id)?;
    let text = page_text_via_cdp(&mut client, &session_id)?;
    if looks_logged_out(&text) {
        return Err("Cookie 无效或已过期".to_string());
    }

    let attendance_url = format!("{}/attendance.php", base_url);
    navigate_via_cdp(&mut client, &session_id, &attendance_url, "打开签到页失败")?;
    wait_for_cloudflare(&mut client, &session_id)?;

    match sign_in_method.as_str() {
        SIGN_IN_METHOD_OPEN_PAGE => run_open_page_sign_in(&mut client, &session_id),
        SIGN_IN_METHOD_CLOUDFLARE => run_cloudflare_sign_in(&mut client, &session_id, &base_url),
        SIGN_IN_METHOD_OCR_CAPTCHA => {
            run_ocr_captcha_sign_in(&mut client, &session_id, &base_url, ocr_api_key.as_deref())
        }
        other => Err(format!("未知签到方式: {}", other)),
    }
}

fn run_open_page_sign_in(client: &mut CdpClient, session_id: &str) -> Result<SignInOutput, String> {
    thread::sleep(Duration::from_secs(7));
    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }
    Ok(SignInOutput {
        status: "success".to_string(),
        message: compact_text(&text).unwrap_or_else(|| "已访问签到页".to_string()),
    })
}

fn run_cloudflare_sign_in(
    client: &mut CdpClient,
    session_id: &str,
    base_url: &str,
) -> Result<SignInOutput, String> {
    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    let clicked = evaluate_bool_via_cdp(client, session_id, CLICK_SIGN_IN_SCRIPT)?;
    if clicked {
        thread::sleep(Duration::from_millis(2500));
    } else {
        for url in [
            format!("{}/attendance.php?action=sign", base_url),
            format!("{}/attendance.php?do=sign", base_url),
            format!("{}/attendance.php?sign=1", base_url),
        ] {
            if navigate_via_cdp(client, session_id, &url, "尝试备用签到地址失败").is_ok()
                && wait_for_cloudflare(client, session_id).is_ok()
            {
                let text = page_text_via_cdp(client, session_id)?;
                if let Some(result) = classify_sign_in_text(&text) {
                    return Ok(result);
                }
            }
        }
    }

    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    Ok(SignInOutput {
        status: if clicked { "success" } else { "failed" }.to_string(),
        message: if clicked {
            compact_text(&text).unwrap_or_else(|| "已尝试点击签到按钮".to_string())
        } else {
            "未找到 NexusPHP 签到入口".to_string()
        },
    })
}

fn run_ocr_captcha_sign_in(
    client: &mut CdpClient,
    session_id: &str,
    base_url: &str,
    ocr_api_key: Option<&str>,
) -> Result<SignInOutput, String> {
    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    let clicked = evaluate_bool_via_cdp(client, session_id, CLICK_SIGN_IN_SCRIPT)?;
    if clicked {
        thread::sleep(Duration::from_millis(2500));
        if handle_captcha_if_present(client, session_id, ocr_api_key)? {
            thread::sleep(Duration::from_millis(2500));
        }
    } else {
        for url in [
            format!("{}/attendance.php?action=sign", base_url),
            format!("{}/attendance.php?do=sign", base_url),
            format!("{}/attendance.php?sign=1", base_url),
        ] {
            if navigate_via_cdp(client, session_id, &url, "尝试备用签到地址失败").is_ok()
                && wait_for_cloudflare(client, session_id).is_ok()
            {
                let text = page_text_via_cdp(client, session_id)?;
                if let Some(result) = classify_sign_in_text(&text) {
                    return Ok(result);
                }
                if handle_captcha_if_present(client, session_id, ocr_api_key)? {
                    thread::sleep(Duration::from_millis(2500));
                }
            }
        }
    }

    let text = page_text_via_cdp(client, session_id)?;
    if let Some(result) = classify_sign_in_text(&text) {
        return Ok(result);
    }

    Ok(SignInOutput {
        status: if clicked { "success" } else { "failed" }.to_string(),
        message: if clicked {
            compact_text(&text).unwrap_or_else(|| "已尝试点击签到按钮".to_string())
        } else {
            "未找到 NexusPHP 签到入口".to_string()
        },
    })
}

fn set_cookies_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    base_url: &str,
    cookie: &str,
) -> Result<(), String> {
    let cookies = parse_cookie_pairs(cookie)
        .into_iter()
        .map(|(name, value)| {
            json!({
                "name": name,
                "value": value,
                "url": base_url,
                "path": "/",
                "secure": base_url.starts_with("https://")
            })
        })
        .collect::<Vec<_>>();
    if cookies.is_empty() {
        return Err("Cookie 不能为空".to_string());
    }
    client.call(
        "Network.setCookies",
        json!({ "cookies": cookies }),
        Some(session_id),
    )?;
    Ok(())
}

fn navigate_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    url: &str,
    context: &str,
) -> Result<(), String> {
    let result = client.call("Page.navigate", json!({ "url": url }), Some(session_id));
    match result {
        Ok(value) => {
            if let Some(error_text) = value.get("errorText").and_then(Value::as_str) {
                return Err(format!("{context}: {error_text}"));
            }
            thread::sleep(Duration::from_secs(2));
            Ok(())
        }
        Err(error) => Err(format!("{context}: {error}")),
    }
}

fn page_text_via_cdp(client: &mut CdpClient, session_id: &str) -> Result<String, String> {
    evaluate_string_via_cdp(
        client,
        session_id,
        "document.body ? document.body.innerText : document.documentElement.innerText",
    )
}

fn wait_for_cloudflare(client: &mut CdpClient, session_id: &str) -> Result<(), String> {
    const MAX_WAIT: Duration = Duration::from_secs(10);
    const POLL_INTERVAL: Duration = Duration::from_secs(2);

    let started = Instant::now();
    loop {
        let title =
            evaluate_string_via_cdp(client, session_id, "document.title").unwrap_or_default();
        let body = page_text_via_cdp(client, session_id).unwrap_or_default();
        if !is_cloudflare_challenge(&title, &body) {
            return Ok(());
        }

        try_click_turnstile(client, session_id);

        if started.elapsed() >= MAX_WAIT {
            return Err("Cloudflare 挑战未通过，请检查 cf_clearance cookie 或代理".to_string());
        }
        thread::sleep(POLL_INTERVAL);
    }
}

fn is_cloudflare_challenge(title: &str, body: &str) -> bool {
    let lower_title = title.to_ascii_lowercase();
    let lower_body = body.to_ascii_lowercase();
    lower_title.contains("just a moment")
        || lower_body.contains("cf-challenge")
        || lower_body.contains("checking your browser")
        || lower_body.contains("checking if the site connection is secure")
        || lower_body.contains("cf_chl_opt")
        || lower_body.contains("turnstile")
}

fn try_click_turnstile(client: &mut CdpClient, session_id: &str) {
    let coords = evaluate_string_via_cdp(
        client,
        session_id,
        r#"(() => {
            const iframe = document.querySelector('iframe[src*="challenges.cloudflare.com"], iframe[id^="cf-chl-widget"], iframe[title*="Cloudflare"]');
            if (!iframe) return '';
            const r = iframe.getBoundingClientRect();
            if (r.width === 0 || r.height === 0) return '';
            return JSON.stringify({ x: r.x + 28, y: r.y + r.height / 2 });
        })()"#,
    )
    .unwrap_or_default();

    if coords.is_empty() {
        return;
    }
    let Ok(value) = serde_json::from_str::<Value>(&coords) else {
        return;
    };
    let Some(x) = value.get("x").and_then(Value::as_f64) else {
        return;
    };
    let Some(y) = value.get("y").and_then(Value::as_f64) else {
        return;
    };

    let _ = client.call(
        "Input.dispatchMouseEvent",
        json!({ "type": "mousePressed", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        Some(session_id),
    );
    let _ = client.call(
        "Input.dispatchMouseEvent",
        json!({ "type": "mouseReleased", "x": x, "y": y, "button": "left", "clickCount": 1 }),
        Some(session_id),
    );
}

fn handle_captcha_if_present(
    client: &mut CdpClient,
    session_id: &str,
    ocr_api_key: Option<&str>,
) -> Result<bool, String> {
    let data_url = evaluate_string_await_via_cdp(
        client,
        session_id,
        EXTRACT_CAPTCHA_IMAGE_SCRIPT,
        Duration::from_secs(5),
    )?;
    if data_url.trim().is_empty() {
        return Ok(false);
    }

    let api_key = match ocr_api_key.map(str::trim).filter(|v| !v.is_empty()) {
        Some(key) => key.to_string(),
        None => return Err("页面要求图片验证码，但未配置 OCR API key".to_string()),
    };

    let code = ocr_space_recognize(&api_key, &data_url)?;
    let code = code.trim().to_string();
    if code.is_empty() {
        return Err("OCR 识别结果为空".to_string());
    }

    let filled = evaluate_bool_via_cdp(client, session_id, &fill_captcha_script(&code))?;
    if !filled {
        return Err("验证码识别成功但未找到输入框".to_string());
    }
    Ok(true)
}

fn ocr_space_recognize(api_key: &str, data_url: &str) -> Result<String, String> {
    let handle = tokio::runtime::Handle::try_current()
        .map_err(|e| format!("获取 tokio handle 失败: {}", e))?;
    handle
        .block_on(async {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .map_err(|e| format!("构建 OCR HTTP 客户端失败: {}", e))?;
            let form = reqwest::multipart::Form::new()
                .text("apikey", api_key.to_string())
                .text("language", "auto".to_string())
                .text("scale", "true".to_string())
                .text("base64Image", data_url.to_string());
            let resp = client
                .post("https://api.ocr.space/parse/image")
                .multipart(form)
                .send()
                .await
                .map_err(|e| format!("OCR 请求失败: {}", e))?;
            let value: Value = resp
                .json()
                .await
                .map_err(|e| format!("解析 OCR 响应失败: {}", e))?;
            if value
                .get("IsErroredOnProcessing")
                .and_then(Value::as_bool)
                .unwrap_or(false)
            {
                let msg = value
                    .get("ErrorMessage")
                    .and_then(Value::as_str)
                    .unwrap_or("未知错误");
                return Err(format!("OCR 处理出错: {}", msg));
            }
            let text = value
                .get("ParsedResults")
                .and_then(|v| v.get(0))
                .and_then(|v| v.get("ParsedText"))
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string();
            Ok(text)
        })
        .map_err(|e| format!("OCR 任务失败: {}", e))
}

fn evaluate_string_await_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    expression: &str,
    _timeout: Duration,
) -> Result<String, String> {
    let value = client.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true,
            "awaitPromise": true,
        }),
        Some(session_id),
    )?;
    Ok(value
        .get("result")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn fill_captcha_script(code: &str) -> String {
    let escaped = code
        .replace('\\', "\\\\")
        .replace('\'', "\\'")
        .replace('\n', "\\n");
    format!(
        r#"
(() => {{
  const code = '{}';
  const visible = (el) => {{
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && el.offsetParent !== null;
  }};
  const inputs = Array.from(document.querySelectorAll('input[type="text"], input:not([type])'))
    .filter(visible)
    .filter((el) => !/search|搜索|username|user|password|email/i.test(el.name + ' ' + (el.placeholder || '') + ' ' + (el.id || '')));
  if (inputs.length === 0) return false;
  const input = inputs[0];
  const setter = Object.getOwnPropertyDescriptor(window.HTMLInputElement.prototype, 'value')?.set;
  if (setter) setter.call(input, code); else input.value = code;
  input.dispatchEvent(new Event('input', {{ bubbles: true }}));
  input.dispatchEvent(new Event('change', {{ bubbles: true }}));
  const form = input.form;
  const submitBtn = form
    ? form.querySelector('input[type="submit"], button[type="submit"], button')
    : null;
  if (submitBtn) {{
    submitBtn.click();
  }} else if (form) {{
    form.submit();
  }} else {{
    const btns = Array.from(document.querySelectorAll('button, input[type="button"], a'))
      .filter(visible)
      .filter((el) => /签到|簽到|打卡|确认|確認|submit|ok|verify/i.test([el.innerText, el.value, el.title].filter(Boolean).join(' ')));
    if (btns.length > 0) btns[0].click(); else return false;
  }}
  return true;
}})()
"#,
        escaped
    )
}

const EXTRACT_CAPTCHA_IMAGE_SCRIPT: &str = r#"
(async () => {
  const visible = (el) => {
    const r = el.getBoundingClientRect();
    return r.width > 0 && r.height > 0 && el.offsetParent !== null;
  };
  const selectors = [
    'img[src*="code" i]', 'img[src*="captcha" i]', 'img[src*="verify" i]',
    'img[src*="image_code" i]', 'img[id*="code" i]', 'img[alt*="captcha" i]',
    'img[alt*="code" i]', 'img[alt*="verify" i]'
  ];
  const imgs = Array.from(document.querySelectorAll(selectors.join(','))).filter(visible);
  if (imgs.length === 0) return '';
  const img = imgs[0];
  if (!img.src) return '';
  try {
    const resp = await fetch(img.src, { credentials: 'include' });
    const blob = await resp.blob();
    const dataUrl = await new Promise((resolve, reject) => {
      const reader = new FileReader();
      reader.onloadend = () => resolve(reader.result);
      reader.onerror = reject;
      reader.readAsDataURL(blob);
    });
    return dataUrl || '';
  } catch (e) {
    return '';
  }
})()
"#;

fn evaluate_string_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    expression: &str,
) -> Result<String, String> {
    let value = client.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true
        }),
        Some(session_id),
    )?;
    Ok(value
        .get("result")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string())
}

fn evaluate_bool_via_cdp(
    client: &mut CdpClient,
    session_id: &str,
    expression: &str,
) -> Result<bool, String> {
    let value = client.call(
        "Runtime.evaluate",
        json!({
            "expression": expression,
            "returnByValue": true
        }),
        Some(session_id),
    )?;
    Ok(value
        .get("result")
        .and_then(|v| v.get("value"))
        .and_then(Value::as_bool)
        .unwrap_or(false))
}

fn build_lightpanda_endpoint(
    config: &LightpandaConfig,
    use_proxy_for_lightpanda: bool,
) -> Result<String, String> {
    if let Some(endpoint) = config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(endpoint.to_string());
    }

    let token = config.token.as_deref().unwrap_or_default().trim();
    if token.is_empty() {
        return Err("Lightpanda Token 不能为空".to_string());
    }

    let mut endpoint = format!(
        "wss://{}.cloud.lightpanda.io/ws?token={}",
        normalize_region(&config.region),
        urlencoding::encode(token)
    );
    if !config.browser.trim().is_empty() {
        endpoint.push_str("&browser=");
        endpoint.push_str(&urlencoding::encode(config.browser.trim()));
    }
    if let Some(proxy) = use_proxy_for_lightpanda
        .then_some(config.proxy.as_deref())
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        endpoint.push_str("&proxy=");
        endpoint.push_str(&urlencoding::encode(proxy));
    }
    if let Some(country) = config
        .country
        .as_deref()
        .map(str::trim)
        .filter(|v| !v.is_empty())
    {
        endpoint.push_str("&country=");
        endpoint.push_str(&urlencoding::encode(country));
    }
    Ok(endpoint)
}

fn normalize_region(region: &str) -> &str {
    match region.trim() {
        "uswest" => "uswest",
        _ => "euwest",
    }
}

pub fn normalize_sign_in_browser(browser: &str) -> Option<&'static str> {
    match browser.trim().to_ascii_lowercase().as_str() {
        SIGN_IN_BROWSER_LIGHTPANDA => Some(SIGN_IN_BROWSER_LIGHTPANDA),
        SIGN_IN_BROWSER_CLOAKBROWSER => Some(SIGN_IN_BROWSER_CLOAKBROWSER),
        _ => None,
    }
}

fn with_cloakbrowser<T>(
    config: CloakBrowserConfig,
    operation: impl FnOnce(String) -> Result<T, String>,
) -> Result<T, String> {
    let _guard = CLOAKBROWSER_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut browser = CloakBrowserProcess::launch(&config)?;
    let operation_result = operation(browser.endpoint.clone());
    let close_result = browser.close();

    match (operation_result, close_result) {
        (Ok(value), Ok(())) => Ok(value),
        (Err(error), Ok(())) => Err(error),
        (Ok(_), Err(close_error)) => Err(close_error),
        (Err(error), Err(close_error)) => {
            Err(format!("{}；CloakBrowser 关闭失败: {}", error, close_error))
        }
    }
}

#[derive(Deserialize)]
struct CloakBrowserLauncherMessage {
    endpoint: Option<String>,
    error: Option<String>,
}

struct CloakBrowserProcess {
    endpoint: String,
    child: Option<Child>,
    stdin: Option<ChildStdin>,
    _stdout: BufReader<ChildStdout>,
}

impl CloakBrowserProcess {
    fn launch(config: &CloakBrowserConfig) -> Result<Self, String> {
        if config
            .license_key
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .is_none()
        {
            return Err("CloakBrowser license key 不能为空".to_string());
        }

        let payload = serde_json::to_string(config)
            .map_err(|error| format!("序列化 CloakBrowser 配置失败: {}", error))?;
        let mut child = spawn_cloakbrowser_helper()?;
        let Some(mut stdin) = child.stdin.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("无法打开 CloakBrowser 启动器 stdin".to_string());
        };
        if let Err(error) = writeln!(stdin, "{}", payload).and_then(|_| stdin.flush()) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!("发送 CloakBrowser 配置失败: {}", error));
        }

        let Some(stdout) = child.stdout.take() else {
            let _ = child.kill();
            let _ = child.wait();
            return Err("无法打开 CloakBrowser 启动器 stdout".to_string());
        };
        let mut stdout = BufReader::new(stdout);
        for _ in 0..2_000 {
            let mut line = String::new();
            let read = match stdout.read_line(&mut line) {
                Ok(read) => read,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("读取 CloakBrowser 启动结果失败: {}", error));
                }
            };
            if read == 0 {
                let status = child.try_wait().ok().flatten();
                let still_running = status.is_none();
                let error = match status {
                    Some(status) => format!("CloakBrowser 启动器提前退出: {}", status),
                    None => "CloakBrowser 启动器未返回 CDP endpoint".to_string(),
                };
                if still_running {
                    let _ = child.kill();
                    let _ = child.wait();
                }
                return Err(error);
            }
            let Ok(message) = serde_json::from_str::<CloakBrowserLauncherMessage>(&line) else {
                continue;
            };
            if let Some(error) = message.error {
                let _ = child.wait();
                return Err(redact_cloakbrowser_error(&error, config));
            }
            if let Some(endpoint) = message
                .endpoint
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
            {
                return Ok(Self {
                    endpoint,
                    child: Some(child),
                    stdin: Some(stdin),
                    _stdout: stdout,
                });
            }
        }

        let _ = child.kill();
        let _ = child.wait();
        Err("CloakBrowser 启动器输出过多，未找到 CDP endpoint".to_string())
    }

    fn close(&mut self) -> Result<(), String> {
        let Some(mut child) = self.child.take() else {
            return Ok(());
        };
        let write_error = self.stdin.take().and_then(|mut stdin| {
            writeln!(stdin, "close")
                .and_then(|_| stdin.flush())
                .err()
                .map(|error| error.to_string())
        });

        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        return Err(format!("CloakBrowser 启动器退出状态异常: {}", status));
                    }
                    if let Some(error) = write_error {
                        return Err(format!("发送 close 指令失败: {}", error));
                    }
                    return Ok(());
                }
                Ok(None) if Instant::now() < deadline => {
                    thread::sleep(Duration::from_millis(100));
                }
                Ok(None) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("等待 CloakBrowser close 超时，已终止进程".to_string());
                }
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!("等待 CloakBrowser 关闭失败: {}", error));
                }
            }
        }
    }
}

impl Drop for CloakBrowserProcess {
    fn drop(&mut self) {
        let _ = self.close();
    }
}

fn spawn_cloakbrowser_helper() -> Result<Child, String> {
    let executables = std::env::var("CLOAKBROWSER_PYTHON")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(|value| vec![value])
        .unwrap_or_else(|| vec!["python3".to_string(), "python".to_string()]);
    let mut errors = Vec::new();
    for executable in executables {
        match Command::new(&executable)
            .args(["-u", "-c", CLOAKBROWSER_LAUNCHER])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => return Ok(child),
            Err(error) => errors.push(format!("{}: {}", executable, error)),
        }
    }
    Err(format!(
        "无法启动 Python；请安装 Python 3 和 cloakbrowser[geoip]：{}",
        errors.join("；")
    ))
}

fn redact_cloakbrowser_error(message: &str, config: &CloakBrowserConfig) -> String {
    let mut redacted = message.to_string();
    let mut secrets = [config.license_key.as_deref(), config.proxy.as_deref()]
        .into_iter()
        .flatten()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    if let Some(proxy) = config.proxy.as_deref()
        && let Ok(proxy_url) = reqwest::Url::parse(proxy.trim())
    {
        if !proxy_url.username().is_empty() {
            secrets.push(proxy_url.username().to_string());
        }
        if let Some(password) = proxy_url.password().filter(|value| !value.is_empty()) {
            secrets.push(password.to_string());
        }
    }
    secrets.sort_by_key(|secret| std::cmp::Reverse(secret.len()));
    secrets.dedup();
    for secret in secrets {
        redacted = redacted.replace(&secret, "[redacted]");
    }
    format!("CloakBrowser 启动失败: {}", redacted)
}

const CLOAKBROWSER_LAUNCHER: &str = r#"
import json
import socket
import sys
import time
import urllib.request

browser = None

def emit(payload):
    print(json.dumps(payload, ensure_ascii=True), flush=True)

try:
    from cloakbrowser import launch

    config = json.loads(sys.stdin.readline())
    with socket.socket(socket.AF_INET, socket.SOCK_STREAM) as port_socket:
        port_socket.bind(("127.0.0.1", 0))
        port = port_socket.getsockname()[1]

    options = {
        "license_key": config.get("license_key"),
        "headless": bool(config.get("headless", False)),
        "humanize": bool(config.get("humanize", True)),
        "human_preset": config.get("human_preset") or "careful",
        "geoip": bool(config.get("geoip", True)),
        "args": [
            "--remote-debugging-address=127.0.0.1",
            f"--remote-debugging-port={port}",
        ],
    }
    proxy = (config.get("proxy") or "").strip()
    if proxy:
        options["proxy"] = proxy

    browser = launch(**options)
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    endpoint = None
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        try:
            with opener.open(f"http://127.0.0.1:{port}/json/version", timeout=1) as response:
                endpoint = json.load(response).get("webSocketDebuggerUrl")
            if endpoint:
                break
        except Exception:
            time.sleep(0.1)
    if not endpoint:
        raise TimeoutError("CDP endpoint did not become ready within 30 seconds")

    emit({"endpoint": endpoint})
    sys.stdin.readline()
except Exception as error:
    emit({"error": f"{type(error).__name__}: {error}"})
finally:
    if browser is not None:
        browser.close()
"#;

#[derive(Debug)]
struct SignInOutput {
    status: String,
    message: String,
}

fn parse_cookie_pairs(cookie: &str) -> Vec<(String, String)> {
    cookie
        .split(';')
        .filter_map(|part| {
            let (name, value) = part.trim().split_once('=')?;
            let name = name.trim();
            if name.is_empty() {
                None
            } else {
                Some((name.to_string(), value.trim().to_string()))
            }
        })
        .collect()
}

fn looks_logged_out(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    (lower.contains("login") || text.contains("用户登录") || text.contains("會員登入"))
        && !(lower.contains("logout") || text.contains("登出") || text.contains("退出"))
}

fn classify_sign_in_text(text: &str) -> Option<SignInOutput> {
    let compact = compact_text(text)?;
    let lower = compact.to_ascii_lowercase();
    if compact.contains("已签到")
        || compact.contains("已经签到")
        || compact.contains("今日已签")
        || compact.contains("今天已签")
        || compact.contains("已打卡")
        || lower.contains("already")
    {
        return Some(SignInOutput {
            status: "already".to_string(),
            message: compact,
        });
    }
    if compact.contains("签到成功")
        || compact.contains("簽到成功")
        || compact.contains("打卡成功")
        || compact.contains("成功签到")
        || lower.contains("success")
    {
        return Some(SignInOutput {
            status: "success".to_string(),
            message: compact,
        });
    }
    None
}

fn compact_text(text: &str) -> Option<String> {
    let compact = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if compact.is_empty() {
        None
    } else {
        Some(compact.chars().take(240).collect())
    }
}

const CLICK_SIGN_IN_SCRIPT: &str = r#"
(() => {
  const candidates = Array.from(document.querySelectorAll('button,input[type="button"],input[type="submit"],a'));
  const target = candidates.find((node) => {
    const label = [node.innerText, node.value, node.title, node.getAttribute('aria-label')]
      .filter(Boolean)
      .join(' ');
    return /签到|簽到|打卡|attendance|sign/i.test(label);
  });
  if (!target) return false;
  target.click();
  return true;
})()
"#;

pub const SIGN_IN_METHOD_OPEN_PAGE: &str = "open_page";
pub const SIGN_IN_METHOD_CLOUDFLARE: &str = "cloudflare";
pub const SIGN_IN_METHOD_OCR_CAPTCHA: &str = "ocr_captcha";

pub const SIGN_IN_METHODS: &[&str] = &[
    SIGN_IN_METHOD_OPEN_PAGE,
    SIGN_IN_METHOD_CLOUDFLARE,
    SIGN_IN_METHOD_OCR_CAPTCHA,
];

pub fn normalize_sign_in_method(value: &str) -> String {
    let trimmed = value.trim();
    if SIGN_IN_METHODS.contains(&trimmed) {
        return trimmed.to_string();
    }
    SIGN_IN_METHOD_OPEN_PAGE.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    #[test]
    fn lightpanda_endpoint_uses_global_browser_configuration() {
        let config = LightpandaConfig {
            endpoint: None,
            token: Some("token with space".to_string()),
            region: "uswest".to_string(),
            browser: "chrome".to_string(),
            proxy: Some("datacenter".to_string()),
            country: Some("DE".to_string()),
        };

        assert_eq!(
            build_lightpanda_endpoint(&config, true).unwrap(),
            "wss://uswest.cloud.lightpanda.io/ws?token=token%20with%20space&browser=chrome&proxy=datacenter&country=DE"
        );
        assert_eq!(
            build_lightpanda_endpoint(&config, false).unwrap(),
            "wss://uswest.cloud.lightpanda.io/ws?token=token%20with%20space&browser=chrome&country=DE"
        );
    }

    #[test]
    fn sign_in_browser_only_accepts_supported_providers() {
        assert_eq!(
            normalize_sign_in_browser(" CloakBrowser "),
            Some(SIGN_IN_BROWSER_CLOAKBROWSER)
        );
        assert_eq!(
            normalize_sign_in_browser("lightpanda"),
            Some(SIGN_IN_BROWSER_LIGHTPANDA)
        );
        assert_eq!(normalize_sign_in_browser("chrome"), None);
    }

    #[test]
    fn cloakbrowser_errors_redact_credentials() {
        let config = CloakBrowserConfig {
            license_key: Some("cb_secret".to_string()),
            proxy: Some("http://secret_user:secret_pass@proxy:8080".to_string()),
            ..CloakBrowserConfig::default()
        };
        let message = redact_cloakbrowser_error(
            "launch cb_secret through http://secret_user:secret_pass@proxy:8080 failed for secret_user / secret_pass",
            &config,
        );

        assert!(!message.contains("cb_secret"));
        assert!(!message.contains("secret_user"));
        assert!(!message.contains("secret_pass"));
        assert_eq!(message.matches("[redacted]").count(), 4);
    }

    #[test]
    fn cron_0_8_hour_schedule_next() {
        let expr = "0 0 0/8 * * *";
        let schedule: cron::Schedule = expr.parse().expect("cron parse");
        let now = chrono::Utc.with_ymd_and_hms(2026, 6, 27, 7, 59, 0).unwrap();
        let next = schedule.after(&now).next();
        assert_eq!(
            next.map(|t| t.format("%H:%M:%S").to_string()),
            Some("08:00:00".to_string()),
            "next after 07:59 should be 08:00"
        );

        let now2 = chrono::Utc.with_ymd_and_hms(2026, 6, 27, 8, 0, 1).unwrap();
        let next2 = schedule.after(&now2).next();
        assert_eq!(
            next2.map(|t| t.format("%H:%M:%S").to_string()),
            Some("16:00:00".to_string()),
            "next after 08:00:01 should be 16:00"
        );

        let now3 = chrono::Utc
            .with_ymd_and_hms(2026, 6, 27, 7, 59, 30)
            .unwrap();
        let next3 = schedule.after(&now3).next();
        let diff = (next3.unwrap() - now3).num_seconds();
        assert_eq!(diff, 30, "diff at 07:59:30 should be 30s");
    }
}
