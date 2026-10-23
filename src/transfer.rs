//! Content transfers: fetching a file's bytes from the CDN link the drive API
//! hands out, resumably and with a retry policy of their own.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::client::{exponential_backoff, is_retryable_status, is_transient, jittered_backoff};
use crate::error::{Error, Result};
use crate::{Client, FileInfo};

/// One failed attempt, classified for retry: transient conditions are retried,
/// fatal ones abort immediately.
enum Attempt {
    Retry(Error),
    Fail(Error),
}

/// Maximum *consecutive non-progressing* attempts per file.
///
/// An attempt that transferred new bytes resets this counter, so a large file on
/// a flaky link survives interruptions as long as it keeps advancing, while a
/// server that fails without producing anything is abandoned quickly.
const MAX_STALLS: u32 = 5;

/// Absolute ceiling on attempts per file, progress or not.
///
/// Without it a pathological server that dribbles out one byte per attempt would
/// reset the stall counter forever and never terminate.
const MAX_TOTAL_ATTEMPTS: u32 = 100;

/// Retry policy for a single file: a stall budget plus an absolute ceiling.
struct RetryBudget {
    max_stalls: u32,
    max_attempts: u32,
    attempts: u32,
    stalls: u32,
}

impl RetryBudget {
    fn new() -> Self {
        Self {
            max_stalls: MAX_STALLS,
            max_attempts: MAX_TOTAL_ATTEMPTS,
            attempts: 0,
            stalls: 0,
        }
    }

    /// Record one *retryable* failed attempt, reporting whether the partial file
    /// grew since the previous attempt.
    ///
    /// Returns the backoff to wait before retrying, or `None` when the budget is
    /// exhausted and the download must fail.
    fn record_failure(&mut self, advanced: bool) -> Option<Duration> {
        self.attempts += 1;
        if advanced {
            self.stalls = 0;
        } else {
            self.stalls += 1;
        }
        if self.stalls >= self.max_stalls || self.attempts >= self.max_attempts {
            return None;
        }
        Some(download_backoff(self.stalls.saturating_sub(1)))
    }
}

/// Backoff for a download retry attempt (0-based): 500ms, 1s, 2s, ...
/// capped at 15s.
fn download_backoff(attempt: u32) -> Duration {
    exponential_backoff(Duration::from_millis(500), Duration::from_secs(15), attempt)
}

/// Current size of a partial file, or `0` if it does not exist yet.
async fn part_size(path: &Path) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Identity of the remote file a partial download belongs to.
///
/// A partial is only safe to resume while the remote file is demonstrably the
/// *same* file: appending the tail of a different version would splice two
/// files together into something that is neither. This identity is recorded next
/// to the partial file and compared before any `Range` request is made.
///
/// It cannot detect a re-upload that kept the id, the length *and* has no
/// modification time, but it does catch the realistic cases: a different entry,
/// a different length, or a changed modification time.
#[derive(Debug, PartialEq, Serialize, Deserialize)]
struct PartIdentity {
    id: String,
    size: u64,
    modified_time: Option<String>,
}

impl PartIdentity {
    /// Identity of `file`, whose expected transferred length is `size` (the
    /// length the completion check uses, so the two cannot disagree).
    fn of(file: &FileInfo, size: u64) -> Self {
        Self {
            id: file.id.clone(),
            size,
            modified_time: file.modified_time.clone(),
        }
    }
}

/// Sidecar path recording the identity of a partial file.
fn identity_path(part: &Path) -> PathBuf {
    let mut path = part.as_os_str().to_os_string();
    path.push(".meta");
    PathBuf::from(path)
}

/// Make the partial file safe to resume, and record which remote file it holds.
///
/// A partial that does not belong to `expected` — including one without any
/// sidecar — is discarded instead of resumed, because its existing bytes cannot
/// be trusted to belong to this file.
async fn prepare_part(part: &Path, expected: &PartIdentity) -> Result<()> {
    let meta_path = identity_path(part);
    if part_size(part).await > 0 {
        let recorded = tokio::fs::read(&meta_path)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PartIdentity>(&bytes).ok());
        if recorded.as_ref() != Some(expected) {
            tracing::debug!(
                part = %part.display(),
                "partial file does not belong to this remote file; restarting from scratch"
            );
            let _ = tokio::fs::remove_file(part).await;
            let _ = tokio::fs::remove_file(&meta_path).await;
        }
    }
    tokio::fs::write(&meta_path, serde_json::to_vec(expected)?)
        .await
        .map_err(io("failed to record partial download identity"))
}

