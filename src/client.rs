//! Top-level API client: wraps a [`reqwest::Client`] with a token manager
//! and a captcha manager, and exposes high-level operations.

use std::collections::HashSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::auth::{OAuthCredentials, TokenHooks, TokenManager};
use crate::captcha::CaptchaManager;
use crate::error::{Error, Result};
use crate::types::{deserialize_lenient_u64, deserialize_optional_lenient_u64, FileInfo, Quota};

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
    hooks: TokenHooks,
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
            hooks: TokenHooks::default(),
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

    /// Override the device id. If unset, it is derived from the refresh token
    /// with [`device_id_from`] — which changes whenever the token rotates, so
    /// pass a saved id to keep one device across runs.
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
        self.hooks.rotation = Some(Arc::new(f));
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
        self.hooks.source = Some(Arc::new(source));
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
            self.hooks,
        );

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

impl Client {
    /// Convenience for [`ClientBuilder::new`].
    pub fn builder() -> ClientBuilder {
        ClientBuilder::new()
    }

    /// Return the device id currently associated with this client.
    pub fn device_id(&self) -> &str {
        &self.device_id
    }

    /// The HTTP client used for file **content** transfers.
    ///
    /// Unlike the API client it has no total deadline, only a per-read stall
    /// timeout, so a large file is never cut off mid-transfer.
    pub(crate) fn download_client(&self) -> &reqwest::Client {
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
            all.extend(body.files.unwrap_or_default());

            // Stop when the server stops paginating. A page it names twice
            // would loop forever, and stopping there quietly would report a
            // partial listing as the whole folder — files silently missing.
            let next = match body.next_page_token {
                Some(token) if !token.is_empty() => token,
                _ => break,
            };
            if !followed.insert(next.clone()) {
                return Err(Error::Protocol(format!(
                    "the listing of folder {parent_id:?} repeated a page token; it may be incomplete"
                )));
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
        // The captcha the server reported expired, once it has; asking with it
        // gets a fresh one.
        let mut expired_captcha: Option<String> = None;
        let mut net_retries: u32 = 0;

        loop {
            let access = self.tokens.access_token().await?;
            let captcha = self
                .captcha
                .token(action, expired_captcha.as_deref())
                .await?;

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
            let retry_after = retry_after(resp.headers());
            let text = match resp.text().await {
                Ok(t) => t,
                Err(e) if is_transient(&e) && net_retries < MAX_NET_RETRIES => {
                    let delay = jittered_backoff(backoff_delay(net_retries)).max(retry_after);
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
                self.tokens.invalidate_if_current(&access).await;
                continue;
            }

            if expired_captcha.is_none()
                && serde_json::from_str::<ApiError>(&text).is_ok_and(|err| err.error_code == 9)
            {
                tracing::debug!(action = %action, "captcha expired, refreshing and retrying");
                expired_captcha = Some(captcha);
                continue;
            }

            // Server-side transient failures (5xx, 429) are worth a backoff retry.
            if is_retryable_status(status) && net_retries < MAX_NET_RETRIES {
                // The server's own wait, when it names one, beats our guess.
                let delay = jittered_backoff(backoff_delay(net_retries)).max(retry_after);
                tracing::debug!(action = %action, status = %status.as_u16(), ?delay, "server error, retrying");
                net_retries += 1;
                tokio::time::sleep(delay).await;
                continue;
            }

            // A rejected refresh token means the same thing whichever endpoint
            // reports it, so give it the actionable message the token exchange
            // uses instead of relaying a raw body from a drive call.
            // What the server said goes into error text, but never the token.
            let text = crate::auth::redact(&text, &access);
            if crate::auth::is_grant_rejection(&text) {
                return Err(crate::auth::explain_auth_failure(status.as_u16(), &text));
            }

            return Err(Error::Api {
                status: status.as_u16(),
                message: text,
            });
        }
    }

    /// Resolve a Unix-style path (e.g. `"/My Pack/video.mp4"`) to its entry,
    /// listing each level once.
    ///
    /// Middle segments must be folders; the last may be a file. At every
    /// segment a folder wins over a same-named file, so a name collision never
    /// resolves according to listing order.
    ///
    /// An empty path or `"/"` refers to the virtual root, which has no entry of
    /// its own, and is therefore rejected.
    pub async fn resolve_path_info(&self, path: &str) -> Result<FileInfo> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let Some((last, folders)) = segments.split_last() else {
            return Err(Error::InvalidPath(
                "the drive root has no entry of its own; name a file or folder",
            ));
        };
        let mut parent_id = String::new();
        for segment in folders {
            parent_id = self.child(&parent_id, segment, path, false).await?.id;
        }
        self.child(&parent_id, last, path, true).await
    }

    /// The entry `name` inside `parent_id` — a folder, or failing that a file
    /// when `file_ok` — while resolving `path`.
    async fn child(
        &self,
        parent_id: &str,
        name: &str,
        path: &str,
        file_ok: bool,
    ) -> Result<FileInfo> {
        let children = self.list_folder(parent_id).await?;
        let named = |f: &&FileInfo| f.name == name;
        children
            .iter()
            .filter(named)
            .find(|f| f.kind.is_folder())
            .or_else(|| children.iter().filter(named).find(|_| file_ok))
            .cloned()
            .ok_or_else(|| Error::NotFound {
                path: path.to_string(),
                segment: name.to_string(),
            })
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
    /// File size in bytes, when the detail response reported one.
    ///
    /// `None` means the field was absent (not that the file is empty), so a
    /// caller holding a size from a directory listing can substitute it.
    pub size: Option<u64>,
}

/// Derive a device id from a refresh token (md5 hex of the token).
///
/// Only as stable as the token: PikPak rotates it on every refresh, so a caller
/// that wants one device across runs should save the result and pass it to
/// [`ClientBuilder::device_id`] from then on.
pub fn device_id_from(refresh_token: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(refresh_token.as_bytes());
    hex::encode(hasher.finalize())
}

/// Maximum retries for transient network / server errors (per request).
const MAX_NET_RETRIES: u32 = 4;

/// Exponential backoff for a retry attempt (0-based): `base`, then doubling,
/// capped at `cap`.
///
/// Shared by the API retries here and the content transfers, which differ only
/// in their base and cap.
pub(crate) fn exponential_backoff(base: Duration, cap: Duration, attempt: u32) -> Duration {
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
pub(crate) fn is_retryable_status(status: reqwest::StatusCode) -> bool {
    status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// Spread a retry delay over `[base/2, base]`.
///
/// Requests that failed together should not retry together, so the delay is
/// jittered downwards by up to half. The entropy is the clock's sub-second
/// component: not a quality random source, but sufficient to decorrelate
/// concurrent retries without taking on a `rand` dependency.
pub(crate) fn jittered_backoff(base: Duration) -> Duration {
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

/// Longest `Retry-After` honoured: past this, waiting inside one call is worse
/// than failing and letting the caller retry later.
const MAX_RETRY_AFTER: Duration = Duration::from_secs(60);

/// The wait a `Retry-After: <seconds>` header asks for, capped; zero when the
/// header is absent or names a date (rare, and not worth a date parser).
fn retry_after(headers: &reqwest::header::HeaderMap) -> Duration {
    headers
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse::<u64>().ok())
        .map_or(Duration::ZERO, |secs| {
            Duration::from_secs(secs).min(MAX_RETRY_AFTER)
        })
}

/// Send the request `build` makes, retrying with the API backoff while the
/// failure is one `retryable` accepts, at most [`MAX_NET_RETRIES`] times.
pub(crate) async fn send_retrying(
    build: impl Fn() -> reqwest::RequestBuilder,
    retryable: fn(&reqwest::Error) -> bool,
) -> reqwest::Result<reqwest::Response> {
    let mut attempt = 0;
    loop {
        match build().send().await {
            Err(e) if retryable(&e) && attempt < MAX_NET_RETRIES => {
                let delay = jittered_backoff(backoff_delay(attempt));
                tracing::debug!(error = %e, ?delay, "transient network error, retrying");
                attempt += 1;
                tokio::time::sleep(delay).await;
            }
            result => return result,
        }
    }
}

/// Whether a reqwest error is a transient network condition worth retrying.
pub(crate) fn is_transient(err: &reqwest::Error) -> bool {
    err.is_timeout() || err.is_connect() || err.is_request() || err.is_body()
}

#[derive(Debug, Deserialize)]
struct ApiError {
    #[serde(default)]
    error_code: i64,
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
    /// Absent or `null` on an empty folder from some servers: no entries.
    #[serde(default)]
    files: Option<Vec<FileInfo>>,
    #[serde(default)]
    next_page_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FileDetailResponse {
    #[serde(default)]
    name: String,
    #[serde(default, deserialize_with = "deserialize_optional_lenient_u64")]
    size: Option<u64>,
    #[serde(default)]
    web_content_link: String,
}

#[cfg(test)]
mod tests;
