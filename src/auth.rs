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
//! it registers [`ClientBuilder::on_refresh_token`](crate::ClientBuilder::on_refresh_token),
//! called the moment the server issues a new token.

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
    user_id: String,
    /// Wall-clock deadline after which we must refresh. We refresh a minute
    /// before the server-reported expiry to absorb clock skew.
    refresh_at: Instant,
}

/// The embedder's callbacks around token rotation; see
/// [`ClientBuilder::on_refresh_token`](crate::ClientBuilder::on_refresh_token)
/// and [`ClientBuilder::refresh_token_source`](crate::ClientBuilder::refresh_token_source).
///
/// Both run while the access-token cache is locked for the refresh, so they
/// must be fast and must not call back into the client.
#[derive(Clone, Default)]
pub(crate) struct TokenHooks {
    /// Called with every newly issued refresh token, once it is stored.
    pub rotation: Option<RotationHook>,
    /// Returns the token the embedder has persisted, tried once when the
    /// current one is rejected and the persisted one differs.
    pub source: Option<TokenSource>,
}

pub(crate) type RotationHook = Arc<dyn Fn(&str) + Send + Sync>;
pub(crate) type TokenSource = Arc<dyn Fn() -> Option<String> + Send + Sync>;

// Hand-written so `{:?}` never prints a live token: the cached access token and
// the refresh token both stay out of it.
impl std::fmt::Debug for TokenManagerInner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenManagerInner")
            .field("auth_endpoint", &self.auth_endpoint)
            .field("device_id", &self.device_id)
            .finish_non_exhaustive()
    }
}

/// Token manager: keeps the current access token and refreshes it on demand.
///
/// Cheap to clone — internally shares state behind an `Arc<RwLock<_>>` so
/// concurrent requests share a single refresh call.
#[derive(Debug, Clone)]
pub(crate) struct TokenManager {
    inner: Arc<TokenManagerInner>,
}

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
    hooks: TokenHooks,
}

impl TokenManager {
    /// Create a new token manager. Does not contact the network.
    pub(crate) fn new(
        http: reqwest::Client,
        auth_endpoint: impl Into<String>,
        credentials: OAuthCredentials,
        device_id: impl Into<String>,
        refresh_token: impl Into<String>,
        hooks: TokenHooks,
    ) -> Self {
        Self {
            inner: Arc::new(TokenManagerInner {
                http,
                auth_endpoint: auth_endpoint.into(),
                credentials,
                device_id: device_id.into(),
                cached: RwLock::new(None),
                refresh_token: RwLock::new(refresh_token.into()),
                hooks,
            }),
        }
    }

    /// A fresh cached token, refreshing first when the current one is stale.
    ///
    /// `pick` selects the field the caller needs, so [`access_token`] and
    /// [`user_id`] share one locking protocol instead of two copies of it that
    /// have to be kept in step.
    ///
    /// [`access_token`]: Self::access_token
    /// [`user_id`]: Self::user_id
    async fn cached_token_after_refresh<T>(&self, pick: impl Fn(&CachedToken) -> T) -> Result<T> {
        // Fast path: still valid.
        {
            let guard = self.inner.cached.read().await;
            if let Some(t) = guard.as_ref() {
                if Instant::now() < t.refresh_at {
                    return Ok(pick(t));
                }
            }
        }

        // Slow path: acquire the write lock and refresh. Double-check inside the
        // write lock in case another task refreshed while we were waiting.
        let mut guard = self.inner.cached.write().await;
        if let Some(t) = guard.as_ref() {
            if Instant::now() < t.refresh_at {
                return Ok(pick(t));
            }
        }

        self.refresh_now(&mut guard).await?;
        guard.as_ref().map(pick).ok_or(Error::TokenExpired)
    }

    /// Return a currently-valid access token, refreshing if needed.
    pub(crate) async fn access_token(&self) -> Result<String> {
        self.cached_token_after_refresh(|t| t.access_token.clone())
            .await
    }

    /// Drop the cached access token `rejected` — the server refused it (HTTP
    /// 401) — so the next [`access_token`](Self::access_token) call refreshes.
    ///
    /// Only while it is still the cached token: concurrent requests rejected
    /// together would otherwise each wipe the token the first of them just
    /// refreshed, and each rotation costs a single-use refresh token. The
    /// rotated refresh token is kept either way, since the server has already
    /// invalidated the previous one.
    pub(crate) async fn invalidate_if_current(&self, rejected: &str) {
        let mut guard = self.inner.cached.write().await;
        if guard
            .as_ref()
            .is_some_and(|cached| cached.access_token == rejected)
        {
            *guard = None;
        }
    }