impl Client {
    /// Download `file` to `dest` through the partial file `part`, resuming a
    /// matching partial and retrying while the transfer keeps making progress.
    ///
    /// `progress` is called with the bytes written so far and the expected
    /// total after every chunk. Returns the size of the file that landed.
    ///
    /// On failure the partial and its `.meta` identity sidecar are deliberately
    /// left in place: together they form a self-validating resume point, so a
    /// later call can continue instead of starting over. Both are gone once the
    /// file is renamed to `dest`.
    pub async fn download_file(
        &self,
        file: &FileInfo,
        dest: &Path,
        part: &Path,
        progress: &(dyn Fn(u64, u64) + Sync),
    ) -> Result<u64> {
        let dl = self.get_download_url(&file.id).await?;
        // An empty link would only fail later as an opaque request-builder
        // error, reported as a network blip worth retrying. It is the service
        // declining.
        if dl.web_content_link.is_empty() {
            return Err(Error::NoDownloadLink(file.name.clone()));
        }

        // The detail response may leave the size out; the directory listing
        // knows it. An absent field is not the same as a zero-byte file, which is
        // why the response models it as an `Option`.
        let expected_size = dl.size.unwrap_or(file.size);

        // Never resume bytes that might belong to a different version of the file.
        prepare_part(part, &PartIdentity::of(file, expected_size)).await?;

        // The download link is time-limited; refresh it on each retry.
        let mut link = dl.web_content_link;
        let mut budget = RetryBudget::new();
        let mut last_size = part_size(part).await;
        loop {
            let size_before = last_size;
            let e = match self.attempt(&link, expected_size, part, progress).await {
                Ok(()) => break,
                Err(Attempt::Fail(e)) => return Err(e),
                Err(Attempt::Retry(e)) => e,
            };
            last_size = part_size(part).await;
            let Some(base_delay) = budget.record_failure(last_size > size_before) else {
                return Err(e);
            };
            // Spread the wait so workers that failed together do not retry
            // together; the budget's own arithmetic stays deterministic.
            let delay = jittered_backoff(base_delay);
            tracing::warn!(
                "{}: attempt {} failed ({e}); retrying in {:.1}s",
                file.name,
                budget.attempts,
                delay.as_secs_f64()
            );
            tokio::time::sleep(delay).await;
            // The link is time-limited, so fetch a fresh one for the next try.
            match self.get_download_url(&file.id).await {
                Ok(fresh) if !fresh.web_content_link.is_empty() => link = fresh.web_content_link,
                // An empty link is no better than the one we have.
                Ok(_) => {}
                // A rejected account token cannot be repaired by retrying:
                // surface it now instead of spending the whole budget and then
                // blaming the CDN status that the stale link produced.
                Err(err @ (Error::Auth(_) | Error::TokenExpired)) => return Err(err),
                // Anything else is a blip in the API call rather than the link
                // itself; keep the current link and let the next attempt decide.
                Err(err) => tracing::debug!(error = %err, "could not refresh the download link"),
            }
        }

        // Windows refuses to rename onto an existing path, so re-downloading a
        // name that already exists would fail there; clear the previous copy.
        #[cfg(windows)]
        if tokio::fs::try_exists(dest).await.unwrap_or(false) {
            let _ = tokio::fs::remove_file(dest).await;
        }

        tokio::fs::rename(part, dest)
            .await
            .map_err(io("failed to finalize output file"))?;
        let _ = tokio::fs::remove_file(identity_path(part)).await;

        // Report what actually landed rather than what was advertised: a resumed
        // file can differ from the size the API claimed.
        Ok(tokio::fs::metadata(dest)
            .await
            .map(|meta| meta.len())
            .unwrap_or(expected_size))
    }

    /// A single download attempt. Resumes from an existing partial via an HTTP
    /// `Range` request when the server supports it, else restarts cleanly.
    async fn attempt(
        &self,
        link: &str,
        total: u64,
        part: &Path,
        progress: &(dyn Fn(u64, u64) + Sync),
    ) -> std::result::Result<(), Attempt> {
        let existing = part_size(part).await;

        let mut request = self.download_client().get(link);
        if existing > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
        }

        let mut resp = request.send().await.map_err(classify)?;
        let status = resp.status();

