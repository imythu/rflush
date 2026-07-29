use std::collections::HashSet;
use std::time::Duration;

use reqwest::header::{
    CONTENT_ENCODING, CONTENT_LENGTH, CONTENT_TYPE, LOCATION, TRANSFER_ENCODING,
};
use reqwest::{Client, Method, Request, RequestBuilder, Response, StatusCode, Url};
use tokio::sync::{Mutex, MutexGuard};
use tokio::time::{Instant, sleep_until};

use super::{
    IndexerError, IndexerRateLimit, http_error, rate_limit_error, resolve_same_origin_url,
    response_is_authentication_page,
};

const MAX_RATE_LIMIT_COOLDOWN: Duration = Duration::from_secs(24 * 60 * 60);
const MAX_SAME_ORIGIN_REDIRECTS: usize = 5;

#[derive(Clone, Copy)]
pub(crate) struct IndexerAccessPolicy {
    pub min_request_interval: Duration,
    pub default_cooldown: Duration,
}

pub(crate) fn default_indexer_access_policy() -> IndexerAccessPolicy {
    #[cfg(test)]
    {
        IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        }
    }
    #[cfg(not(test))]
    {
        IndexerAccessPolicy {
            min_request_interval: Duration::from_secs(1),
            default_cooldown: Duration::from_secs(60),
        }
    }
}

pub(crate) struct OriginAccessGate {
    operation: Mutex<()>,
    request: Mutex<()>,
    rate: Mutex<OriginRateState>,
    policy: IndexerAccessPolicy,
}

struct OriginRateState {
    next_request_at: Instant,
    blocked_until: Option<Instant>,
}

impl OriginAccessGate {
    pub(crate) fn new(policy: IndexerAccessPolicy) -> Self {
        Self {
            operation: Mutex::new(()),
            request: Mutex::new(()),
            rate: Mutex::new(OriginRateState {
                next_request_at: Instant::now(),
                blocked_until: None,
            }),
            policy,
        }
    }

