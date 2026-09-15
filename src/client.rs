//! Top-level API client: wraps a [`reqwest::Client`] with a token manager
//! and a captcha manager, and exposes high-level operations.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::auth::{OAuthCredentials, TokenManager};
use crate::captcha::CaptchaManager;
use crate::error::{Error, Result};
use crate::types::{deserialize_lenient_u64, FileInfo, Quota};

/// PikPak user/auth service base URL. Overridable via
/// [`ClientBuilder::auth_base_url`].
const DEFAULT_AUTH_BASE: &str = "https://user.mypikpak.com";

/// PikPak drive API base URL. Overridable via
/// [`ClientBuilder::api_base_url`].
const DEFAULT_API_BASE: &str = "https://api-drive.mypikpak.com";

/// Default HTTP timeout.
///
/// API calls use this as a *total* deadline for the whole request (they carry
/// small JSON bodies). Content transfers use it only as a per-read *stall*
/// timeout, so a download is aborted when the connection stops producing bytes
/// — never merely because the file is large.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// User-Agent string baked into the upstream pikpakcli project (Android build).
const DEFAULT_USER_AGENT: &str = "ANDROID-com.pikcloud.pikpak/1.21.0";

/// Shared callback invoked with each newly rotated refresh token.
type RefreshTokenHook = Arc<dyn Fn(&str) + Send + Sync>;

/// Shared callback returning the refresh token the caller has persisted.
type RefreshTokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

/// Public API surface: every PikPak operation goes through this struct.
///
/// Construct via [`ClientBuilder`]. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
    download_http: reqwest::Client,
    api_base: String,
    device_id: String,
    tokens: TokenManager,
    captcha: CaptchaManager,
}

/// Builder for [`Client`].
pub struct ClientBuilder {
    refresh_token: Option<String>,
    credentials: OAuthCredentials,
    device_id: Option<String>,
    auth_base_url: String,
    api_base_url: String,
    timeout: Duration,
    user_agent: String,
    proxy: Option<String>,
    on_refresh_token: Option<RefreshTokenHook>,
    refresh_token_source: Option<RefreshTokenSource>,
}

impl Default for ClientBuilder {
    fn default() -> Self {
        Self {
            refresh_token: None,
            credentials: OAuthCredentials::default(),
            device_id: None,
            auth_base_url: DEFAULT_AUTH_BASE.into(),
            api_base_url: DEFAULT_API_BASE.into(),
            timeout: DEFAULT_TIMEOUT,
            user_agent: DEFAULT_USER_AGENT.into(),
            proxy: None,
            on_refresh_token: None,
            refresh_token_source: None,
        }
    }
}

impl ClientBuilder {
    /// Start a new builder.
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the refresh token (required).
    pub fn refresh_token(mut self, token: impl Into<String>) -> Self {
        self.refresh_token = Some(token.into());
        self
    }

    /// Override OAuth client id / secret. Defaults match upstream pikpakcli.
    pub fn credentials(mut self, c: OAuthCredentials) -> Self {
        self.credentials = c;
        self
    }

    /// Override the device id. If unset, we derive a stable id via
    /// `md5(refresh_token)` so users don't need to pick one.
    pub fn device_id(mut self, id: impl Into<String>) -> Self {
        self.device_id = Some(id.into());
        self
    }

    /// Override the OAuth service base URL.
    pub fn auth_base_url(mut self, url: impl Into<String>) -> Self {
        self.auth_base_url = url.into();
        self
    }

    /// Override the Drive API base URL.
    pub fn api_base_url(mut self, url: impl Into<String>) -> Self {
        self.api_base_url = url.into();
        self
    }

    /// Set the HTTP timeout (default: 30s).
    ///
    /// It is applied as a total deadline to API calls and as a per-read stall
    /// timeout to content downloads.
    pub fn timeout(mut self, t: Duration) -> Self {
        self.timeout = t;
        self
    }

    /// Set a custom User-Agent header (default: the Android pikpakcli UA).
    pub fn user_agent(mut self, ua: impl Into<String>) -> Self {
        self.user_agent = ua.into();
        self
    }

    /// Route all requests through an HTTP(S) proxy.
    pub fn proxy(mut self, url: impl Into<String>) -> Self {
        self.proxy = Some(url.into());
        self
    }