        let (mut out, mut downloaded) =
            if existing > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
                // The server honoured our Range request, but we must confirm it
                // started where we asked: appending at the wrong offset would
                // silently corrupt the file.
                let start = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(parse_range_start);
                if start != Some(existing) {
                    create_part(part).await?;
                    return Err(Attempt::Retry(Error::Protocol(
                        "server did not resume from the requested offset; restarting from scratch"
                            .to_string(),
                    )));
                }
                // Append to what we already have.
                let f = tokio::fs::OpenOptions::new()
                    .append(true)
                    .open(part)
                    .await
                    .map_err(fatal("failed to open partial file"))?;
                (f, existing)
            } else if existing > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
                // The part is at/past the full length. If it is exactly the full
                // length it is already complete; otherwise the part is stale
                // (bigger than a now-shorter remote file). We cannot stream this
                // 416 response — its body is an error page, not file bytes — so
                // truncate the part and let the retry loop restart with a plain
                // (no-Range) GET.
                if total > 0 && existing == total {
                    return Ok(());
                }
                create_part(part).await?;
                return Err(Attempt::Retry(Error::Protocol(
                    "partial file did not match server range; restarting from scratch".to_string(),
                )));
            } else if status.is_success() {
                // 200 OK (range ignored) or a fresh download: start from zero.
                (create_part(part).await?, 0)
            } else {
                let retry = is_retryable_status(status) || is_expired_link_status(status);
                let err = Error::Transfer {
                    status: status.as_u16(),
                    message: resp.text().await.unwrap_or_default(),
                };
                return Err(if retry {
                    Attempt::Retry(err)
                } else {
                    Attempt::Fail(err)
                });
            };

        loop {
            match resp.chunk().await {
                Ok(Some(chunk)) => {
                    out.write_all(&chunk)
                        .await
                        .map_err(fatal("failed to write output file"))?;
                    downloaded += chunk.len() as u64;
                    progress(downloaded, total);
                }
                Ok(None) => break,
                Err(e) => {
                    // Persist what we have so the retry can resume from here.
                    out.flush().await.ok();
                    return Err(classify(e));
                }
            }
        }

        out.flush()
            .await
            .map_err(fatal("failed to flush output file"))?;

        // A short transfer must never be finalized. Without this check a truncated
        // stream whose own framing is self-consistent (e.g. a response closed early
        // with no length information, or a length shorter than the API-reported
        // size) would be renamed to the final name and look like a success. Framing
        // violations such as a lying Content-Length already error out upstream.
        if total > 0 && downloaded < total {
            return Err(Attempt::Retry(Error::Incomplete {
                received: downloaded,
                expected: total,
            }));
        }
        Ok(())
    }
}

/// Parse the start offset from a `Content-Range: bytes <start>-<end>/<total>`
/// header. Returns `None` for a malformed value or the `bytes */<total>` form
/// used by 416 responses.
fn parse_range_start(value: &reqwest::header::HeaderValue) -> Option<u64> {
    let text = value.to_str().ok()?.trim();
    // `to_str` yields visible ASCII only, so byte 5 is always a character
    // boundary; `get` states that rather than leaning on the reader knowing it.
    let unit = text.get(..5)?;
    if !unit.eq_ignore_ascii_case("bytes") {
        return None;
    }
    let rest = text.get(5..)?;
    let rest = rest.trim_start_matches([' ', '=']);
    rest.split('-').next()?.trim().parse::<u64>().ok()
}

/// Whether a content-transfer status means the time-limited download link has
/// gone stale, so fetching a fresh link and retrying is worthwhile.
///
/// A CDN answering a signed URL carries no bearer token of ours, so a 401 here
/// is the link expiring, not the account token being rejected — the two differ
/// in what fixes them, and only the link can be refreshed.
fn is_expired_link_status(status: reqwest::StatusCode) -> bool {
    status == reqwest::StatusCode::FORBIDDEN
        || status == reqwest::StatusCode::UNAUTHORIZED
        || status == reqwest::StatusCode::GONE
}

async fn create_part(path: &Path) -> std::result::Result<tokio::fs::File, Attempt> {
    tokio::fs::File::create(path)
        .await
        .map_err(fatal("failed to create output file"))
}

/// An I/O failure, with what was being done when it happened.
fn io(context: &'static str) -> impl FnOnce(std::io::Error) -> Error {
    move |source| Error::Io { context, source }
}

/// An I/O failure that ends the attempt and the download with it.
fn fatal(context: &'static str) -> impl FnOnce(std::io::Error) -> Attempt {
    move |source| Attempt::Fail(io(context)(source))
}

/// Classify a transport failure as retryable (transient network) or fatal.
fn classify(err: reqwest::Error) -> Attempt {
    let transient = is_transient(&err);
    // The URL goes: its query string is a signed credential, and this text
    // reaches logs.
    let e = Error::Http(err.without_url());
    if transient {
        Attempt::Retry(e)
    } else {
        Attempt::Fail(e)
    }
}

#[cfg(test)]
mod tests;