    pub(crate) async fn lock_operation(&self) -> MutexGuard<'_, ()> {
        self.operation.lock().await
    }

    pub(crate) async fn send(&self, request: RequestBuilder) -> Result<Response, IndexerError> {
        let _request = self.request.lock().await;
        self.before_request().await?;
        let response = request.send().await.map_err(http_error)?;
        self.handle_rate_limit(response).await
    }

    /// Follow a bounded chain of same-origin redirects with every network request
    /// passing through this gate. The indexer clients disable reqwest's automatic redirects so
    /// redirects cannot bypass origin throttling or leak tracker credentials to another origin.
    pub(crate) async fn send_with_same_origin_redirects(
        &self,
        client: &Client,
        request: RequestBuilder,
        base_url: &Url,
    ) -> Result<Response, IndexerError> {
        self.send_with_same_origin_redirects_rebuilding(client, request, base_url, |_, _| None)
            .await
    }

    /// Variant for streaming request bodies such as multipart forms. `rebuild_request` is used
    /// only when a 307/308 requires replaying a body that reqwest cannot clone.
    pub(crate) async fn send_with_same_origin_redirects_rebuilding<F>(
        &self,
        client: &Client,
        request: RequestBuilder,
        base_url: &Url,
        rebuild_request: F,
    ) -> Result<Response, IndexerError>
    where
        F: Fn(&Method, &Url) -> Option<RequestBuilder> + Send + Sync,
    {
        let mut request = request.build().map_err(http_error)?;

        let mut visited = HashSet::with_capacity(MAX_SAME_ORIGIN_REDIRECTS + 1);
        for redirect_count in 0..=MAX_SAME_ORIGIN_REDIRECTS {
            let current_url = resolve_same_origin_url(base_url, request.url().as_str())?;
            if response_is_authentication_page(&current_url, "") {
                return Err(IndexerError::AuthenticationExpired(
                    "tracker redirected to a login or verification page".to_string(),
                ));
            }
            if !visited.insert(redirect_identity(&current_url)) {
                return Err(IndexerError::Http(
                    "tracker redirect loop detected".to_string(),
                ));
            }
            *request.url_mut() = current_url;

            let method = request.method().clone();
            let headers = request.headers().clone();
            let replay_request = request.try_clone();
            let response = self.execute(client, request).await?;
            let status = response.status();
            if !is_follow_redirect(status) {
                return Ok(response);
            }
            if redirect_count == MAX_SAME_ORIGIN_REDIRECTS {
                return Err(IndexerError::Http(
                    "tracker response exceeded the redirect limit".to_string(),
                ));
            }

            let location = response
                .headers()
                .get(LOCATION)
                .and_then(|value| value.to_str().ok())
                .ok_or_else(|| {
                    IndexerError::Http("tracker redirect has no valid location".to_string())
                })?;
            let mut next_url = response.url().join(location).map_err(|_| {
                IndexerError::UnsafeUrl("tracker redirect location is invalid".to_string())
            })?;
            next_url.set_fragment(None);
            let next_url = resolve_same_origin_url(base_url, next_url.as_str())?;
            if response_is_authentication_page(&next_url, "") {
                return Err(IndexerError::AuthenticationExpired(
                    "tracker redirected to a login or verification page".to_string(),
                ));
            }
            if visited.contains(&redirect_identity(&next_url)) {
                return Err(IndexerError::Http(
                    "tracker redirect loop detected".to_string(),
                ));
            }
            request = redirected_request(
                status,
                method,
                headers,
                replay_request,
                next_url,
                &rebuild_request,
            )?;
        }

        unreachable!("bounded redirect loop always returns")
    }

    async fn execute(&self, client: &Client, request: Request) -> Result<Response, IndexerError> {
        let _request = self.request.lock().await;
        self.before_request().await?;
        let response = client.execute(request).await.map_err(http_error)?;
        self.handle_rate_limit(response).await
    }

    async fn handle_rate_limit(&self, response: Response) -> Result<Response, IndexerError> {
        if response.status() == StatusCode::TOO_MANY_REQUESTS {
            let error = rate_limit_error(&response);
            if let IndexerError::RateLimited(limit) = error {
                let normalized = self.observe_rate_limit(limit).await;
                return Err(IndexerError::RateLimited(normalized));
            }
        }
        Ok(response)
    }

    async fn before_request(&self) -> Result<(), IndexerError> {
        let mut state = self.rate.lock().await;
        let now = Instant::now();
        if let Some(blocked_until) = state.blocked_until {
            if blocked_until > now {
                let remaining = blocked_until.saturating_duration_since(now);
                return Err(IndexerError::RateLimited(IndexerRateLimit::new(Some(
                    duration_seconds_ceil(remaining).max(1),
                ))));
            }
            state.blocked_until = None;
        }
        if state.next_request_at > now {
            sleep_until(state.next_request_at).await;
        }
        state.next_request_at = Instant::now() + self.policy.min_request_interval;
        Ok(())
    }

    pub(crate) async fn observe_rate_limit(&self, limit: IndexerRateLimit) -> IndexerRateLimit {
        let mut state = self.rate.lock().await;
        let now = Instant::now();
        if let Some(blocked_until) = state.blocked_until
            && blocked_until > now
        {
            return IndexerRateLimit::new(Some(
                duration_seconds_ceil(blocked_until.saturating_duration_since(now)).max(1),
            ));
        }

        let normalized_retry_after = limit
            .retry_after_secs()
            .map(|seconds| seconds.min(MAX_RATE_LIMIT_COOLDOWN.as_secs()));
        let retry_after = normalized_retry_after
            .map(Duration::from_secs)
            .unwrap_or_default();
        let cooldown = self
            .policy
            .default_cooldown
            .max(retry_after)
            .min(MAX_RATE_LIMIT_COOLDOWN);
        state.blocked_until = Some(now + cooldown);
        let effective_retry_after = if cooldown.is_zero() {
            normalized_retry_after.map(|_| 0)
        } else {
            Some(duration_seconds_ceil(cooldown).max(1))
        };
        IndexerRateLimit::new(effective_retry_after)
    }
}

