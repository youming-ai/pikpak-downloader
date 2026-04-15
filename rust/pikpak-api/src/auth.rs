//! OAuth2 refresh-token flow for PikPak.
//!
//! PikPak uses a long-lived refresh token (obtainable from the web UI's
//! local storage) which we exchange for a short-lived access token. The
//! access token is attached as `Authorization: Bearer <token>` on every
//! subsequent API call.
//!
//! Endpoint: `POST https://user.mypikpak.com/v1/auth/token`
//!
//! Request body (JSON):
//! ```json
//! { "client_id": "...", "client_secret": "...",
//!   "grant_type": "refresh_token", "refresh_token": "..." }
//! ```
//!
//! Response body (JSON):
//! ```json
//! { "access_token": "...", "refresh_token": "...",
//!   "expires_in": 7200, "sub": "<user_id>" }
//! ```
//!
//! PikPak *rotates* the refresh token on each exchange; we keep the latest
//! one in memory but don't persist it — callers that want persistence
//! should read [`TokenManager::current_refresh_token`] and store it.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::Deserialize;
use tokio::sync::RwLock;

use crate::error::{Error, Result};

/// OAuth client credentials baked into the upstream `pikpakcli` project
/// (as of its 2025 builds). These identify the *application*, not the user.
///
/// We duplicate them here only because PikPak has no publicly registered
/// OAuth application story; callers can override via
/// [`ClientBuilder::credentials`](crate::ClientBuilder::credentials)
/// if PikPak rotates them.
/// Default OAuth client id (upstream pikpakcli value).
pub const DEFAULT_CLIENT_ID: &str = "YNxT9w7GMdWvEOKa";
/// Default OAuth client secret (upstream pikpakcli value).
pub const DEFAULT_CLIENT_SECRET: &str = "dbw2OtmVEeuUvIptb1Coyg";

/// OAuth client credentials for the PikPak auth server.
///
/// Not per-user secrets: these identify the application (pikpakcli's
/// Android build). Defaults are crate constants.
#[derive(Debug, Clone)]
pub struct OAuthCredentials {
    /// OAuth client id.
    pub client_id: String,
    /// OAuth client secret.
    pub client_secret: String,
}

impl Default for OAuthCredentials {
    fn default() -> Self {
        Self {
            client_id: DEFAULT_CLIENT_ID.to_string(),
            client_secret: DEFAULT_CLIENT_SECRET.to_string(),
        }
    }
}

impl OAuthCredentials {
    /// Construct credentials from explicit values.
    pub fn new(client_id: impl Into<String>, client_secret: impl Into<String>) -> Self {
        Self {
            client_id: client_id.into(),
            client_secret: client_secret.into(),
        }
    }
}

/// A cached access token plus its expiry instant and owning user id.
#[derive(Debug, Clone)]
struct CachedToken {
    access_token: String,
    refresh_token: String,
    user_id: String,
    /// Wall-clock deadline after which we must refresh. We refresh a minute
    /// before the server-reported expiry to absorb clock skew.
    refresh_at: Instant,
}

/// Token manager: keeps the current access token and refreshes it on demand.
///
/// Cheap to clone — internally shares state behind an `Arc<RwLock<_>>` so
/// concurrent requests share a single refresh call.
#[derive(Debug, Clone)]
pub struct TokenManager {
    inner: Arc<TokenManagerInner>,
}

#[derive(Debug)]
struct TokenManagerInner {
    http: reqwest::Client,
    auth_endpoint: String,
    credentials: OAuthCredentials,
    device_id: String,
    cached: RwLock<Option<CachedToken>>,
    /// Initial refresh token supplied by the caller. Kept so that after a
    /// rotation we can recover if the in-memory cache is reset.
    initial_refresh_token: String,
}

impl TokenManager {
    /// Create a new token manager. Does not contact the network.
    pub fn new(
        http: reqwest::Client,
        auth_endpoint: impl Into<String>,
        credentials: OAuthCredentials,
        device_id: impl Into<String>,
        refresh_token: impl Into<String>,
    ) -> Self {
        let refresh_token = refresh_token.into();
        Self {
            inner: Arc::new(TokenManagerInner {
                http,
                auth_endpoint: auth_endpoint.into(),
                credentials,
                device_id: device_id.into(),
                cached: RwLock::new(None),
                initial_refresh_token: refresh_token,
            }),
        }
    }

    /// Return a currently-valid access token, refreshing if needed.
    pub async fn access_token(&self) -> Result<String> {
        // Fast path: still valid.
        {
            let guard = self.inner.cached.read().await;
            if let Some(t) = guard.as_ref() {
                if Instant::now() < t.refresh_at {
                    return Ok(t.access_token.clone());
                }
            }
        }

        // Slow path: acquire write lock and refresh. Double-check inside the
        // write lock in case another task refreshed while we were waiting.
        let mut guard = self.inner.cached.write().await;
        if let Some(t) = guard.as_ref() {
            if Instant::now() < t.refresh_at {
                return Ok(t.access_token.clone());
            }
        }

        let fresh = self.refresh_now(&mut guard).await?;
        Ok(fresh)
    }

    /// Return the `sub` (user id) claim from the current token, refreshing
    /// if necessary. Used by the captcha flow to bind requests to a user.
    pub async fn user_id(&self) -> Result<String> {
        // Trigger a refresh if needed, then read the cache.
        let _ = self.access_token().await?;
        let guard = self.inner.cached.read().await;
        guard
            .as_ref()
            .map(|t| t.user_id.clone())
            .ok_or(Error::TokenExpired)
    }

    /// Return the most recently issued refresh token (rotated on each
    /// exchange), or the initial one if we haven't refreshed yet.
    pub async fn current_refresh_token(&self) -> String {
        let guard = self.inner.cached.read().await;
        guard
            .as_ref()
            .map(|t| t.refresh_token.clone())
            .unwrap_or_else(|| self.inner.initial_refresh_token.clone())
    }

    /// Perform the actual refresh call. Caller holds the write lock.
    async fn refresh_now(
        &self,
        guard: &mut tokio::sync::RwLockWriteGuard<'_, Option<CachedToken>>,
    ) -> Result<String> {
        let current_refresh = guard
            .as_ref()
            .map(|t| t.refresh_token.clone())
            .unwrap_or_else(|| self.inner.initial_refresh_token.clone());

        let req = self
            .inner
            .http
            .post(&self.inner.auth_endpoint)
            .header("X-Device-Id", &self.inner.device_id)
            .json(&serde_json::json!({
                "client_id": self.inner.credentials.client_id,
                "client_secret": self.inner.credentials.client_secret,
                "grant_type": "refresh_token",
                "refresh_token": current_refresh,
            }));

        tracing::debug!(endpoint = %self.inner.auth_endpoint, "refreshing access token");
        let resp = req.send().await?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(Error::Auth(format!("status {status}: {body}")));
        }

        let body: RefreshResponse = resp.json().await?;
        let refresh_at =
            Instant::now() + Duration::from_secs(body.expires_in.saturating_sub(60));
        let access = body.access_token.clone();
        **guard = Some(CachedToken {
            access_token: body.access_token,
            refresh_token: body.refresh_token,
            user_id: body.sub,
            refresh_at,
        });
        Ok(access)
    }
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    sub: String,
}
