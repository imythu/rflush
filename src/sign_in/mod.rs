pub mod scheduler;

use std::thread;
use std::time::{Duration, Instant};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tungstenite::{Message, connect};

use crate::site::{SiteAuth, SiteRecord};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignInTaskRecord {
    pub id: i64,
    pub name: String,
    pub site_id: i64,
    pub cron_expression: String,
    pub lightpanda_endpoint: Option<String>,
    pub lightpanda_token: String,
    pub lightpanda_region: String,
    pub browser: String,
    pub proxy: String,
    pub country: Option<String>,
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
    pub lightpanda_endpoint: Option<String>,
    pub lightpanda_token: String,
    pub lightpanda_region: Option<String>,
    pub browser: Option<String>,
    pub proxy: Option<String>,
    pub country: Option<String>,
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
pub struct LightpandaProbeResult {
    pub success: bool,
    pub url: String,
    pub message: String,
    pub title: Option<String>,
}

pub async fn probe_lightpanda_1_1_1_1(
    task: SignInTaskRecord,
    use_proxy_for_lightpanda: bool,
) -> Result<LightpandaProbeResult, String> {
    let endpoint = build_lightpanda_endpoint(&task, use_proxy_for_lightpanda)?;
    probe_lightpanda_endpoint_1_1_1_1(endpoint).await
}

pub async fn probe_lightpanda_request_1_1_1_1(
    request: SignInTaskRequest,
    use_proxy_for_lightpanda: bool,
) -> Result<LightpandaProbeResult, String> {
    let task = SignInTaskRecord {
        id: 0,
        name: request.name,
        site_id: request.site_id,
        cron_expression: request.cron_expression,
        lightpanda_endpoint: request.lightpanda_endpoint,
        lightpanda_token: request.lightpanda_token,
        lightpanda_region: request
            .lightpanda_region
            .unwrap_or_else(|| "euwest".to_string()),
        browser: request.browser.unwrap_or_else(|| "lightpanda".to_string()),
        proxy: request.proxy.unwrap_or_else(|| "fast_dc".to_string()),
        country: request.country,
        sign_in_method: request
            .sign_in_method
            .unwrap_or_else(|| SIGN_IN_METHOD_OPEN_PAGE.to_string()),
        enabled: true,
        last_status: None,
        last_message: None,
        last_run_at: None,
        created_at: String::new(),
        updated_at: String::new(),
    };
    let endpoint = build_lightpanda_endpoint(&task, use_proxy_for_lightpanda)?;
    probe_lightpanda_endpoint_1_1_1_1(endpoint).await
}

async fn probe_lightpanda_endpoint_1_1_1_1(
    endpoint: String,
) -> Result<LightpandaProbeResult, String> {
    tokio::task::spawn_blocking(move || run_cdp_probe(endpoint, "https://1.1.1.1"))
        .await
        .map_err(|e| format!("Lightpanda 探测任务 join 失败: {}", e))?
}

pub async fn execute_task(
    _base_dir: std::path::PathBuf,
    task: SignInTaskRecord,
    site: SiteRecord,
    use_proxy_for_lightpanda: bool,
    ocr_api_key: Option<String>,
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

    let endpoint = build_lightpanda_endpoint(&task, use_proxy_for_lightpanda)?;
    let base_url = site.base_url.trim_end_matches('/').to_string();
    let started_at = Utc::now().to_rfc3339();
    let output =
        run_cdp_sign_in(endpoint, base_url, cookie, task.sign_in_method, ocr_api_key).await?;
    let finished_at = Utc::now().to_rfc3339();

    Ok(SignInResult {
        status: output.status,
        message: output.message,
        started_at,
        finished_at,
    })
}

fn run_cdp_probe(endpoint: String, url: &str) -> Result<LightpandaProbeResult, String> {
    let mut client = CdpClient::connect(endpoint)?;
    let session_id = create_target_session(&mut client)?;

    let _ = client.call("Page.enable", json!({}), Some(&session_id));
    let navigate = client.call("Page.navigate", json!({ "url": url }), Some(&session_id));
    if let Err(error) = navigate {
        return Ok(LightpandaProbeResult {
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

    Ok(LightpandaProbeResult {
        success: true,
        url: url.to_string(),
        message: "Lightpanda 已成功导航到 1.1.1.1".to_string(),
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
            connect(endpoint.as_str()).map_err(|e| format!("连接 Lightpanda CDP 失败: {}", e))?;
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

async fn run_cdp_sign_in(
    endpoint: String,
    base_url: String,
    cookie: String,
    sign_in_method: String,
    ocr_api_key: Option<String>,
) -> Result<SignInOutput, String> {
    tokio::task::spawn_blocking(move || {
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
            SIGN_IN_METHOD_CLOUDFLARE => {
                run_cloudflare_sign_in(&mut client, &session_id, &base_url)
            }
            SIGN_IN_METHOD_OCR_CAPTCHA => {
                run_ocr_captcha_sign_in(&mut client, &session_id, &base_url, ocr_api_key.as_deref())
            }
            other => Err(format!("未知签到方式: {}", other)),
        }
    })
    .await
    .map_err(|e| format!("签到任务 join 失败: {}", e))?
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
    task: &SignInTaskRecord,
    use_proxy_for_lightpanda: bool,
) -> Result<String, String> {
    if let Some(endpoint) = task
        .lightpanda_endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(endpoint.to_string());
    }

    let token = task.lightpanda_token.trim();
    if token.is_empty() {
        return Err("Lightpanda Token 不能为空".to_string());
    }

    let mut endpoint = format!(
        "wss://{}.cloud.lightpanda.io/ws?token={}",
        normalize_region(&task.lightpanda_region),
        urlencoding::encode(token)
    );
    if !task.browser.trim().is_empty() {
        endpoint.push_str("&browser=");
        endpoint.push_str(&urlencoding::encode(task.browser.trim()));
    }
    if use_proxy_for_lightpanda && !task.proxy.trim().is_empty() {
        endpoint.push_str("&proxy=");
        endpoint.push_str(&urlencoding::encode(task.proxy.trim()));
    }
    if let Some(country) = task
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
    use chrono::TimeZone;

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