    /// Register a callback invoked with every newly rotated refresh token.
    ///
    /// PikPak invalidates the previous refresh token on each exchange, so a
    /// long-running command that is interrupted before it can persist the new
    /// value may lose access to the account. This hook fires the moment the
    /// server issues the token, which is the earliest point it is safe to store.
    pub fn on_refresh_token<F>(mut self, f: F) -> Self
    where
        F: Fn(&str) + Send + Sync + 'static,
    {
        self.on_refresh_token = Some(Arc::new(f));
        self
    }

    /// Register a callback returning the refresh token you have persisted, if
    /// any (for the CLI: the `PIKPAK_REFRESH_TOKEN` line of its `.env`).
    ///
    /// PikPak invalidates the previous refresh token on every exchange, so a
    /// token read a moment ago can already be superseded — typically when a
    /// second copy of the CLI, or the web client, refreshed in the meantime.
    /// When an exchange is rejected, the token this callback returns is tried
    /// once if it differs from the rejected one, which lets concurrent runs
    /// recover instead of failing while the working replacement sits on disk.
    pub fn refresh_token_source<F>(mut self, source: F) -> Self
    where
        F: Fn() -> Option<String> + Send + Sync + 'static,
    {
        self.refresh_token_source = Some(Arc::new(source));
        self
    }

    /// Finalize and build the [`Client`].
    pub fn build(self) -> Result<Client> {
        let refresh_token = self
            .refresh_token
            .ok_or(Error::NotConfigured("refresh_token"))?;

        let device_id = self
            .device_id
            .unwrap_or_else(|| device_id_from(&refresh_token));

        // Two clients, because the two workloads need opposite timeout
        // semantics:
        //
        // * API calls get a *total* deadline — they carry small JSON bodies and
        //   a hung request should not pin a worker forever.
        // * Content transfers must have no total deadline at all: reqwest's
        //   `timeout` would abort the body mid-stream once the download runs
        //   longer than the deadline (e.g. any file that takes >30s), which
        //   after the bounded retries fails the file permanently. A per-read
        //   stall timeout gives the protection we actually want — abort when
        //   the connection stops producing bytes, keep going while it does.
        let mut api = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(&self.user_agent);
        let mut content = reqwest::Client::builder()
            .connect_timeout(self.timeout)
            .read_timeout(self.timeout)
            .user_agent(&self.user_agent);
        if let Some(p) = self.proxy.as_deref() {
            let proxy = reqwest::Proxy::all(p)?;
            api = api.proxy(proxy.clone());
            content = content.proxy(proxy);
        }
        let http = api.build()?;
        let download_http = content.build()?;

        let auth_base = self.auth_base_url.trim_end_matches('/');
        let auth_endpoint = format!("{auth_base}/v1/auth/token");
        let captcha_endpoint = format!("{auth_base}/v1/shield/captcha/init");

        let tokens = TokenManager::new(
            http.clone(),
            auth_endpoint,
            self.credentials.clone(),
            &device_id,
            refresh_token,
        );
        if let Some(hook) = self.on_refresh_token {
            tokens.set_rotation_hook(move |token| hook(token));
        }
        if let Some(source) = self.refresh_token_source {
            tokens.set_token_source(move || source());
        }

        let captcha = CaptchaManager::new(
            http.clone(),
            captcha_endpoint,
            &self.credentials.client_id,
            &device_id,
            tokens.clone(),
        );

        Ok(Client {
            http,
            download_http,
            api_base: self.api_base_url,
            device_id,
            tokens,
            captcha,
        })
    }
}

/// Whether `path` refers to the drive root: empty, `/`, or any number of
/// slashes.
pub fn is_drive_root(path: &str) -> bool {
    path.trim_matches('/').is_empty()
}

/// Where a resolved path ended up.
enum Resolved {
    /// The virtual root, which has no [`FileInfo`] of its own.
    Root,
    /// The entry named by the final path segment.
    Entry(FileInfo),
}

impl Client {
    /// Convenience for [`ClientBuilder::new`].
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Return the device id currently associated with this client.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// Return the [`TokenManager`] so callers can read the rotated
    /// refresh token and persist it.
    pub fn tokens(&self) -> &TokenManager {
        &self.tokens
    }

