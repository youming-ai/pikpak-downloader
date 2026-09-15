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

/// A callback returning the refresh token the embedder has persisted, if any.
#[derive(Clone)]
struct TokenSource(Arc<dyn Fn() -> Option<String> + Send + Sync>);

impl std::fmt::Debug for TokenSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("TokenSource")
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
    /// Optional callback returning the token the embedder has persisted, used to
    /// recover when someone else rotated it first.
    token_source: OnceLock<TokenSource>,
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
                token_source: OnceLock::new(),
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

    /// Register a callback returning the refresh token the embedder has
    /// persisted (from a `.env` file, a keyring, …), or `None` if there is none.
    ///
    /// PikPak invalidates the previous refresh token on every exchange, so a
    /// token read a moment ago can already be superseded — typically when a
    /// second copy of the CLI, or the web client, refreshed in the meantime. When
    /// an exchange is rejected as an invalid grant, the source is consulted and a
    /// *different* token it returns is retried once, so concurrent runs recover
    /// instead of failing while the working replacement sits on disk.
    ///
    /// The callback runs while the access-token cache is locked for this refresh,
    /// so keep it fast and do not call back into this manager. Registering twice
    /// is a no-op.
    pub fn set_token_source(&self, source: impl Fn() -> Option<String> + Send + Sync + 'static) {
        let _ = self.inner.token_source.set(TokenSource(Arc::new(source)));
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

        let response = match self.exchange(&current_refresh).await {
            Ok(response) => response,
            Err(failure) if failure.grant_rejected => {
                // Someone else may have rotated this token already and saved the
                // replacement where we read ours from. Only a *different* token is
                // worth one retry; repeating the same one would fail identically.
                match self.reloaded_token(&current_refresh) {
                    Some(replacement) => {
                        tracing::debug!(
                            "refresh token was rejected; retrying with the token from the \
                             configured source"
                        );
                        self.exchange(&replacement).await.map_err(|f| f.error)?
                    }
                    None => return Err(failure.error),
                }
            }
            Err(failure) => return Err(failure.error),
        };

        let RefreshResponse {
            access_token,
            refresh_token,
            expires_in,
            sub,
        } = response;
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

    /// Exchange `refresh_token` for an access token.
    ///
    /// Fails with the failure classified for the caller, which has to decide
    /// whether a different token is worth trying.
    async fn exchange(
        &self,
        refresh_token: &str,
    ) -> std::result::Result<RefreshResponse, ExchangeFailure> {
        let req = self
            .inner
            .http
            .post(&self.inner.auth_endpoint)
            .header("X-Device-Id", &self.inner.device_id)
            .json(&serde_json::json!({
                "client_id": self.inner.credentials.client_id,
                "client_secret": self.inner.credentials.client_secret,
                "grant_type": "refresh_token",
                "refresh_token": refresh_token,
            }));

        tracing::debug!(endpoint = %self.inner.auth_endpoint, "refreshing access token");
        let resp = req.send().await.map_err(|error| ExchangeFailure {
            grant_rejected: false,
            error: error.into(),
        })?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            return Err(ExchangeFailure {
                grant_rejected: is_grant_rejection(&body),
                error: explain_auth_failure(status, &body),
            });
        }
        resp.json().await.map_err(|error| ExchangeFailure {
            grant_rejected: false,
            error: error.into(),
        })
    }

    /// The token the embedder has persisted, if it is worth retrying with.
    ///
    /// Returns `None` when there is no source, nothing stored, or the stored
    /// value is the very token that was just rejected.
    fn reloaded_token(&self, rejected: &str) -> Option<String> {
        let source = self.inner.token_source.get()?;
        let candidate = (source.0)()?;
        if candidate.is_empty() || candidate == rejected {
            return None;
        }
        Some(candidate)
    }
}

/// One failed token exchange, classified for the retry decision.
struct ExchangeFailure {
    /// PikPak says this refresh token is unacceptable, so a *different* token may
    /// still work (as opposed to a transient or unrelated failure).
    grant_rejected: bool,
    error: Error,
}

