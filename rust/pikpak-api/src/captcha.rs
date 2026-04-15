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
use tokio::sync::RwLock;

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
            }),
        }
    }

    /// Return a captcha token for the given action (e.g. `"GET:/drive/v1/files"`).
    pub(crate) async fn token_for(&self, action: &str) -> Result<String> {
        {
            let cache = self.inner.cache.read().await;
            if let Some(c) = cache.get(action) {
                if Instant::now() < c.refresh_after {
                    return Ok(c.token.clone());
                }
            }
        }

        self.refresh(action, None).await
    }

    /// Force a fresh captcha token for the given action. Optionally pass the
    /// previous token so the server can invalidate it. Returns the new token.
    pub(crate) async fn refresh(
        &self,
        action: &str,
        previous: Option<&str>,
    ) -> Result<String> {
        let mut cache = self.inner.cache.write().await;

        // Double-check inside the write lock.
        if previous.is_none() {
            if let Some(c) = cache.get(action) {
                if Instant::now() < c.refresh_after {
                    return Ok(c.token.clone());
                }
            }
        }

        let user_id = self.inner.tokens.user_id().await?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_millis().to_string())
            .unwrap_or_else(|_| "0".to_string());

        let sign = captcha_sign(
            &self.inner.client_id,
            &self.inner.device_id,
            &timestamp,
        );

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
            self.inner.init_endpoint,
            self.inner.client_id,
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
            return Err(Error::Api { status, message: text });
        }

        let parsed: CaptchaInitResponse = resp.json().await?;
        let cached = CachedCaptcha {
            token: parsed.captcha_token.clone(),
            refresh_after: Instant::now() + PROACTIVE_REFRESH,
        };
        cache.insert(action.to_string(), cached);
        Ok(parsed.captcha_token)
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
}