    /// Return the `sub` (user id) claim from the current token, refreshing
    /// if necessary. Used by the captcha flow to bind requests to a user.
    pub(crate) async fn user_id(&self) -> Result<String> {
        self.cached_token_after_refresh(|t| t.user_id.clone()).await
    }

    /// Return the most recently issued refresh token (rotated on each
    /// exchange), or the initial one if we haven't refreshed yet.
    #[cfg(test)]
    pub(crate) async fn current_refresh_token(&self) -> String {
        self.inner.refresh_token.read().await.clone()
    }

    /// Perform the actual refresh call. Caller holds the write lock.
    async fn refresh_now(
        &self,
        guard: &mut tokio::sync::RwLockWriteGuard<'_, Option<CachedToken>>,
    ) -> Result<()> {
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

        // Take the rotated refresh token before reading anything else: the server
        // has already invalidated the one we sent, so a response we cannot make
        // full sense of must not take the only valid token down with it.
        let refresh_token = match response.get("refresh_token") {
            Some(serde_json::Value::String(token)) if !token.is_empty() => token.clone(),
            _ => {
                return Err(Error::Auth(
                    "the token response carried no refresh_token".to_string(),
                ))
            }
        };

        // Store the rotated refresh token before publishing the access token, so
        // an invalidation racing with this can never resurrect the previous
        // (server-invalidated) refresh token.
        {
            *self.inner.refresh_token.write().await = refresh_token.clone();
        }
        // Notify the embedder only once the new value is safe to persist, and
        // without holding the lock across the callback.
        if let Some(hook) = &self.inner.hooks.rotation {
            hook(&refresh_token);
        }

        let AccessGrant {
            access_token,
            expires_in,
            sub,
        } = serde_json::from_value(response)?;
        let sub = sub.unwrap_or_default();
        let expires_in = expires_in.unwrap_or(ASSUMED_TOKEN_TTL_SECS);
        let effective_ttl = expires_in.saturating_sub(60).max(5);
        let refresh_at = Instant::now() + Duration::from_secs(effective_ttl);

        **guard = Some(CachedToken {
            access_token,
            user_id: sub,
            refresh_at,
        });
        Ok(())
    }

    /// Exchange `refresh_token` for an access token.
    ///
    /// Fails with the failure classified for the caller, which has to decide
    /// whether a different token is worth trying.
    async fn exchange(
        &self,
        refresh_token: &str,
    ) -> std::result::Result<serde_json::Value, ExchangeFailure> {
        let body = serde_json::json!({
            "client_id": self.inner.credentials.client_id,
            "client_secret": self.inner.credentials.client_secret,
            "grant_type": "refresh_token",
            "refresh_token": refresh_token,
        });

        tracing::debug!(endpoint = %self.inner.auth_endpoint, "refreshing access token");
        let resp = crate::client::send_retrying(
            || {
                self.inner
                    .http
                    .post(&self.inner.auth_endpoint)
                    .header("X-Device-Id", &self.inner.device_id)
                    .json(&body)
            },
            // Only a connection that never opened is safe to repeat: once the
            // token reached the server it may be spent already, with the
            // response that would carry its replacement lost.
            reqwest::Error::is_connect,
        )
        .await
        .map_err(|error| ExchangeFailure {
            grant_rejected: false,
            error: error.into(),
        })?;
        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let body = redact(&resp.text().await.unwrap_or_default(), refresh_token);
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
        let candidate = (self.inner.hooks.source.as_ref()?)()?;
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
pub(crate) fn is_grant_rejection(body: &str) -> bool {
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
pub(crate) fn explain_auth_failure(status: u16, body: &str) -> Error {
    if !is_grant_rejection(body) {
        // An overloaded or failing auth service says nothing about the token:
        // report it as the service's failure, so the caller retries later
        // instead of replacing a token that may be perfectly good.
        if status == 429 || status >= 500 {
            return Error::Api {
                status,
                message: body.to_string(),
            };
        }
        return Error::Auth(format!("status {status}: {body}"));
    }
    Error::Auth(format!(
        "status {status}: PikPak rejected the refresh token (invalid_grant). \
         The token is single-use — every login rotates it, so the copy you supplied may \
         already have been superseded by the web client refreshing in the background, or \
         by an earlier run of this tool. It may also have been issued for a different \
         client platform: web-app tokens are not always refreshable by this tool's \
         Android-style client. Put a fresh token in a .env file (rotations are persisted \
         there automatically) and keep the web app closed while this runs. \
         Server response: {body}"
    ))
}

/// The rest of a token response, read once its refresh token is safe — and
/// leniently, because a run that fails here has already spent the token it sent.
#[derive(Debug, Deserialize)]
struct AccessGrant {
    access_token: String,
    /// Seconds, as a number or a decimal string.
    #[serde(
        default,
        deserialize_with = "crate::types::deserialize_optional_lenient_u64"
    )]
    expires_in: Option<u64>,
    /// Only the captcha flow uses it.
    #[serde(default)]
    sub: Option<String>,
}

/// Lifetime assumed when a token response leaves `expires_in` out. Long on
/// purpose: every refresh rotates the single-use refresh token, and a guess
/// that turns out too long costs one 401, after which the token is refreshed.
const ASSUMED_TOKEN_TTL_SECS: u64 = 3600;

/// `body` with every occurrence of `secret` blanked out, for error text that
/// ends up in logs, job records and JSON documents. A server that echoes the
/// token it rejected must not get it written down in the clear.
pub(crate) fn redact(body: &str, secret: &str) -> String {
    // An empty secret would "match" between every character.
    if secret.is_empty() {
        return body.to_string();
    }
    body.replace(secret, "[redacted]")
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
            TokenHooks::default(),
        );