/// Whether PikPak refused this refresh token outright.
///
/// `invalid_grant` (its error_code 4126) means the token itself is the problem,
/// so a *different* one may still work — as opposed to a transient or unrelated
/// failure, where retrying with another token would be pointless.
fn is_grant_rejection(body: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(body)
        .ok()
        .is_some_and(|v| {
            v.get("error").and_then(|e| e.as_str()) == Some("invalid_grant")
                || v.get("error_code").and_then(|c| c.as_u64()) == Some(4126)
        })
}

/// Turn a failed token exchange into the most useful error we can produce.
///
/// PikPak reports a rejected refresh token as `invalid_grant` (error_code 4126)
/// for two everyday situations, both of which lock the user out until they act:
/// the token has already been superseded — every exchange rotates it, so any
/// other copy (the web client refreshing in the background, or an earlier run of
/// this tool whose replacement was not kept) is dead — or it was issued for a
/// different client platform than the Android one this crate authenticates as.
/// Naming both, and the remedy, turns the server's opaque JSON into something
/// the user can act on.
fn explain_auth_failure(status: u16, body: &str) -> Error {
    if !is_grant_rejection(body) {
        return Error::Auth(format!("status {status}: {body}"));
    }
    Error::Auth(format!(
        "status {status}: PikPak rejected the refresh token (invalid_grant). \
         The token is single-use — every login rotates it, so the copy you supplied may \
         already have been superseded by the web client refreshing in the background, or \
         by an earlier run of this tool. It may also have been issued for a different \
         client platform: web-app tokens are not always refreshable by this tool's \
         Android-style client. Put a fresh token in a .env file (rotations are persisted \
         there automatically), keep other PikPak sessions logged out while this runs, and \
         see the README's Troubleshooting section. Server response: {body}"
    ))
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

    /// Serve one canned HTTP response, then close.
    async fn single_shot_server(
        status: u16,
        reason: &'static str,
        body: &'static str,
    ) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let _ = crate::test_http::read_request(&mut sock).await;
            crate::test_http::respond_json(&mut sock, status, reason, body).await;
        });
        addr
    }

    /// Serve scripted responses in order, recording each request's refresh token.
    ///
    /// Anything past the script answers 500, so an unexpected extra request is
    /// visible rather than silently accepted.
    async fn scripted_auth_server(
        responses: Vec<(u16, &'static str)>,
    ) -> (std::net::SocketAddr, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorder = seen.clone();
        tokio::spawn(async move {
            let mut index = 0usize;
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let (status, body) = responses
                    .get(index)
                    .copied()
                    .unwrap_or((500, "unexpected request"));
                index += 1;
                let recorder = recorder.clone();
                tokio::spawn(async move {
                    let request = crate::test_http::read_request(&mut sock).await;
                    let sent = serde_json::from_str::<serde_json::Value>(&request.body)
                        .ok()
                        .and_then(|v| {
                            v.get("refresh_token")
                                .and_then(|t| t.as_str())
                                .map(str::to_string)
                        })
                        .unwrap_or_default();
                    recorder.lock().unwrap().push(sent);
                    let reason = if status == 200 { "OK" } else { "Error" };
                    crate::test_http::respond_json(&mut sock, status, reason, body).await;
                });
            }
        });
        (addr, seen)
    }

    fn manager_for(addr: std::net::SocketAddr, token: &str) -> TokenManager {
        TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            token,
        )
    }

    const GRANT_REJECTION: &str = r#"{"error":"invalid_grant","error_code":4126}"#;
    const ROTATED_OK: &str = r#"{"access_token":"access-2","refresh_token":"rotated-2","expires_in":7200,"sub":"user-1"}"#;

    #[tokio::test]
    async fn a_rejected_grant_is_retried_with_the_reloaded_token() {
        // The situation the error message describes: another process has already
        // rotated the token and saved the replacement, so this run's copy is dead
        // while a working one sits on disk.
        let (addr, seen) =
            scripted_auth_server(vec![(400, GRANT_REJECTION), (200, ROTATED_OK)]).await;
        let tokens = manager_for(addr, "stale-token");
        tokens.set_token_source(|| Some("fresh-token".to_string()));

        assert_eq!(tokens.access_token().await.unwrap(), "access-2");
        assert_eq!(
            *seen.lock().unwrap(),
            vec!["stale-token".to_string(), "fresh-token".to_string()],
            "the retry must carry the reloaded token"
        );
        assert_eq!(
            tokens.current_refresh_token().await,
            "rotated-2",
            "the retry's rotation is the token that gets persisted from here on"
        );
    }

    #[tokio::test]
    async fn a_rejected_grant_is_not_retried_with_the_same_token() {
        let (addr, seen) = scripted_auth_server(vec![(400, GRANT_REJECTION)]).await;
        let tokens = manager_for(addr, "stale-token");
        // The source still holds the token that was just rejected: sending it
        // again would fail identically, so it must not be sent again.
        tokens.set_token_source(|| Some("stale-token".to_string()));

        assert!(tokens.access_token().await.is_err());
        assert_eq!(*seen.lock().unwrap(), vec!["stale-token".to_string()]);
    }

    #[tokio::test]
    async fn a_transient_auth_failure_is_not_retried_with_another_token() {
        // 503 says nothing about the token, so the source must not be consulted.
        let (addr, seen) = scripted_auth_server(vec![(503, "backend gone")]).await;
        let tokens = manager_for(addr, "stale-token");
        tokens.set_token_source(|| Some("fresh-token".to_string()));

        assert!(tokens.access_token().await.is_err());
        assert_eq!(*seen.lock().unwrap(), vec!["stale-token".to_string()]);
    }

    #[tokio::test]
    async fn a_grant_rejection_is_retried_only_once() {
        // The source yields a *different* token every time, so an implementation
        // that kept retrying would keep finding something new to try; exactly two
        // requests proves there is one retry and no loop.
        let (addr, seen) =
            scripted_auth_server(vec![(400, GRANT_REJECTION), (400, GRANT_REJECTION)]).await;
        let tokens = manager_for(addr, "stale-token");
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = attempts.clone();
        tokens.set_token_source(move || {
            Some(format!(
                "reloaded-{}",
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            ))
        });

        assert!(tokens.access_token().await.is_err());
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "exactly one retry, even when the source keeps offering new tokens"
        );
    }

    #[tokio::test]
    async fn a_rejected_grant_explains_its_causes() {
        // The verbatim rejection from issue #2: a refresh token PikPak refuses
        // as already superseded. The server's JSON alone says nothing about
        // what to do, so the error must.
        let body = r#"{"error":"invalid_grant","error_code":4126,"error_description":"invalid refresh token for it may be has been refreshed by other process, more info redis: nil, RefreshToken [REDACTED]"}"#;
        let addr = single_shot_server(400, "Bad Request", body).await;
        let tokens = TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            "superseded-token",
        );

        let err = tokens
            .access_token()
            .await
            .expect_err("the rejected exchange must fail");
        let text = format!("{err:#}");

        assert!(text.contains("invalid_grant"), "{text}");
        assert!(
            text.contains("single-use"),
            "must explain the rotation cause: {text}"
        );
        assert!(
            text.contains("different client platform"),
            "must explain the client-binding cause: {text}"
        );
        assert!(
            text.contains(".env"),
            "must point at the .env remedy: {text}"
        );
        assert!(text.contains("Server response:"), "{text}");
    }

    #[tokio::test]
    async fn an_unrelated_auth_failure_stays_plain() {
        // Only the token rejection gets the long explanation; anything else
        // keeps the terse status line.
        let addr = single_shot_server(503, "Service Unavailable", "backend gone").await;
        let tokens = TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            "some-token",
        );

        let err = tokens
            .access_token()
            .await
            .expect_err("the failed exchange must fail");
        let text = format!("{err:#}");

        assert!(text.contains("status 503"), "{text}");
        assert!(!text.contains("single-use"), "{text}");
    }
}
