//! Top-level API client: wraps a [`reqwest::Client`] with a token manager
//! and a captcha manager, and exposes high-level operations.

use std::time::Duration;

use md5::{Digest, Md5};
use serde::Deserialize;

use crate::auth::{OAuthCredentials, TokenManager};
use crate::captcha::CaptchaManager;
use crate::error::{Error, Result};
use crate::types::{FileInfo, Quota, deserialize_string_to_u64};

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

    /// Return the user's storage quota.
    pub async fn quota(&self) -> Result<Quota> {
        let action = "GET:/drive/v1/about";
        let url = format!(
            "{}/drive/v1/about",
            self.api_base.trim_end_matches('/')
        );

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
        let url = format!(
            "{}/drive/v1/files",
            self.api_base.trim_end_matches('/')
        );

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

            match body.next_page_token {
                Some(t) if !t.is_empty() => page_token = Some(t),
                _ => break,
            }
        }

        Ok(all)
    }

    /// Execute a GET against the drive API with bearer auth + captcha token.
    ///
    /// If the server returns error_code 9 (captcha expired), refreshes the
    /// captcha and retries exactly once.
    async fn drive_get(
        &self,
        url: &str,
        action: &str,
        query: &[(&str, String)],
    ) -> Result<String> {
        for attempt in 0..2 {
            let access = self.tokens.access_token().await?;
            let captcha = self.captcha.token_for(action).await?;

            let resp = self
                .http
                .get(url)
                .bearer_auth(&access)
                .header("X-Device-Id", &self.device_id)
                .header("X-Captcha-Token", &captcha)
                .query(query)
                .send()
                .await?;

            let status = resp.status();
            let text = resp.text().await?;

            if status.is_success() {
                return Ok(text);
            }

            // On the first attempt, try to rescue a captcha expiry (code 9).
            if attempt == 0 {
                if let Ok(err) = serde_json::from_str::<ApiError>(&text) {
                    if err.error_code == 9 {
                        tracing::debug!(
                            action = %action,
                            "captcha expired, refreshing and retrying"
                        );
                        let prev = captcha.clone();
                        let _ = self.captcha.refresh(action, Some(&prev)).await?;
                        continue;
                    }
                }
            }

            return Err(Error::Api {
                status: status.as_u16(),
                message: text,
            });
        }

        unreachable!("drive_get loop exits via return")
    }
}

/// Derive a stable device id from the refresh token (md5 hex of the token).
fn device_id_from(refresh_token: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(refresh_token.as_bytes());
    hex::encode(hasher.finalize())
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