    /// Return the HTTP client used for API calls.
    ///
    /// It carries the configured total request deadline. For file content use
    /// [`download_client`](Self::download_client) instead.
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
    }

    /// Return the HTTP client used for file **content** transfers.
    ///
    /// Unlike [`http_client`](Self::http_client) it has no total deadline, only
    /// a per-read stall timeout, so a large file is never cut off mid-transfer.
    pub fn download_client(&self) -> &reqwest::Client {
        &self.download_http
    }

    /// Return the user's storage quota.
    pub async fn quota(&self) -> Result<Quota> {
        let action = "GET:/drive/v1/about";
        let url = format!("{}/drive/v1/about", self.api_base.trim_end_matches('/'));

        let resp = self.drive_get(&url, action, &[]).await?;
        let body: AboutResponse = serde_json::from_str(&resp)?;
        Ok(Quota {
            total: body.quota.limit,
            used: body.quota.usage,
        })
    }

    /// List the direct children of a folder. Pass `""` for the root.
    pub async fn list_folder(&self, parent_id: &str) -> Result<Vec<FileInfo>> {
        let action = "GET:/drive/v1/files";
        let url = format!("{}/drive/v1/files", self.api_base.trim_end_matches('/'));

        let mut all = Vec::new();
        let mut page_token: Option<String> = None;
        // Every token already followed. Guarding against the *previous* token
        // alone would not catch a server cycling through two tokens
        // (A -> B -> A -> ...), which would loop forever.
        let mut followed: HashSet<String> = HashSet::new();

        loop {
            let mut params: Vec<(&str, String)> = vec![
                ("parent_id", parent_id.to_string()),
                ("limit", "500".to_string()),
                ("thumbnail_size", "SIZE_MEDIUM".to_string()),
                ("with_audit", "false".to_string()),
                ("filters", r#"{"trashed":{"eq":false}}"#.to_string()),
            ];
            if let Some(t) = page_token.as_deref() {
                params.push(("page_token", t.to_string()));
            }

            let resp = self.drive_get(&url, action, &params).await?;
            let body: ListResponse = serde_json::from_str(&resp)?;
            all.extend(body.files);

            // Stop when the server stops paginating, or names a page that has
            // already been fetched.
            let next = match body.next_page_token {
                Some(token) if !token.is_empty() => token,
                _ => break,
            };
            if !followed.insert(next.clone()) {
                break;
            }
            page_token = Some(next);
        }

        Ok(all)
    }

    /// Execute a GET against the drive API with bearer auth + captcha token.
    ///
    /// If the server returns error_code 9 (captcha expired), refreshes the
    /// captcha and retries exactly once.
    async fn drive_get(&self, url: &str, action: &str, query: &[(&str, String)]) -> Result<String> {
        // 401 and captcha-expiry each get one targeted retry; transient
        // network failures and 5xx/429 get bounded exponential-backoff retries.
        let mut auth_retried = false;
        let mut captcha_retried = false;
        let mut net_retries: u32 = 0;

        loop {
            let access = self.tokens.access_token().await?;
            let captcha = self.captcha.token_for(action).await?;

            let send = self
                .http
                .get(url)
                .bearer_auth(&access)
                .header("X-Device-Id", &self.device_id)
                .header("X-Captcha-Token", &captcha)
                .query(query)
                .send()
                .await;

            let resp = match send {
                Ok(r) => r,
                Err(e) if is_transient(&e) && net_retries < MAX_NET_RETRIES => {
                    let delay = jittered_backoff(backoff_delay(net_retries));
                    tracing::debug!(action = %action, error = %e, ?delay, "transient network error, retrying");
                    net_retries += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            let status = resp.status();
            let text = match resp.text().await {
                Ok(t) => t,
                Err(e) if is_transient(&e) && net_retries < MAX_NET_RETRIES => {
                    let delay = jittered_backoff(backoff_delay(net_retries));
                    net_retries += 1;
                    tokio::time::sleep(delay).await;
                    continue;
                }
                Err(e) => return Err(e.into()),
            };

            if status.is_success() {
                return Ok(text);
            }

            if status == reqwest::StatusCode::UNAUTHORIZED && !auth_retried {
                tracing::debug!(
                    action = %action,
                    "access token rejected, invalidating and retrying"
                );
                auth_retried = true;
                self.tokens.invalidate().await;
                continue;
            }

            if !captcha_retried {
                if let Ok(err) = serde_json::from_str::<ApiError>(&text) {
                    if err.error_code == 9 {
                        tracing::debug!(
                            action = %action,
                            "captcha expired, refreshing and retrying"
                        );
                        captcha_retried = true;
                        let prev = captcha.clone();
                        // The refreshed token is re-read from the cache on the
                        // next iteration; only the side effect matters here.
                        self.captcha.refresh(action, Some(&prev)).await?;
                        continue;
                    }
                }
            }

            // Server-side transient failures (5xx, 429) are worth a backoff retry.
            if is_retryable_status(status) && net_retries < MAX_NET_RETRIES {
                let delay = jittered_backoff(backoff_delay(net_retries));
                tracing::debug!(action = %action, status = %status.as_u16(), ?delay, "server error, retrying");
                net_retries += 1;
                tokio::time::sleep(delay).await;
                continue;
            }

            return Err(Error::Api {
                status: status.as_u16(),
                message: text,
            });
        }
    }

    /// Resolve a Unix-style path (e.g. `"/My Pack/videos"`) to the
    /// corresponding folder id by walking each path segment.
    ///
    /// Returns `Ok(id)` on success. An empty path or `"/"` resolves to the
    /// root (empty string, matching PikPak convention).
    pub async fn resolve_path(&self, path: &str) -> Result<String> {
        match self.walk_path(path, false).await? {
            Resolved::Root => Ok(String::new()),
            Resolved::Entry(info) => Ok(info.id),
        }
    }

    /// Resolve a path to a [`FileInfo`]. If the path points to a file
    /// (i.e. the last segment is a file, not a folder), returns that file's
    /// info; otherwise returns the folder's info.
    ///
    /// An empty path or `"/"` refers to the virtual root, which has no entry of
    /// its own, and is therefore rejected.
    pub async fn resolve_path_info(&self, path: &str) -> Result<FileInfo> {
        match self.walk_path(path, true).await? {
            Resolved::Root => Err(Error::InvalidPath(
                "the drive root has no entry of its own; name a file or folder",
            )),
            Resolved::Entry(info) => Ok(info),
        }
    }

    /// Walk `path` one segment at a time, listing each level exactly once.
    ///
    /// `allow_file_last` decides whether the final segment may be a file (as
    /// [`resolve_path_info`](Self::resolve_path_info) needs) or must be a folder
    /// (as [`resolve_path`](Self::resolve_path) needs). Middle segments are
    /// always folders: restricting the search avoids matching a same-named file
    /// that happens to precede the intended folder.
    async fn walk_path(&self, path: &str, allow_file_last: bool) -> Result<Resolved> {
        if is_drive_root(path) {
            return Ok(Resolved::Root);
        }
        let normalized = path.trim_matches('/');

        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent_id = String::new();

        for (index, segment) in segments.iter().enumerate() {
            let is_last = index == segments.len() - 1;
            let may_be_file = is_last && allow_file_last;
            let children = self.list_folder(&parent_id).await?;
            let found = children
                .into_iter()
                .find(|f| f.name == *segment && (may_be_file || f.kind.is_folder()));

            match found {
                Some(info) if is_last => return Ok(Resolved::Entry(info)),
                Some(info) => parent_id = info.id,
                None => {
                    return Err(Error::NotFound {
                        path: path.to_string(),
                        segment: (*segment).to_string(),
                    });
                }
            }
        }

        // Unreachable: a non-empty segment list returns from inside the loop.
        Ok(Resolved::Root)
    }

    /// Get a download URL for a file by its id.
    pub async fn get_download_url(&self, file_id: &str) -> Result<DownloadInfo> {
        let action = "GET:/drive/v1/files/:id";
        let url = format!(
            "{}/drive/v1/files/{}",
            self.api_base.trim_end_matches('/'),
            file_id,
        );

        let resp = self.drive_get(&url, action, &[]).await?;
        let body: FileDetailResponse = serde_json::from_str(&resp)?;

        Ok(DownloadInfo {
            web_content_link: body.web_content_link,
            name: body.name,
            size: body.size,
        })
    }
}

/// Download info returned by the API for a single file.
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    /// Direct download URL (time-limited).
    pub web_content_link: String,
    /// File name.
    pub name: String,
    /// File size in bytes.
    pub size: u64,
}

/// Derive a stable device id from the refresh token (md5 hex of the token).
fn device_id_from(refresh_token: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(refresh_token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Maximum retries for transient network / server errors (per request).
const MAX_NET_RETRIES: u32 = 4;

/// Exponential backoff for a retry attempt (0-based): `base`, then doubling,
/// capped at `cap`.
///
/// Shared by the API retries here and the CLI's download retries, which differ
/// only in their base and cap.
pub fn exponential_backoff(base: Duration, cap: Duration, attempt: u32) -> Duration {
    let ms = (base.as_millis() as u64).saturating_mul(1u64 << attempt.min(5));
    Duration::from_millis(ms.min(cap.as_millis() as u64))
}

/// Backoff for an API retry attempt (0-based): 300ms, 600ms, 1.2s, 2.4s, ...
/// capped at 10s.
fn backoff_delay(attempt: u32) -> Duration {
    exponential_backoff(Duration::from_millis(300), Duration::from_secs(10), attempt)
}

/// Whether a status is a transient server-side failure worth retrying.
///
/// 5xx responses and 429 (Too Many Requests) both mean "try again later";
/// repeating any other client error unchanged cannot succeed.
pub fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// Spread a retry delay over `[base/2, base]`.
///
/// Requests that failed together should not retry together, so the delay is
/// jittered downwards by up to half. The entropy is the clock's sub-second
/// component: not a quality random source, but sufficient to decorrelate
/// concurrent retries without taking on a `rand` dependency.
///
/// This is public because both the API retries here and the CLI's download
/// retries use it; the underlying backoff remains deterministic and separately
/// tested.
pub fn jittered_backoff(base: Duration) -> Duration {
    let base_ms = base.as_millis() as u64;
    let span = base_ms / 2;
    if span == 0 {
        return base;
    }
    let entropy = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| u64::from(elapsed.subsec_nanos()))
        .unwrap_or(0);
    Duration::from_millis(base_ms - span + entropy % (span + 1))
}

/// Whether a reqwest error is a transient network condition worth retrying.
fn is_transient(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request() || err.is_body()
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    error_code: i64,
    #[serde(default)]
    #[allow(dead_code)]
    error: String,
}

#[derive(Debug, Deserialize)]
struct AboutResponse {
    quota: AboutQuota,
}

#[derive(Debug, Deserialize)]
struct AboutQuota {
    #[serde(deserialize_with = "deserialize_lenient_u64")]
    limit: u64,
    #[serde(deserialize_with = "deserialize_lenient_u64")]
    usage: u64,
}

#[derive(Debug, Deserialize)]
struct ListResponse {
    files: Vec<FileInfo>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileDetailResponse {
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "deserialize_lenient_u64")]
    size: u64,
    #[serde(default)]
    web_content_link: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::VecDeque;
    use std::net::SocketAddr;
    use std::sync::{Arc, Mutex};

    /// Build a listing body; `next` adds a pagination token.
    fn page_json(entries: &[(&str, &str, &str)], next: Option<&str>) -> String {
        let items: Vec<String> = entries
            .iter()
            .map(|(id, name, kind)| format!(r#"{{"id":"{id}","name":"{name}","kind":"{kind}"}}"#))
            .collect();
        match next {
            Some(token) => format!(
                r#"{{"files":[{}],"next_page_token":"{token}"}}"#,
                items.join(",")
            ),
            None => format!(r#"{{"files":[{}]}}"#, items.join(",")),
        }
    }

    #[derive(Default)]
    struct MockState {
        /// Scripted drive responses, consumed in order; the last one repeats.
        drive: VecDeque<(u16, String)>,
        /// Every request target (path + query), in order.
        targets: Vec<String>,
        auth_calls: usize,
        captcha_calls: usize,
    }

    /// Scripted stand-in for the PikPak stack: auth, captcha init and drive.
    struct Mock {
        addr: SocketAddr,
        state: Arc<Mutex<MockState>>,
    }

    impl Mock {
        async fn start(drive: Vec<(u16, String)>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let addr = listener.local_addr().unwrap();
            let state = Arc::new(Mutex::new(MockState {
                drive: drive.into_iter().collect(),
                ..MockState::default()
            }));
            tokio::spawn({
                let state = state.clone();
                async move {
                    loop {
                        let Ok((mut sock, _)) = listener.accept().await else {
                            return;
                        };
                        let state = state.clone();
                        tokio::spawn(async move {
                            let request = crate::test_http::read_request(&mut sock).await;

                            // The std mutex is released before any await.
                            let (status, payload) = {
                                let mut st = state.lock().unwrap();
                                st.targets.push(request.target.clone());
                                if request.target.starts_with("/v1/auth/token") {
                                    st.auth_calls += 1;
                                    (200, crate::test_http::TOKEN_JSON.to_string())
                                } else if request.target.starts_with("/v1/shield/captcha/init") {
                                    st.captcha_calls += 1;
                                    let token = format!("captcha-{}", st.captcha_calls);
                                    (200, format!(r#"{{"captcha_token":"{token}"}}"#))
                                } else {
                                    match st.drive.len() {
                                        0 => (200, "{}".to_string()),
                                        1 => st.drive[0].clone(),
                                        _ => st.drive.pop_front().unwrap(),
                                    }
                                }
                            };

                            let reason = if status == 200 { "OK" } else { "Error" };
                            crate::test_http::respond_json(&mut sock, status, reason, &payload)
                                .await;
                        });
                    }
                }
            });
            Self { addr, state }
        }

        fn client(&self) -> Client {
            Client::builder()
                .refresh_token("initial-token")
                .auth_base_url(format!("http://{}", self.addr))
                .api_base_url(format!("http://{}", self.addr))
                .build()
                .unwrap()
        }

        fn targets(&self) -> Vec<String> {
            self.state.lock().unwrap().targets.clone()
        }

        fn drive_targets(&self) -> Vec<String> {
            self.targets()
                .into_iter()
                .filter(|t| t.starts_with("/drive/"))
                .collect()
        }

        fn auth_calls(&self) -> usize {
            self.state.lock().unwrap().auth_calls
        }

        fn captcha_calls(&self) -> usize {
            self.state.lock().unwrap().captcha_calls
        }
    }

    #[tokio::test]
    async fn a_rejected_access_token_is_refreshed_and_retried_once() {
        let mock = Mock::start(vec![
            (401, r#"{"error":"token expired"}"#.to_string()),
            (200, page_json(&[], None)),
        ])
        .await;

        let files = mock.client().list_folder("").await.unwrap();

        assert!(files.is_empty());
        assert_eq!(
            mock.auth_calls(),
            2,
            "the 401 must invalidate the cached token so the retry authenticates again"
        );
        assert_eq!(
            mock.drive_targets().len(),
            2,
            "the request must be retried once"
        );
    }

    #[tokio::test]
    async fn a_second_consecutive_401_surfaces_as_an_error() {
        // The auth path gets exactly one retry. If the fresh token is also
        // rejected the request must fail instead of looping. The trailing 500
        // keeps a regression that retried forever from hanging the suite: it
        // would fail out through the 5xx budget with a different status.
        let mock = Mock::start(vec![
            (401, r#"{"error":"token expired"}"#.to_string()),
            (401, r#"{"error":"still expired"}"#.to_string()),
            (500, "unavailable".to_string()),
        ])
        .await;

        match mock.client().list_folder("").await {
            Err(Error::Api { status, .. }) => assert_eq!(status, 401),
            other => panic!("expected an Api error, got {other:?}"),
        }
        assert_eq!(
            mock.drive_targets().len(),
            2,
            "exactly one auth retry, then an error"
        );
        assert_eq!(
            mock.auth_calls(),
            2,
            "the second 401 must not trigger a third authentication"
        );
    }

    #[tokio::test]
    async fn captcha_error_code_9_refreshes_the_captcha_and_retries() {
        let mock = Mock::start(vec![
            (
                400,
                r#"{"error_code":9,"error":"captcha token expired"}"#.to_string(),
            ),
            (200, page_json(&[], None)),
        ])
        .await;

        mock.client().list_folder("").await.unwrap();

        assert_eq!(
            mock.captcha_calls(),
            2,
            "error_code 9 must force a new captcha token"
        );
        assert_eq!(
            mock.auth_calls(),
            1,
            "the access token was still valid and must not be refreshed"
        );
        assert_eq!(mock.drive_targets().len(), 2);
    }

    #[tokio::test]
    async fn a_server_error_is_retried_with_backoff() {
        let mock = Mock::start(vec![
            (503, "temporarily unavailable".to_string()),
            (200, page_json(&[], None)),
        ])
        .await;

        mock.client().list_folder("").await.unwrap();

        assert_eq!(mock.drive_targets().len(), 2, "5xx must be retried");
    }

    #[tokio::test]
    async fn a_non_retryable_status_surfaces_as_an_api_error() {
        let mock = Mock::start(vec![(404, r#"{"error":"not found"}"#.to_string())]).await;

        match mock.client().get_download_url("missing").await {
            Err(Error::Api { status, .. }) => assert_eq!(status, 404),
            other => panic!("expected an Api error, got {other:?}"),
        }
        assert_eq!(mock.drive_targets().len(), 1, "404 must not be retried");
    }

    #[tokio::test]
    async fn listing_follows_next_page_tokens() {
        let mock = Mock::start(vec![
            (200, page_json(&[("1", "a.bin", "drive#file")], Some("t1"))),
            (200, page_json(&[("2", "b.bin", "drive#file")], None)),
        ])
        .await;

        let files = mock.client().list_folder("").await.unwrap();

        assert_eq!(
            files.iter().map(|f| f.name.as_str()).collect::<Vec<_>>(),
            vec!["a.bin", "b.bin"]
        );
        let drive = mock.drive_targets();
        assert_eq!(drive.len(), 2);
        assert!(
            drive[1].contains("page_token=t1"),
            "the second page must carry the token: {}",
            drive[1]
        );
    }

    #[tokio::test]
    async fn listing_terminates_when_the_server_repeats_a_page_token() {
        // A server that keeps returning the same token must not spin forever.
        // The final scripted 500 keeps a regression from hanging the suite: the
        // loop would then fail out through the normal retry budget.
        let repeated = page_json(&[], Some("same"));
        let mock = Mock::start(vec![
            (200, repeated.clone()),
            (200, repeated.clone()),
            (200, repeated),
            (500, "still paginating".to_string()),
        ])
        .await;

        mock.client().list_folder("").await.unwrap();

        assert_eq!(
            mock.drive_targets().len(),
            2,
            "a repeated page token must end the loop after the second request"
        );
    }

    #[tokio::test]
    async fn listing_terminates_on_a_two_token_cycle() {
        // A -> B -> A: comparing only with the previous token would ping-pong
        // forever. The trailing 500 keeps a regression failing rather than
        // hanging the suite.
        let mock = Mock::start(vec![
            (200, page_json(&[], Some("A"))),
            (200, page_json(&[], Some("B"))),
            (200, page_json(&[], Some("A"))),
            (500, "still paginating".to_string()),
        ])
        .await;

        mock.client().list_folder("").await.unwrap();

        let drive = mock.drive_targets();
        assert_eq!(
            drive.len(),
            3,
            "the cycle must end once A has been followed already: {drive:?}"
        );
        assert!(drive[1].contains("page_token=A"), "{}", drive[1]);
        assert!(drive[2].contains("page_token=B"), "{}", drive[2]);
    }

    #[tokio::test]
    async fn resolve_path_walks_segments_and_names_the_missing_one() {
        let mock = Mock::start(vec![
            (200, page_json(&[("A", "A", "drive#folder")], None)),
            (200, page_json(&[("B", "B", "drive#folder")], None)),
            (200, page_json(&[("A", "A", "drive#folder")], None)),
            (200, page_json(&[], None)),
        ])
        .await;
        let client = mock.client();

        assert_eq!(client.resolve_path("/A/B").await.unwrap(), "B");

        match client.resolve_path("/A/Nope").await {
            Err(Error::NotFound { segment, .. }) => assert_eq!(segment, "Nope"),
            other => panic!("expected NotFound, got {other:?}"),
        }

        let drive = mock.drive_targets();
        assert!(
            drive[1].contains("parent_id=A"),
            "the second hop must list inside A: {}",
            drive[1]
        );
        assert!(
            drive[3].contains("parent_id=A"),
            "the failing lookup must be inside A: {}",
            drive[3]
        );
    }

    #[tokio::test]
    async fn resolve_path_info_prefers_a_folder_for_middle_segments() {
        // The parent holds both a file and a folder named "X": a middle segment
        // must resolve to the folder, not the same-named file.
        let mock = Mock::start(vec![
            (
                200,
                page_json(
                    &[
                        ("file-X", "X", "drive#file"),
                        ("folder-X", "X", "drive#folder"),
                    ],
                    None,
                ),
            ),
            (200, page_json(&[("f1", "movie.mp4", "drive#file")], None)),
        ])
        .await;

        let info = mock
            .client()
            .resolve_path_info("/X/movie.mp4")
            .await
            .unwrap();

        assert_eq!(info.id, "f1");
        assert_eq!(info.name, "movie.mp4");
        assert!(
            mock.drive_targets()[1].contains("parent_id=folder-X"),
            "middle segment must resolve to the folder"
        );
    }

    #[tokio::test]
    async fn resolve_path_info_rejects_an_empty_path() {
        let mock = Mock::start(vec![]).await;

        match mock.client().resolve_path_info("/").await {
            Err(Error::InvalidPath(_)) => {}
            other => panic!("expected InvalidPath, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn quota_reads_the_about_endpoint() {
        let mock = Mock::start(vec![(
            200,
            r#"{"quota":{"limit":"1000","usage":"250"}}"#.to_string(),
        )])
        .await;

        let quota = mock.client().quota().await.unwrap();

        assert_eq!(quota.total, 1000);
        assert_eq!(quota.used, 250);
        assert_eq!(quota.free(), 750);
    }

    #[test]
    fn retryable_statuses_are_server_errors_and_429() {
        assert!(is_retryable_status(reqwest::StatusCode::TOO_MANY_REQUESTS));
        assert!(is_retryable_status(
            reqwest::StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(is_retryable_status(reqwest::StatusCode::BAD_GATEWAY));
        assert!(!is_retryable_status(reqwest::StatusCode::NOT_FOUND));
        assert!(!is_retryable_status(reqwest::StatusCode::UNAUTHORIZED));
        assert!(!is_retryable_status(reqwest::StatusCode::BAD_REQUEST));
    }

    #[test]
    fn drive_root_paths() {
        assert!(is_drive_root(""));
        assert!(is_drive_root("/"));
        assert!(is_drive_root("///"));
        assert!(!is_drive_root("/a"));
        assert!(!is_drive_root("a"));
    }

    #[test]
    fn exponential_backoff_grows_from_its_base_and_caps() {
        let base = Duration::from_millis(500);
        let cap = Duration::from_secs(15);
        assert_eq!(exponential_backoff(base, cap, 0), base);
        assert_eq!(exponential_backoff(base, cap, 1), Duration::from_secs(1));
        assert_eq!(
            exponential_backoff(base, cap, 30),
            cap,
            "the cap applies far past the doubling range"
        );
    }

    #[test]
    fn jittered_backoff_stays_in_bounds_and_varies() {
        for ms in [1u64, 2, 10, 500, 1000, 15_000] {
            let base = Duration::from_millis(ms);
            for _ in 0..50 {
                let jittered = jittered_backoff(base);
                assert!(
                    jittered >= base / 2 && jittered <= base,
                    "{jittered:?} outside [{:?}, {base:?}]",
                    base / 2
                );
            }
        }
        // A zero base has no span to spread; it must stay zero.
        assert_eq!(jittered_backoff(Duration::ZERO), Duration::ZERO);

        // It must actually spread: a constant delay would leave workers that
        // failed together retrying together, which is the whole point.
        let base = Duration::from_millis(1000);
        let distinct: HashSet<u128> = (0..100)
            .map(|_| jittered_backoff(base).as_nanos())
            .collect();
        assert!(
            distinct.len() > 1,
            "jitter produced no variation: {distinct:?}"
        );
    }
}
