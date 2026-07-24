//! Top-level API client: wraps a [`reqwest::Client`] with a token manager
//! and a captcha manager, and exposes high-level operations.

use std::time::Duration;

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::auth::{OAuthCredentials, TokenManager};
use crate::captcha::CaptchaManager;
use crate::error::{Error, Result};
use crate::types::{deserialize_string_to_u64, FileInfo, Quota};

/// PikPak user/auth service base URL. Overridable via
/// [`ClientBuilder::auth_base_url`].
const DEFAULT_AUTH_BASE: &str = "https://user.mypikpak.com";

/// PikPak drive API base URL. Overridable via
/// [`ClientBuilder::api_base_url`].
const DEFAULT_API_BASE: &str = "https://api-drive.mypikpak.com";

/// Default request timeout applied to every HTTP call.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

/// User-Agent string baked into the upstream pikpakcli project (Android build).
const DEFAULT_USER_AGENT: &str = "ANDROID-com.pikcloud.pikpak/1.21.0";

/// Public API surface: every PikPak operation goes through this struct.
///
/// Construct via [`ClientBuilder`]. Cheap to clone.
#[derive(Clone)]
pub struct Client {
    http: reqwest::Client,
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

    /// Set the request timeout (default: 30s).
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

    /// Finalize and build the [`Client`].
    pub fn build(self) -> Result<Client> {
        let refresh_token = self
            .refresh_token
            .ok_or(Error::NotConfigured("refresh_token"))?;

        let device_id = self
            .device_id
            .unwrap_or_else(|| device_id_from(&refresh_token));

        let mut http = reqwest::Client::builder()
            .timeout(self.timeout)
            .user_agent(&self.user_agent);
        if let Some(p) = self.proxy.as_deref() {
            http = http.proxy(reqwest::Proxy::all(p)?);
        }
        let http = http.build()?;

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

        let captcha = CaptchaManager::new(
            http.clone(),
            captcha_endpoint,
            &self.credentials.client_id,
            &device_id,
            tokens.clone(),
        );

        Ok(Client {
            http,
            api_base: self.api_base_url,
            device_id,
            tokens,
            captcha,
        })
    }
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

    /// Return a handle to the underlying HTTP client for direct requests
    /// (e.g. downloading file content).
    pub fn http_client(&self) -> &reqwest::Client {
        &self.http
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

            // Break if the server stops paginating or repeats a token we
            // already used (defends against a malformed infinite response).
            match body.next_page_token {
                Some(t) if !t.is_empty() && Some(&t) != page_token.as_ref() => page_token = Some(t),
                _ => break,
            }
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
                    let delay = backoff_delay(net_retries);
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
                    let delay = backoff_delay(net_retries);
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
                        let _ = self.captcha.refresh(action, Some(&prev)).await?;
                        continue;
                    }
                }
            }

            // Server-side transient failures (5xx, 429) are worth a backoff retry.
            if (status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS)
                && net_retries < MAX_NET_RETRIES
            {
                let delay = backoff_delay(net_retries);
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
        let normalized = path.trim_matches('/');
        if normalized.is_empty() {
            return Ok(String::new());
        }

        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent_id = String::new();

        for seg in &segments {
            let children = self.list_folder(&parent_id).await?;
            let found = children
                .iter()
                .find(|f| f.kind.is_folder() && f.name == *seg);

            match found {
                Some(f) => parent_id = f.id.clone(),
                None => {
                    return Err(Error::NotFound {
                        path: path.to_string(),
                        segment: (*seg).to_string(),
                    });
                }
            }
        }

        Ok(parent_id)
    }

    /// Resolve a path to a [`FileInfo`]. If the path points to a file
    /// (i.e. the last segment is a file, not a folder), returns that file's
    /// info; otherwise returns the folder's info.
    pub async fn resolve_path_info(&self, path: &str) -> Result<FileInfo> {
        let normalized = path.trim_matches('/');
        if normalized.is_empty() {
            return Err(Error::InvalidPath("path must not be empty"));
        }

        let segments: Vec<&str> = normalized.split('/').filter(|s| !s.is_empty()).collect();
        let mut parent_id = String::new();

        for (i, seg) in segments.iter().enumerate() {
            let is_last = i == segments.len() - 1;
            let children = self.list_folder(&parent_id).await?;
            // Middle segments must be folders; only the final segment may be
            // a file. Restricting the search avoids matching a same-named
            // file that happens to precede the intended folder.
            let found = if is_last {
                children.iter().find(|f| f.name == **seg)
            } else {
                children
                    .iter()
                    .find(|f| f.kind.is_folder() && f.name == **seg)
            };

            match found {
                Some(f) => {
                    if is_last {
                        return Ok(f.clone());
                    }
                    parent_id = f.id.clone();
                }
                None => {
                    return Err(Error::NotFound {
                        path: path.to_string(),
                        segment: (*seg).to_string(),
                    });
                }
            }
        }

        unreachable!("resolve_path_info loop must return inside")
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

/// Exponential backoff for retry attempt `n` (0-based): 300ms, 600ms, 1.2s,
/// 2.4s, ... capped at 10s.
fn backoff_delay(attempt: u32) -> Duration {
    let ms = 300u64.saturating_mul(1u64 << attempt.min(5));
    Duration::from_millis(ms.min(10_000))
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
    #[serde(deserialize_with = "deserialize_string_to_u64")]
    limit: u64,
    #[serde(deserialize_with = "deserialize_string_to_u64")]
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
    #[serde(default, deserialize_with = "deserialize_string_to_u64")]
    size: u64,
    #[serde(default)]
    web_content_link: String,
}