fn is_follow_redirect(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::MOVED_PERMANENTLY
            | StatusCode::FOUND
            | StatusCode::SEE_OTHER
            | StatusCode::TEMPORARY_REDIRECT
            | StatusCode::PERMANENT_REDIRECT
    )
}

fn redirected_request<F>(
    status: StatusCode,
    method: Method,
    mut headers: reqwest::header::HeaderMap,
    replay_request: Option<Request>,
    next_url: Url,
    rebuild_request: &F,
) -> Result<Request, IndexerError>
where
    F: Fn(&Method, &Url) -> Option<RequestBuilder>,
{
    let drops_request_body = status == StatusCode::SEE_OTHER
        || matches!(status, StatusCode::MOVED_PERMANENTLY | StatusCode::FOUND)
            && method == Method::POST;
    if drops_request_body {
        headers.remove(CONTENT_ENCODING);
        headers.remove(CONTENT_LENGTH);
        headers.remove(CONTENT_TYPE);
        headers.remove(TRANSFER_ENCODING);
        let redirected_method = if method == Method::HEAD {
            Method::HEAD
        } else {
            Method::GET
        };
        let mut request = Request::new(redirected_method, next_url);
        *request.headers_mut() = headers;
        return Ok(request);
    }

    let mut request = match replay_request {
        Some(request) => request,
        None => rebuild_request(&method, &next_url)
            .ok_or_else(|| {
                IndexerError::Http("tracker redirect request body cannot be replayed".to_string())
            })?
            .build()
            .map_err(http_error)?,
    };
    *request.url_mut() = next_url;
    Ok(request)
}

fn redirect_identity(url: &Url) -> String {
    let mut identity = url.clone();
    identity.set_fragment(None);
    identity.to_string()
}

