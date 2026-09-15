//! PikPak drive-API captcha token flow.
//!
//! Every request to `api-drive.mypikpak.com/drive/v1/*` must carry an
//! `X-Captcha-Token` header. The token is obtained from
//! `POST https://user.mypikpak.com/v1/shield/captcha/init` and requires
//! a chained-MD5 signature proving the client knows the app package
//! identity + a hard-coded salt table.
//!
//! The server rotates captcha tokens; it returns error_code `9` on drive
//! endpoints when the current token has expired, at which point the
//! client must re-init and retry.
//!
//! This implementation caches the token per-action (the captcha is bound
//! to the request method + path it was issued for) and transparently
//! re-inits when the cached one stops working.
//!
//! The MD5 chain algorithm and salt table come from the upstream
//! `pikpakcli` project. See `cmd/pikpakcli/internal/pikpak/captcha_token.go`
//! in that repo.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use serde::Deserialize;
use tokio::sync::{Mutex, RwLock};

use crate::auth::TokenManager;
use crate::error::{Error, Result};

/// Android client package name baked into pikpakcli.
const CLIENT_PACKAGE_NAME: &str = "com.pikcloud.pikpak";
/// Android client version baked into pikpakcli.
const CLIENT_VERSION: &str = "1.21.0";

/// 9-entry salt table used to build the captcha_sign.
///
/// Empty strings are meaningful: the very first entry is empty so the
/// first MD5 is `md5(client_id + version + package + device_id + ts)`,
/// with no appended salt.
const SALTS: &[&str] = &[
    "",
    "E32cSkYXC2bciKJGxRsE8ZgwmH/YwkvpD6/O9guSOa2irCwciH4xPHaH",
    "QtqgfMgHP2TFl",
    "zOKgHT56L7nIzFzDpUGhpWFrgP53m3G6ML",
    "S",
    "THxpsktzfFXizUv7DK1y/N7NZ1WhayViluBEvAJJ8bA1Wr6",
    "y9PXH3xGUhG/zQI8CaapRw2LhldCaFM9CRlKpZXJvj+pifu",
    "+RaaG7T8FRTI4cP019N5y9ofLyHE9ySFUr",
    "6Pf1l8UTeuzYldGtb/d",
];

/// How long to trust a freshly issued captcha token before refreshing
/// proactively. PikPak doesn't advertise a TTL; pikpakcli re-inits on
/// demand when it sees error_code 9.
const PROACTIVE_REFRESH: Duration = Duration::from_secs(60 * 30); // 30 min

/// Manages captcha tokens for drive-API actions.
///
/// Cheap to clone.
#[derive(Clone)]
pub(crate) struct CaptchaManager {
    inner: Arc<CaptchaManagerInner>,
}

struct CaptchaManagerInner {
    http: reqwest::Client,
    init_endpoint: String,
    client_id: String,
    device_id: String,
    tokens: TokenManager,
    /// Cache of (action -> cached captcha).
    cache: RwLock<HashMap<String, CachedCaptcha>>,
    /// One lock per action.
    ///
    /// Refreshes of the same action must collapse into a single init request,
    /// but a refresh of one action must not stall another action's request for a
    /// different token — and it must never block cache reads. Holding a lock per
    /// action (rather than the cache lock itself) across the network call gives
    /// exactly that; the map is bounded by the handful of actions in use.
    action_locks: Mutex<HashMap<String, Arc<Mutex<()>>>>,
}

#[derive(Clone)]
struct CachedCaptcha {
    token: String,
    refresh_after: Instant,
}

