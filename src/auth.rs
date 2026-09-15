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
//! PikPak *rotates* the refresh token on each exchange. We keep the latest one
//! in memory and never touch the disk ourselves; an embedder that must persist
//! it can either register [`TokenManager::set_rotation_hook`] — called the
//! moment the server issues a new token — or read
//! [`TokenManager::current_refresh_token`] later.

use std::sync::{Arc, OnceLock};
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
    user_id: String,
    /// Wall-clock deadline after which we must refresh. We refresh a minute
    /// before the server-reported expiry to absorb clock skew.
    refresh_at: Instant,
}

/// A callback invoked with each newly rotated refresh token.
#[derive(Clone)]
struct RotationHook(Arc<dyn Fn(&str) + Send + Sync>);

impl std::fmt::Debug for RotationHook {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RotationHook")
    }
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
    /// Cached access token. Cleared by [`TokenManager::invalidate`] when the
    /// server rejects it.
    cached: RwLock<Option<CachedToken>>,
    /// The latest refresh token, rotated by the server on every exchange.
    ///
    /// Deliberately kept *separate* from `cached`: invalidating the access token
    /// must not discard the rotated refresh token, because PikPak invalidates
    /// the previous one — falling back to it would break authentication.
    refresh_token: RwLock<String>,
    /// Optional callback fired with every newly issued refresh token, the moment
    /// the server returns it.
    rotation_hook: OnceLock<RotationHook>,
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
                refresh_token: RwLock::new(refresh_token),
                rotation_hook: OnceLock::new(),
            }),
        }
    }

    /// Register a callback invoked with every newly rotated refresh token.
    ///
    /// PikPak invalidates the previous refresh token on each exchange, so a
    /// process that exits before persisting the new value can lose access to the
    /// account. Firing the hook the moment the server issues the token shrinks
    /// that window from "whenever the command finishes" to the rotation itself.
    ///
    /// The hook runs after the new value has been stored and without the
    /// refresh-token lock held, but it does run while the access-token cache is
    /// locked for this refresh — so keep it fast and do not call back into this
    /// manager. Registering twice is a no-op.
    pub fn set_rotation_hook(&self, hook: impl Fn(&str) + Send + Sync + 'static) {
        let _ = self.inner.rotation_hook.set(RotationHook(Arc::new(hook)));
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

    /// Drop the cached access token so the next [`access_token`] call
    /// forces a fresh refresh. Used when the server rejects a token we
    /// still believed valid (HTTP 401).
    ///
    /// The rotated refresh token is deliberately retained: the next refresh
    /// must use the newest one, since the server invalidates the previous.
    ///
    /// [`access_token`]: Self::access_token
    pub async fn invalidate(&self) {
        let mut guard = self.inner.cached.write().await;
        *guard = None;
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
        self.inner.refresh_token.read().await.clone()
    }

    /// Perform the actual refresh call. Caller holds the write lock.
    async fn refresh_now(
        &self,
        guard: &mut tokio::sync::RwLockWriteGuard<'_, Option<CachedToken>>,
    ) -> Result<String> {
        // Copy the latest rotated token out before the await; no lock guard may
        // be held across it.
        let current_refresh = self.inner.refresh_token.read().await.clone();

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

        let RefreshResponse {
            access_token,
            refresh_token,
            expires_in,
            sub,
        } = resp.json().await?;
        let effective_ttl = expires_in.saturating_sub(60).max(5);
        let refresh_at = Instant::now() + Duration::from_secs(effective_ttl);

        // Store the rotated refresh token before publishing the access token, so
        // an invalidate() racing with this can never resurrect the previous
        // (server-invalidated) refresh token.
        {
            *self.inner.refresh_token.write().await = refresh_token.clone();
        }
        // Notify the embedder only once the new value is safe to persist, and
        // without holding the lock across the callback.
        if let Some(hook) = self.inner.rotation_hook.get() {
            (hook.0)(&refresh_token);
        }

        **guard = Some(CachedToken {
            access_token: access_token.clone(),
            user_id: sub,
            refresh_at,
        });
        Ok(access_token)
    }
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    access_token: String,
    refresh_token: String,
    expires_in: u64,
    sub: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Minimal stand-in for `POST /v1/auth/token`: records the refresh token it
    /// was sent and answers with a freshly rotated one (`rotated-N`) each time.
    async fn spawn_auth_server(seen: Arc<Mutex<Vec<String>>>) -> std::net::SocketAddr {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let issued = Arc::new(AtomicUsize::new(0));
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let seen = seen.clone();
                let issued = issued.clone();
                tokio::spawn(async move {
                    let request = crate::test_http::read_request(&mut sock).await;
                    let sent = serde_json::from_str::<serde_json::Value>(&request.body)
                        .ok()
                        .and_then(|v| {
                            v.get("refresh_token")
                                .and_then(|s| s.as_str())
                                .map(str::to_string)
                        })
                        .unwrap_or_else(|| "<unparsed>".into());
                    seen.lock().unwrap().push(sent);

                    let n = issued.fetch_add(1, Ordering::SeqCst) + 1;
                    let resp = format!(
                        r#"{{"access_token":"access-{n}","refresh_token":"rotated-{n}","expires_in":7200,"sub":"user-1"}}"#
                    );
                    crate::test_http::respond_json(&mut sock, 200, "OK", &resp).await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn invalidate_keeps_rotated_refresh_token() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_auth_server(seen.clone()).await;
        let tokens = TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            "initial-token",
        );

        assert_eq!(tokens.access_token().await.unwrap(), "access-1");
        assert_eq!(tokens.current_refresh_token().await, "rotated-1");

        // The server rejected the access token, so clear it and authenticate
        // again. The refresh must reuse the rotated token; falling back to the
        // initial one would fail against the real server, which invalidates the
        // previous refresh token on every rotation.
        tokens.invalidate().await;
        assert_eq!(tokens.access_token().await.unwrap(), "access-2");

        let sent = seen.lock().unwrap().clone();
        // The second request carries the token from the first rotation, not the
        // initial one.
        assert_eq!(sent, vec!["initial-token", "rotated-1"]);
        assert_eq!(tokens.current_refresh_token().await, "rotated-2");
    }

    #[tokio::test]
    async fn invalidate_drops_only_the_access_token() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_auth_server(seen.clone()).await;
        let tokens = TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            "initial-token",
        );

        // A freshly built manager reports the caller-supplied token and reads
        // it back unchanged when no refresh has happened yet.
        assert_eq!(tokens.current_refresh_token().await, "initial-token");
        tokens.invalidate().await;
        assert_eq!(tokens.current_refresh_token().await, "initial-token");
        assert_eq!(
            seen.lock().unwrap().len(),
            0,
            "invalidate must not hit the network"
        );
    }

    #[tokio::test]
    async fn rotation_hook_fires_with_each_new_token() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_auth_server(seen).await;
        let hook_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = hook_calls.clone();

        let tokens = TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            "initial-token",
        );
        tokens.set_rotation_hook(move |token| recorder.lock().unwrap().push(token.to_string()));

        // The caller-supplied token is not a rotation, so it must not be
        // reported: only values the server actually issues are.
        assert!(hook_calls.lock().unwrap().is_empty());

        tokens.access_token().await.unwrap();
        assert_eq!(hook_calls.lock().unwrap().clone(), vec!["rotated-1"]);

        tokens.invalidate().await;
        tokens.access_token().await.unwrap();
        assert_eq!(
            hook_calls.lock().unwrap().clone(),
            vec!["rotated-1", "rotated-2"],
            "every rotation must be reported, not just the first"
        );
    }

    #[tokio::test]
    async fn client_builder_wires_the_rotation_hook() {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let addr = spawn_auth_server(seen).await;
        let hook_calls: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let recorder = hook_calls.clone();

        let client = crate::client::Client::builder()
            .refresh_token("initial-token")
            .auth_base_url(format!("http://{addr}"))
            .on_refresh_token(move |token| recorder.lock().unwrap().push(token.to_string()))
            .build()
            .unwrap();

        client.tokens().access_token().await.unwrap();
        assert_eq!(hook_calls.lock().unwrap().clone(), vec!["rotated-1"]);
    }
}