        assert_eq!(tokens.access_token().await.unwrap(), "access-1");
        assert_eq!(tokens.current_refresh_token().await, "rotated-1");

        // The server rejected the access token, so clear it and authenticate
        // again. The refresh must reuse the rotated token; falling back to the
        // initial one would fail against the real server, which invalidates the
        // previous refresh token on every rotation.
        tokens.invalidate_if_current("access-1").await;
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
            TokenHooks::default(),
        );

        // A freshly built manager reports the caller-supplied token and reads
        // it back unchanged when no refresh has happened yet.
        assert_eq!(tokens.current_refresh_token().await, "initial-token");
        tokens.invalidate_if_current("access-1").await;
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
            rotation(move |token| recorder.lock().unwrap().push(token.to_string())),
        );

        // The caller-supplied token is not a rotation, so it must not be
        // reported: only values the server actually issues are.
        assert!(hook_calls.lock().unwrap().is_empty());

        tokens.access_token().await.unwrap();
        assert_eq!(hook_calls.lock().unwrap().clone(), vec!["rotated-1"]);

        tokens.invalidate_if_current("access-1").await;
        tokens.access_token().await.unwrap();
        assert_eq!(
            hook_calls.lock().unwrap().clone(),
            vec!["rotated-1", "rotated-2"],
            "every rotation must be reported, not just the first"
        );
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
        manager_with(addr, token, TokenHooks::default())
    }

    fn manager_with(addr: std::net::SocketAddr, token: &str, hooks: TokenHooks) -> TokenManager {
        TokenManager::new(
            reqwest::Client::new(),
            format!("http://{addr}/v1/auth/token"),
            OAuthCredentials::default(),
            "device-1",
            token,
            hooks,
        )
    }

    fn rotation(hook: impl Fn(&str) + Send + Sync + 'static) -> TokenHooks {
        TokenHooks {
            rotation: Some(Arc::new(hook)),
            source: None,
        }
    }

    fn source(source: impl Fn() -> Option<String> + Send + Sync + 'static) -> TokenHooks {
        TokenHooks {
            rotation: None,
            source: Some(Arc::new(source)),
        }
    }

    #[tokio::test]
    async fn only_the_rejected_access_token_is_invalidated() {
        let (addr, seen) = scripted_auth_server(vec![(200, ROTATED_OK), (200, ROTATED_OK)]).await;
        let tokens = manager_for(addr, "token-1");
        assert_eq!(tokens.access_token().await.unwrap(), "access-2");

        // A request rejected with an older token must not wipe the current one:
        // that would spend another single-use refresh token for nothing.
        tokens.invalidate_if_current("access-1").await;
        tokens.access_token().await.unwrap();
        assert_eq!(seen.lock().unwrap().len(), 1, "no second refresh");

        tokens.invalidate_if_current("access-2").await;
        tokens.access_token().await.unwrap();
        assert_eq!(
            seen.lock().unwrap().len(),
            2,
            "the rejected token is replaced"
        );
    }

    #[tokio::test]
    async fn a_rejected_token_echoed_by_the_server_is_not_repeated() {
        let (addr, _) = scripted_auth_server(vec![(
            400,
            r#"{"error":"invalid_grant","error_description":"bad token sent-token-123"}"#,
        )])
        .await;
        let tokens = manager_for(addr, "sent-token-123");

        let text = format!("{:#}", tokens.access_token().await.unwrap_err());
        assert!(!text.contains("sent-token-123"), "{text}");
        assert!(text.contains("[redacted]"), "{text}");
    }

    #[test]
    fn redacting_nothing_changes_nothing() {
        assert_eq!(super::redact("body", ""), "body");
    }

    #[test]
    fn debug_output_never_shows_a_token() {
        let tokens = TokenManager::new(
            reqwest::Client::new(),
            "http://127.0.0.1:1/v1/auth/token".to_string(),
            OAuthCredentials::default(),
            "device-1",
            "secret-refresh-token",
            TokenHooks::default(),
        );
        let shown = format!("{tokens:?}");
        assert!(!shown.contains("secret-refresh-token"), "{shown}");
        assert!(shown.contains("device-1"), "{shown}");
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
        let tokens = manager_with(
            addr,
            "stale-token",
            source(|| Some("fresh-token".to_string())),
        );

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
        let tokens = manager_with(
            addr,
            "stale-token",
            source(|| Some("stale-token".to_string())),
        );
        // The source still holds the token that was just rejected: sending it
        // again would fail identically, so it must not be sent again.

        assert!(tokens.access_token().await.is_err());
        assert_eq!(*seen.lock().unwrap(), vec!["stale-token".to_string()]);
    }

    #[tokio::test]
    async fn a_transient_auth_failure_is_not_retried_with_another_token() {
        // 503 says nothing about the token, so the source must not be consulted.
        let (addr, seen) = scripted_auth_server(vec![(503, "backend gone")]).await;
        let tokens = manager_with(
            addr,
            "stale-token",
            source(|| Some("fresh-token".to_string())),
        );

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
        let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = attempts.clone();
        let tokens = manager_with(
            addr,
            "stale-token",
            source(move || {
                Some(format!(
                    "reloaded-{}",
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst)
                ))
            }),
        );

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
            TokenHooks::default(),
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
            TokenHooks::default(),
        );

        let err = tokens
            .access_token()
            .await
            .expect_err("the failed exchange must fail");
        let text = format!("{err:#}");

        assert!(text.contains("503"), "{text}");
        assert!(!text.contains("single-use"), "{text}");
        // The service's failure, not the token's: retry later, keep the token.
        assert!(matches!(err, Error::Api { status: 503, .. }), "{err:?}");
    }

    #[tokio::test]
    async fn a_token_response_with_odd_fields_is_read_leniently() {
        // `expires_in` as a string, `sub` missing: read leniently, so the run
        // carries on instead of failing after its token was already spent.
        let (addr, _) = scripted_auth_server(vec![(
            200,
            r#"{"access_token":"access-2","refresh_token":"rotated-2","expires_in":"7200"}"#,
        )])
        .await;
        let tokens = manager_for(addr, "old-token");

        assert_eq!(tokens.access_token().await.unwrap(), "access-2");
        assert_eq!(tokens.current_refresh_token().await, "rotated-2");
    }

    #[tokio::test]
    async fn a_rotated_token_survives_a_response_that_cannot_be_read_in_full() {
        // The server rotated (and so invalidated) the token we sent, then left
        // out the access token. The run fails, but the replacement must still
        // reach the store and the hook, or the account is locked out.
        let (addr, _) = scripted_auth_server(vec![(200, r#"{"refresh_token":"rotated-2"}"#)]).await;
        let persisted = Arc::new(std::sync::Mutex::new(None));
        let sink = persisted.clone();
        let tokens = manager_with(
            addr,
            "old-token",
            rotation(move |token| *sink.lock().unwrap() = Some(token.to_string())),
        );

        assert!(tokens.access_token().await.is_err());
        assert_eq!(tokens.current_refresh_token().await, "rotated-2");
        assert_eq!(persisted.lock().unwrap().as_deref(), Some("rotated-2"));
    }

    #[tokio::test]
    async fn a_connection_that_never_opened_is_retried() {
        // The auth server is not up yet for the first attempt: nothing was sent,
        // so trying again cannot spend the token twice.
        let addr = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .unwrap()
            .local_addr()
            .unwrap();
        let tokens = manager_for(addr, "initial-token");
        let server = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            let listener = tokio::net::TcpListener::bind(addr).await.unwrap();
            let (mut sock, _) = listener.accept().await.unwrap();
            let _ = crate::test_http::read_request(&mut sock).await;
            crate::test_http::respond_json(&mut sock, 200, "OK", crate::test_http::TOKEN_JSON)
                .await;
        });

        assert_eq!(tokens.access_token().await.unwrap(), "access");
        server.await.unwrap();
    }

    #[tokio::test]
    async fn a_request_that_reached_the_server_is_not_repeated() {
        // The server took the request and hung up without answering: the token
        // may be spent, so sending it again could only make things worse.
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let accepted = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = accepted.clone();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let _ = crate::test_http::read_request(&mut sock).await;
            }
        });

        let tokens = manager_for(addr, "initial-token");
        assert!(tokens.access_token().await.is_err());
        assert_eq!(accepted.load(std::sync::atomic::Ordering::SeqCst), 1);
    }
}