impl CaptchaManager {
    pub(crate) fn new(
        http: reqwest::Client,
        init_endpoint: impl Into<String>,
        client_id: impl Into<String>,
        device_id: impl Into<String>,
        tokens: TokenManager,
    ) -> Self {
        Self {
            inner: Arc::new(CaptchaManagerInner {
                http,
                init_endpoint: init_endpoint.into(),
                client_id: client_id.into(),
                device_id: device_id.into(),
                tokens,
                cache: RwLock::new(HashMap::new()),
                action_locks: Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Return a captcha token for the given action (e.g. `"GET:/drive/v1/files"`).
    pub(crate) async fn token_for(&self, action: &str) -> Result<String> {
        if let Some(token) = self.cached_token(action, None).await {
            return Ok(token);
        }
        self.refresh(action, None).await
    }

    /// Force a fresh captcha token for the given action. Optionally pass the
    /// previous token so the server can invalidate it. Returns the new token.
    ///
    /// Only refreshes of the *same* action wait for each other, so a burst of
    /// workers still collapses into one init request. Different actions — and
    /// every cache read — proceed in parallel: the network call is never made
    /// while holding the shared cache lock.
    pub(crate) async fn refresh(&self, action: &str, previous: Option<&str>) -> Result<String> {
        let action_lock = self
            .inner
            .action_locks
            .lock()
            .await
            .entry(action.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone();
        let _guard = action_lock.lock().await;

        // Re-check under the action lock: another task may have refreshed while
        // we were waiting. If `previous` is supplied and the cached token is
        // still the one that just failed, force a real refresh instead of
        // handing back the known-bad token.
        if let Some(token) = self.cached_token(action, previous).await {
            return Ok(token);
        }

        let user_id = self.inner.tokens.user_id().await?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_else(|_| "0".to_string());

        let sign = captcha_sign(&self.inner.client_id, &self.inner.device_id, &timestamp);

        let body = serde_json::json!({
            "action": action,
            "captcha_token": previous.unwrap_or(""),
            "client_id": self.inner.client_id,
            "device_id": self.inner.device_id,
            "meta": {
                "captcha_sign": sign,
                "user_id": user_id,
                "package_name": CLIENT_PACKAGE_NAME,
                "client_version": CLIENT_VERSION,
                "timestamp": timestamp,
            },
            // NOTE: this "ttps://" typo is verbatim from the pikpakcli source;
            // the PikPak server accepts it.
            "redirect_uri": "ttps://api.mypikpak.com/v1/auth/callback",
        });

        let access = self.inner.tokens.access_token().await?;
        let url = format!(
            "{}?client_id={}",
            self.inner.init_endpoint, self.inner.client_id,
        );

        tracing::debug!(action = %action, "initializing captcha token");
        let resp = self
            .inner
            .http
            .post(&url)
            .bearer_auth(&access)
            .header("X-Device-Id", &self.inner.device_id)
            .json(&body)
            .send()
            .await?;

        if !resp.status().is_success() {
            let status = resp.status().as_u16();
            let text = resp.text().await.unwrap_or_default();
            return Err(Error::Api {
                status,
                message: text,
            });
        }

        let parsed: CaptchaInitResponse = resp.json().await?;
        let cached = CachedCaptcha {
            token: parsed.captcha_token.clone(),
            refresh_after: Instant::now() + PROACTIVE_REFRESH,
        };
        self.inner
            .cache
            .write()
            .await
            .insert(action.to_string(), cached);
        Ok(parsed.captcha_token)
    }

    /// Return a usable cached token for `action`, if there is one.
    ///
    /// When `previous` is supplied, a cached entry equal to it counts as
    /// unusable: the caller is telling us that exact token just failed.
    async fn cached_token(&self, action: &str, previous: Option<&str>) -> Option<String> {
        let cache = self.inner.cache.read().await;
        let entry = cache.get(action)?;
        if Instant::now() >= entry.refresh_after {
            return None;
        }
        if previous.is_some_and(|prev| prev == entry.token) {
            return None;
        }
        Some(entry.token.clone())
    }
}

#[derive(Debug, Deserialize)]
struct CaptchaInitResponse {
    captcha_token: String,
    // expires_in, url, etc. — not used.
}

/// Compute the `captcha_sign` string used in the captcha init body.
///
/// Formula (from pikpakcli):
/// ```text
/// s0 = client_id + client_version + package_name + device_id + timestamp_ms
/// for salt in SALTS:
///     s_next = hex(md5(s_prev + salt))
/// result = "1." + s_final
/// ```
fn captcha_sign(client_id: &str, device_id: &str, timestamp: &str) -> String {
    let mut s = String::with_capacity(128);
    s.push_str(client_id);
    s.push_str(CLIENT_VERSION);
    s.push_str(CLIENT_PACKAGE_NAME);
    s.push_str(device_id);
    s.push_str(timestamp);

    for salt in SALTS {
        let mut hasher = Md5::new();
        hasher.update(s.as_bytes());
        hasher.update(salt.as_bytes());
        let digest = hasher.finalize();
        s = hex::encode(digest);
    }

    format!("1.{s}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sign_is_deterministic_and_prefixed() {
        let s1 = captcha_sign("id", "dev", "1700000000000");
        let s2 = captcha_sign("id", "dev", "1700000000000");
        assert_eq!(s1, s2, "identical inputs must produce identical signs");
        assert!(s1.starts_with("1."), "sign must have the '1.' prefix");
        // The body after the prefix is the final MD5 hex -> 32 chars.
        assert_eq!(s1.len(), 2 + 32);
    }

    #[test]
    fn sign_changes_on_any_input_change() {
        let base = captcha_sign("id", "dev", "1700000000000");
        assert_ne!(base, captcha_sign("ID", "dev", "1700000000000"));
        assert_ne!(base, captcha_sign("id", "DEV", "1700000000000"));
        assert_ne!(base, captcha_sign("id", "dev", "1700000000001"));
    }

    /// The action whose captcha init the test can hold in flight.
    const SLOW_ACTION: &str = "GET:/slow";

    /// Handle to the mock server's observable state.
    struct MockServer {
        addr: std::net::SocketAddr,
        /// Number of captcha-init requests received.
        init_requests: Arc<std::sync::atomic::AtomicUsize>,
        /// Notified once a `SLOW_ACTION` init request has arrived.
        slow_started: Arc<tokio::sync::Notify>,
        /// Releasing this lets the held `SLOW_ACTION` request finish.
        release_slow: Arc<tokio::sync::Notify>,
    }

    /// Mock auth + captcha-init server.
    ///
    /// A `SLOW_ACTION` init request is held until the test releases it, so the
    /// test can guarantee a refresh is in flight rather than racing a timer.
    async fn spawn_mock_server() -> MockServer {
        use std::sync::atomic::Ordering;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let init_requests = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let slow_started = Arc::new(tokio::sync::Notify::new());
        let release_slow = Arc::new(tokio::sync::Notify::new());

        tokio::spawn({
            let init_requests = init_requests.clone();
            let slow_started = slow_started.clone();
            let release_slow = release_slow.clone();
            async move {
                loop {
                    let Ok((mut sock, _)) = listener.accept().await else {
                        return;
                    };
                    let init_requests = init_requests.clone();
                    let slow_started = slow_started.clone();
                    let release_slow = release_slow.clone();
                    tokio::spawn(async move {
                        let request = crate::test_http::read_request(&mut sock).await;

                        if request.target.starts_with("/v1/auth/token") {
                            crate::test_http::respond_json(
                                &mut sock,
                                200,
                                "OK",
                                crate::test_http::TOKEN_JSON,
                            )
                            .await;
                            return;
                        }

                        init_requests.fetch_add(1, Ordering::SeqCst);
                        let action = serde_json::from_str::<serde_json::Value>(&request.body)
                            .ok()
                            .and_then(|v| {
                                v.get("action").and_then(|a| a.as_str()).map(str::to_string)
                            })
                            .unwrap_or_default();
                        if action == SLOW_ACTION {
                            slow_started.notify_one();
                            release_slow.notified().await;
                        }
                        crate::test_http::respond_json(
                            &mut sock,
                            200,
                            "OK",
                            &format!(r#"{{"captcha_token":"token-{action}"}}"#),
                        )
                        .await;
                    });
                }
            }
        });

        MockServer {
            addr,
            init_requests,
            slow_started,
            release_slow,
        }
    }

    fn manager_for(addr: std::net::SocketAddr) -> CaptchaManager {
        let http = reqwest::Client::new();
        let tokens = TokenManager::new(
            http.clone(),
            format!("http://{addr}/v1/auth/token"),
            crate::auth::OAuthCredentials::default(),
            "device-1",
            "initial-token",
        );
        CaptchaManager::new(
            http,
            format!("http://{addr}/v1/shield/captcha/init"),
            "client-1",
            "device-1",
            tokens,
        )
    }

    async fn cache_token(manager: &CaptchaManager, action: &str, token: &str) {
        manager.inner.cache.write().await.insert(
            action.to_string(),
            CachedCaptcha {
                token: token.to_string(),
                refresh_after: Instant::now() + PROACTIVE_REFRESH,
            },
        );
    }

    #[tokio::test]
    async fn refresh_in_flight_does_not_block_cache_reads_of_other_actions() {
        let server = spawn_mock_server().await;
        let manager = manager_for(server.addr);
        cache_token(&manager, "GET:/other", "cached-other").await;

        let slow = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.token_for(SLOW_ACTION).await })
        };
        tokio::time::timeout(Duration::from_secs(5), server.slow_started.notified())
            .await
            .expect("the slow refresh should reach the server");

        // The refresh is provably in flight here; a cached read for a different
        // action must still complete without waiting for it.
        let other =
            tokio::time::timeout(Duration::from_millis(300), manager.token_for("GET:/other")).await;
        server.release_slow.notify_one();

        assert_eq!(
            other
                .expect("a cached token must be readable while another action refreshes")
                .unwrap(),
            "cached-other"
        );
        assert_eq!(slow.await.unwrap().unwrap(), "token-GET:/slow");
    }

    #[tokio::test]
    async fn refreshes_of_different_actions_do_not_serialize() {
        let server = spawn_mock_server().await;
        let manager = manager_for(server.addr);

        let slow = {
            let manager = manager.clone();
            tokio::spawn(async move { manager.token_for(SLOW_ACTION).await })
        };
        tokio::time::timeout(Duration::from_secs(5), server.slow_started.notified())
            .await
            .expect("the slow refresh should reach the server");

        // A *miss* for a different action must also proceed in parallel.
        let other =
            tokio::time::timeout(Duration::from_millis(300), manager.token_for("GET:/other")).await;
        server.release_slow.notify_one();

        assert_eq!(
            other
                .expect("a different action must not queue behind an in-flight refresh")
                .unwrap(),
            "token-GET:/other"
        );
        assert_eq!(slow.await.unwrap().unwrap(), "token-GET:/slow");
    }

    #[tokio::test]
    async fn concurrent_refreshes_of_one_action_collapse_into_one_request() {
        use std::sync::atomic::Ordering;
        let server = spawn_mock_server().await;
        let manager = manager_for(server.addr);

        let mut tasks = Vec::new();
        for _ in 0..5 {
            let manager = manager.clone();
            tasks.push(tokio::spawn(async move {
                manager.token_for("GET:/shared").await
            }));
        }
        for task in tasks {
            assert_eq!(task.await.unwrap().unwrap(), "token-GET:/shared");
        }
        assert_eq!(
            server.init_requests.load(Ordering::SeqCst),
            1,
            "concurrent misses for one action must share a single init request"
        );
    }

    #[tokio::test]
    async fn a_failed_token_is_not_returned_from_the_cache() {
        let server = spawn_mock_server().await;
        let manager = manager_for(server.addr);
        // Deliberately not SLOW_ACTION: this test needs the init to complete.
        cache_token(&manager, "GET:/bad", "known-bad").await;

        // `previous` names the cached token, so it must be replaced rather than
        // handed back from the cache.
        let token = manager
            .refresh("GET:/bad", Some("known-bad"))
            .await
            .unwrap();
        assert_eq!(token, "token-GET:/bad");
        assert_eq!(
            server
                .init_requests
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }
}