fn duration_seconds_ceil(duration: Duration) -> u64 {
    duration
        .as_secs()
        .saturating_add(u64::from(duration.subsec_nanos() > 0))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::Mutex as StdMutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use reqwest::redirect::Policy;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::TcpListener;
    use tokio::task::JoinHandle;

    use super::*;

    async fn spawn_server<F>(handler: F) -> (std::net::SocketAddr, Arc<AtomicUsize>, JoinHandle<()>)
    where
        F: Fn(&str, usize) -> String + Send + Sync + 'static,
    {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let handler = Arc::new(handler);
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let call = server_calls.fetch_add(1, Ordering::SeqCst);
                let mut request = [0_u8; 4096];
                let read = stream.read(&mut request).await.unwrap();
                let request = String::from_utf8_lossy(&request[..read]);
                let response = handler(&request, call);
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        (address, calls, server)
    }

    fn test_client() -> Client {
        Client::builder().redirect(Policy::none()).build().unwrap()
    }

    fn request_path(request: &str) -> &str {
        request.split_whitespace().nth(1).unwrap_or_default()
    }

    #[test]
    fn redirect_to_get_drops_all_payload_headers() {
        let client = test_client();
        let original = client
            .post("https://tracker.example/start")
            .header(CONTENT_ENCODING, "gzip")
            .header(CONTENT_TYPE, "application/json")
            .body("payload")
            .build()
            .unwrap();
        let headers = original.headers().clone();
        let replay = original.try_clone();
        let redirected = redirected_request(
            StatusCode::FOUND,
            Method::POST,
            headers,
            replay,
            Url::parse("https://tracker.example/final").unwrap(),
            &|_, _| None,
        )
        .unwrap();

        assert_eq!(redirected.method(), Method::GET);
        for header in [
            CONTENT_ENCODING,
            CONTENT_LENGTH,
            CONTENT_TYPE,
            TRANSFER_ENCODING,
        ] {
            assert!(!redirected.headers().contains_key(header));
        }
        assert!(redirected.body().is_none());
    }

    #[tokio::test]
    async fn concurrent_sends_start_at_least_the_configured_interval_apart() {
        let (address, calls, server) = spawn_server(|_, _| {
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        })
        .await;
        let interval = Duration::from_millis(35);
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: interval,
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let url = format!("http://{address}/search");

        let started = std::time::Instant::now();
        let (first, second) =
            tokio::join!(gate.send(client.get(&url)), gate.send(client.get(&url)),);
        assert!(first.unwrap().status().is_success());
        assert!(second.unwrap().status().is_success());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        assert!(started.elapsed() >= Duration::from_millis(25));
        server.abort();
    }

    #[tokio::test]
    async fn same_origin_redirect_hops_are_gated_and_keep_headers() {
        let starts = Arc::new(StdMutex::new(Vec::new()));
        let server_starts = Arc::clone(&starts);
        let (address, calls, server) = spawn_server(move |request, _| {
            server_starts.lock().unwrap().push(std::time::Instant::now());
            assert!(request.to_ascii_lowercase().contains("x-test-auth: retained"));
            match request_path(request) {
                "/start" => "HTTP/1.1 302 Found\r\nLocation: /middle\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                "/middle" => "HTTP/1.1 307 Temporary Redirect\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                "/final" => "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok".to_string(),
                path => panic!("unexpected path: {path}"),
            }
        })
        .await;
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::from_millis(30),
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let base_url = Url::parse(&format!("http://{address}")).unwrap();
        let request = client
            .get(base_url.join("/start").unwrap())
            .header("x-test-auth", "retained");

        let response = gate
            .send_with_same_origin_redirects(&client, request, &base_url)
            .await
            .unwrap();
        assert_eq!(response.url().path(), "/final");
        assert_eq!(response.text().await.unwrap(), "ok");
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        let starts = starts.lock().unwrap();
        assert!(starts[1].duration_since(starts[0]) >= Duration::from_millis(20));
        assert!(starts[2].duration_since(starts[1]) >= Duration::from_millis(20));
        server.abort();
    }

    #[tokio::test]
    async fn temporary_redirect_can_rebuild_a_streaming_post_body() {
        let (address, calls, server) = spawn_server(|request, _| {
            assert!(request.starts_with("POST "));
            assert!(
                request
                    .to_ascii_lowercase()
                    .contains("content-type: multipart/form-data")
            );
            match request_path(request) {
                "/token" => "HTTP/1.1 307 Temporary Redirect\r\nLocation: /canonical-token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                "/canonical-token" => "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
                path => panic!("unexpected path: {path}"),
            }
        })
        .await;
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let base_url = Url::parse(&format!("http://{address}")).unwrap();
        let form = reqwest::multipart::Form::new().text("id", "42");
        let request = client
            .post(base_url.join("/token").unwrap())
            .multipart(form);
        assert!(request.try_clone().is_none());

        let response = gate
            .send_with_same_origin_redirects_rebuilding(
                &client,
                request,
                &base_url,
                |method, url| {
                    (method == Method::POST).then(|| {
                        client
                            .post(url.clone())
                            .multipart(reqwest::multipart::Form::new().text("id", "42"))
                    })
                },
            )
            .await
            .unwrap();
        assert!(response.status().is_success());
        assert_eq!(response.url().path(), "/canonical-token");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn cross_origin_redirect_is_rejected_without_contacting_target() {
        let (target_address, target_calls, target_server) = spawn_server(|_, _| {
            "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string()
        })
        .await;
        let (source_address, source_calls, source_server) = spawn_server(move |_, _| {
            format!(
                "HTTP/1.1 302 Found\r\nLocation: http://{target_address}/stolen\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            )
        })
        .await;
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let base_url = Url::parse(&format!("http://{source_address}")).unwrap();
        let request = client.get(base_url.join("/start").unwrap());

        assert!(matches!(
            gate.send_with_same_origin_redirects(&client, request, &base_url)
                .await,
            Err(IndexerError::UnsafeUrl(_))
        ));
        assert_eq!(source_calls.load(Ordering::SeqCst), 1);
        tokio::task::yield_now().await;
        assert_eq!(target_calls.load(Ordering::SeqCst), 0);
        source_server.abort();
        target_server.abort();
    }

    #[tokio::test]
    async fn login_redirect_is_reported_as_authentication_expired_without_following() {
        let (address, calls, server) = spawn_server(|request, _| match request_path(request) {
            "/start" => "HTTP/1.1 302 Found\r\nLocation: /login.php\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            path => panic!("login redirect must not be followed: {path}"),
        })
        .await;
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let base_url = Url::parse(&format!("http://{address}")).unwrap();
        let request = client.get(base_url.join("/start").unwrap());

        assert!(matches!(
            gate.send_with_same_origin_redirects(&client, request, &base_url)
                .await,
            Err(IndexerError::AuthenticationExpired(_))
        ));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }

    #[tokio::test]
    async fn redirect_loop_is_rejected_before_repeating_a_request() {
        let (address, calls, server) = spawn_server(|request, _| match request_path(request) {
            "/a" => "HTTP/1.1 302 Found\r\nLocation: /b\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            "/b" => "HTTP/1.1 302 Found\r\nLocation: /a\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            path => panic!("unexpected path: {path}"),
        })
        .await;
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let base_url = Url::parse(&format!("http://{address}")).unwrap();
        let request = client.get(base_url.join("/a").unwrap());

        let error = gate
            .send_with_same_origin_redirects(&client, request, &base_url)
            .await
            .unwrap_err();
        assert!(matches!(error, IndexerError::Http(message) if message.contains("loop")));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }

    #[tokio::test]
    async fn redirect_limit_stops_before_sending_an_unbounded_chain() {
        let (address, calls, server) = spawn_server(|request, _| {
            let current: usize = request_path(request)
                .trim_start_matches('/')
                .parse()
                .unwrap();
            format!(
                "HTTP/1.1 302 Found\r\nLocation: /{}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
                current + 1
            )
        })
        .await;
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::ZERO,
        });
        let client = test_client();
        let base_url = Url::parse(&format!("http://{address}")).unwrap();
        let request = client.get(base_url.join("/0").unwrap());

        let error = gate
            .send_with_same_origin_redirects(&client, request, &base_url)
            .await
            .unwrap_err();
        assert!(matches!(error, IndexerError::Http(message) if message.contains("redirect limit")));
        assert_eq!(calls.load(Ordering::SeqCst), MAX_SAME_ORIGIN_REDIRECTS + 1);
        server.abort();
    }

    #[tokio::test]
    async fn http_429_blocks_followups_without_another_network_request() {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let server_calls = Arc::clone(&calls);
        let server = tokio::spawn(async move {
            loop {
                let (mut stream, _) = listener.accept().await.unwrap();
                let call = server_calls.fetch_add(1, Ordering::SeqCst);
                let mut request = [0_u8; 1024];
                let _ = stream.read(&mut request).await.unwrap();
                let response = if call == 0 {
                    tokio::time::sleep(Duration::from_millis(25)).await;
                    "HTTP/1.1 429 Too Many Requests\r\nRetry-After: 0\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                } else {
                    "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                };
                stream.write_all(response.as_bytes()).await.unwrap();
            }
        });
        let gate = OriginAccessGate::new(IndexerAccessPolicy {
            min_request_interval: Duration::ZERO,
            default_cooldown: Duration::from_millis(40),
        });
        let client = reqwest::Client::new();
        let url = format!("http://{address}/search");

        let (first, second) =
            tokio::join!(gate.send(client.get(&url)), gate.send(client.get(&url)),);
        assert!(matches!(first, Err(IndexerError::RateLimited(_))));
        assert!(matches!(second, Err(IndexerError::RateLimited(_))));
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        tokio::time::sleep(Duration::from_millis(45)).await;
        assert_eq!(gate.send(client.get(&url)).await.unwrap().status(), 200);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        server.abort();
    }
}
