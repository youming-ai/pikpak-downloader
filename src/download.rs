//! Downloading: task collection, resumable transfers, retry policy.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path as StdPath, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use humansize::{format_size, BINARY};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;

use crate::DownloadArgs;
use pikpak::{
    exponential_backoff, is_drive_root, is_retryable_status, jittered_backoff, Client, FileKind,
};

/// One file to fetch, the directory it belongs in, and the local name it gets.
struct DownloadTask {
    file: pikpak::FileInfo,
    dir: PathBuf,
    /// Unique within `dir`; see [`NameClaims`].
    name: String,
}

/// A folder still to be walked, and where its contents belong.
struct FolderToWalk {
    id: String,
    name: String,
    dir: PathBuf,
}

/// Local names already handed out, per directory.
///
/// PikPak identifies entries by id, so one folder may hold several entries with
/// the same name — and names that differ only in case, or only in characters
/// Windows replaces, collide on the local filesystem too. Every entry claims its
/// local name here before anything is written; a taken name gets a ` (2)`,
/// ` (3)`, … suffix. Claims follow listing order, so a rerun assigns the same
/// names and partial files still resume.
struct NameClaims {
    taken: HashMap<PathBuf, HashSet<String>>,
    case_insensitive: bool,
}

impl NameClaims {
    /// Claims for this platform's default filesystem semantics: Windows and
    /// macOS fold case, so `A.txt` and `a.txt` are one file there.
    fn new() -> Self {
        Self::with_case_folding(cfg!(any(windows, target_os = "macos")))
    }

    fn with_case_folding(case_insensitive: bool) -> Self {
        Self {
            taken: HashMap::new(),
            case_insensitive,
        }
    }

    /// Claim a safe, unused name for the remote `raw` name inside `dir`.
    ///
    /// Files keep their extension after the suffix (`movie (2).mp4`); folders
    /// are suffixed as a whole.
    fn claim(&mut self, dir: &StdPath, raw: &str, is_file: bool) -> Result<String> {
        let base = safe_component(raw)?;
        let taken = self.taken.entry(dir.to_path_buf()).or_default();
        let key = |name: &str| {
            if self.case_insensitive {
                name.to_lowercase()
            } else {
                name.to_string()
            }
        };
        if taken.insert(key(&base)) {
            return Ok(base);
        }
        // A leading dot marks a hidden name, not an extension.
        let (stem, ext) = match base.rfind('.') {
            Some(dot) if is_file && dot > 0 => base.split_at(dot),
            _ => (base.as_str(), ""),
        };
        let mut n = 2u32;
        loop {
            let candidate = format!("{stem} ({n}){ext}");
            if taken.insert(key(&candidate)) {
                return Ok(candidate);
            }
            n += 1;
        }
    }
}

/// A remote entry classified by what downloading it means.
enum Downloadable {
    File(pikpak::FileInfo),
    Folder(pikpak::FileInfo),
    /// An entry whose kind this crate does not model: refused, not guessed at.
    Unsupported(String),
}

/// Classify a remote entry for downloading.
///
/// The single place that decides what an entry kind means, so the download
/// paths never repeat the cascade. (The similarly-named `classify` below sorts
/// *transport errors*; this one sorts remote entries.)
fn classify_entry(entry: pikpak::FileInfo) -> Downloadable {
    match entry.kind {
        FileKind::File => Downloadable::File(entry),
        FileKind::Folder => Downloadable::Folder(entry),
        FileKind::Unknown => Downloadable::Unsupported(entry.name),
    }
}

/// Queue `file` for download into `dir` under a freshly claimed name.
fn push_file(
    tasks: &mut Vec<DownloadTask>,
    claims: &mut NameClaims,
    file: pikpak::FileInfo,
    dir: &StdPath,
) -> Result<()> {
    // The listing name is what the user saw; an entry without one still gets
    // a stable, unique name instead of failing the whole run.
    let raw = if file.name.trim().is_empty() {
        file.id.clone()
    } else {
        file.name.clone()
    };
    let name = claims.claim(dir, &raw, true)?;
    tasks.push(DownloadTask {
        file,
        dir: dir.to_path_buf(),
        name,
    });
    Ok(())
}

pub(crate) async fn cmd_download(client: &Client, args: DownloadArgs) -> Result<()> {
    let output = StdPath::new(&args.output);
    tokio::fs::create_dir_all(output)
        .await
        .context("failed to create output directory")?;

    // Flatten into tasks, creating the directory tree up front so concurrent
    // downloads never race on mkdir.
    let mut tasks: Vec<DownloadTask> = Vec::new();
    let mut claims = NameClaims::new();

    if is_drive_root(&args.path) {
        // The virtual root has no entry of its own, so expand it into its
        // children instead of resolving a path. Files land directly in the
        // output directory; folders keep their own subdirectory.
        for child in client.list_folder("").await.context("list_folder failed")? {
            match classify_entry(child) {
                Downloadable::Folder(folder) => {
                    collect_folder(client, &folder, output, &mut tasks, &mut claims).await?
                }
                Downloadable::File(file) => push_file(&mut tasks, &mut claims, file, output)?,
                Downloadable::Unsupported(name) => bail!(
                    "refusing to download {name:?}: unsupported entry kind reported by the server"
                ),
            }
        }
    } else {
        let info = client.resolve_path_info(&args.path).await?;
        match classify_entry(info) {
            Downloadable::Folder(folder) => {
                collect_folder(client, &folder, output, &mut tasks, &mut claims).await?
            }
            Downloadable::File(file) => push_file(&mut tasks, &mut claims, file, output)?,
            Downloadable::Unsupported(name) => bail!(
                "refusing to download {name:?}: unsupported entry kind reported by the server"
            ),
        }
    }

    if tasks.is_empty() {
        println!("(nothing to download)");
        return Ok(());
    }

    let jobs = args.jobs.max(1);
    // Live byte-level progress only reads cleanly with a single active
    // transfer; with concurrency we fall back to per-file start/finish lines.
    let show_progress = jobs == 1;

    let sem = Arc::new(Semaphore::new(jobs));
    let mut set = tokio::task::JoinSet::new();
    for task in tasks {
        let client = client.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore is never closed");
            download_file(&client, &task.file, &task.dir, &task.name, show_progress).await
        });
    }

    let mut first_err: Option<anyhow::Error> = None;
    while let Some(joined) = set.join_next().await {
        match joined.context("download task panicked")? {
            Ok(()) => {}
            Err(e) => {
                eprintln!("error: {e:#}");
                if first_err.is_none() {
                    first_err = Some(e);
                }
            }
        }
    }

    match first_err {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

/// Reduce a server-provided name to a single safe path component,
/// preventing path traversal via absolute paths, `..`, or embedded
/// separators. Returns the basename, or an error if none remains.
///
/// On Windows the result is additionally adjusted for that platform's file-name
/// rules (reserved device names, trailing dots and spaces); elsewhere the remote
/// name is preserved verbatim.
fn safe_component(name: &str) -> Result<String> {
    match StdPath::new(name).file_name().and_then(|s| s.to_str()) {
        Some(base) => Ok(if cfg!(windows) {
            sanitize_for_windows(base)
        } else {
            base.to_string()
        }),
        None => bail!("refusing unsafe remote name: {name:?}"),
    }
}

/// Adjust `name` for Windows file-name rules.
///
/// Windows reserves `CON`, `PRN`, `AUX`, `NUL` and `COM1`-`COM9` / `LPT1`-`LPT9`
/// — with or without an extension — and silently strips trailing dots and
/// spaces, so such a name cannot be stored faithfully there. Reserved names get
/// a leading underscore; trailing dots and spaces are dropped.
///
/// Kept separate from [`safe_component`] so the rules can be tested on any
/// platform, while only being *applied* when actually running on Windows.
fn sanitize_for_windows(name: &str) -> String {
    let filtered: String = name
        .chars()
        .map(|c| match c {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '_',
            c if (c as u32) < 32 => '_',
            c => c,
        })
        .collect();
    let trimmed = filtered.trim_end_matches(['.', ' ']);
    let trimmed = if trimmed.is_empty() { "_" } else { trimmed };
    let stem = trimmed
        .split('.')
        .next()
        .unwrap_or(trimmed)
        .to_ascii_uppercase();
    let is_device = matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit());
    if is_device {
        format!("_{trimmed}")
    } else {
        trimmed.to_string()
    }
}

/// A download failure classified for retry: transient conditions are retried,
/// fatal ones abort immediately.
enum DlError {
    Retryable(anyhow::Error),
    Fatal(anyhow::Error),
}

impl DlError {
    fn into_inner(self) -> anyhow::Error {
        match self {
            DlError::Retryable(e) | DlError::Fatal(e) => e,
        }
    }
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

/// Current size of a partial file, or `0` if it does not exist yet.
async fn part_size(path: &StdPath) -> u64 {
    tokio::fs::metadata(path)
        .await
        .map(|m| m.len())
        .unwrap_or(0)
}

/// Identity of the remote file a partial download belongs to.
///
/// A `.part` file is only safe to resume while the remote file is demonstrably
/// the *same* file: appending the tail of a different version would splice two
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
    fn of(file: &pikpak::FileInfo, size: u64) -> Self {
        Self {
            id: file.id.clone(),
            size,
            modified_time: file.modified_time.clone(),
        }
    }
}

/// Sidecar path recording the identity of a partial file.
fn identity_path(part_path: &StdPath) -> PathBuf {
    let mut path = part_path.as_os_str().to_os_string();
    path.push(".meta");
    PathBuf::from(path)
}

/// Make the partial file safe to resume, and record which remote file it holds.
///
/// A partial that does not belong to `expected` — including one left by an older
/// version of this tool, which has no sidecar at all — is discarded instead of
/// resumed, because its existing bytes cannot be trusted to belong to this file.
async fn prepare_part(part_path: &StdPath, expected: &PartIdentity) -> Result<()> {
    let meta_path = identity_path(part_path);
    if part_size(part_path).await > 0 {
        let recorded = tokio::fs::read(&meta_path)
            .await
            .ok()
            .and_then(|bytes| serde_json::from_slice::<PartIdentity>(&bytes).ok());
        if recorded.as_ref() != Some(expected) {
            tracing::debug!(
                part = %part_path.display(),
                "partial file does not belong to this remote file; restarting from scratch"
            );
            let _ = tokio::fs::remove_file(part_path).await;
            let _ = tokio::fs::remove_file(&meta_path).await;
        }
    }
    tokio::fs::write(&meta_path, serde_json::to_vec(expected)?)
        .await
        .context("failed to record partial download identity")?;
    Ok(())
}

/// Download one file into `output_dir`, resuming a matching partial file and
/// retrying while the transfer keeps making progress.
///
/// On failure the `.part` file and its identity sidecar are deliberately left in
/// place: together they form a self-validating resume point, so a later run can
/// continue instead of starting over. Both are removed once the file is
/// finalized.
async fn download_file(
    client: &Client,
    file: &pikpak::FileInfo,
    output_dir: &StdPath,
    dest_name: &str,
    show_progress: bool,
) -> Result<()> {
    let dl: pikpak::DownloadInfo = client
        .get_download_url(&file.id)
        .await
        .context("failed to get download URL")?;

    let safe_dest_name = safe_component(dest_name)?;
    // The detail response may leave the size out; the directory listing we already
    // have knows it. An absent field is not the same as a zero-byte file, which is
    // why the response models it as an `Option`.
    let expected_size = dl.size.unwrap_or(file.size);

    let file_path = output_dir.join(&safe_dest_name);
    let mut part = file_path.clone().into_os_string();
    part.push(".part");
    let part_path = PathBuf::from(part);

    // Never resume bytes that might belong to a different version of the file.
    prepare_part(&part_path, &PartIdentity::of(file, expected_size)).await?;

    println!(
        "Downloading: {} ({})",
        safe_dest_name,
        format_size(expected_size, BINARY)
    );

    // The download link is time-limited; refresh it on each retry.
    let mut link = dl.web_content_link.clone();
    let mut budget = RetryBudget::new();
    let mut last_size = part_size(&part_path).await;
    loop {
        let size_before = last_size;
        match download_attempt(client, &link, expected_size, &part_path, show_progress).await {
            Ok(()) => break,
            Err(err) => {
                if !matches!(err, DlError::Retryable(_)) {
                    return Err(err.into_inner())
                        .with_context(|| format!("failed to download {}", safe_dest_name));
                }
                let e = err.into_inner();
                last_size = part_size(&part_path).await;
                let Some(base_delay) = budget.record_failure(last_size > size_before) else {
                    return Err(e)
                        .with_context(|| format!("failed to download {}", safe_dest_name));
                };
                // Spread the wait so workers that failed together do not retry
                // together; the budget's own arithmetic stays deterministic.
                let delay = jittered_backoff(base_delay);
                eprintln!(
                    "  {}: attempt {} failed ({e:#}); retrying in {:.1}s",
                    safe_dest_name,
                    budget.attempts,
                    delay.as_secs_f64()
                );
                tokio::time::sleep(delay).await;
                // The link is time-limited, so fetch a fresh one for the next try.
                match client.get_download_url(&file.id).await {
                    Ok(fresh) => link = fresh.web_content_link,
                    // A rejected account token cannot be repaired by retrying:
                    // surface it now instead of spending the whole budget and then
                    // blaming the CDN status that the stale link produced.
                    Err(err @ (pikpak::Error::Auth(_) | pikpak::Error::TokenExpired)) => {
                        return Err(err).with_context(|| {
                            format!("failed to refresh the download link for {safe_dest_name}")
                        })
                    }
                    // Anything else is a blip in the API call rather than the link
                    // itself; keep the current link and let the next attempt decide.
                    Err(err) => {
                        tracing::debug!(error = %err, "could not refresh the download link")
                    }
                }
            }
        }
    }

    // Windows refuses to rename onto an existing path, so re-downloading a name
    // that already exists would fail there; clear the previous copy first.
    #[cfg(windows)]
    if tokio::fs::try_exists(&file_path).await.unwrap_or(false) {
        let _ = tokio::fs::remove_file(&file_path).await;
    }

    tokio::fs::rename(&part_path, &file_path)
        .await
        .context("failed to finalize output file")?;
    let _ = tokio::fs::remove_file(identity_path(&part_path)).await;
    println!("Saved: {}", file_path.display());
    Ok(())
}

/// A single download attempt. Resumes from an existing `.part` file via an HTTP
/// `Range` request when the server supports it, else restarts cleanly.
async fn download_attempt(
    client: &Client,
    link: &str,
    total: u64,
    part_path: &StdPath,
    show_progress: bool,
) -> std::result::Result<(), DlError> {
    let existing = part_size(part_path).await;

    let mut request = client.download_client().get(link);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }

    let mut resp = request
        .send()
        .await
        .map_err(|e| classify(e, "download request failed"))?;
    let status = resp.status();

    let (mut out, mut downloaded) =
        if existing > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT {
            // The server honoured our Range request, but we must confirm it started
            // where we asked: appending at the wrong offset would silently corrupt
            // the file.
            let start = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(parse_range_start);
            if start != Some(existing) {
                create_part(part_path).await?;
                return Err(DlError::Retryable(anyhow::anyhow!(
                    "server did not resume from the requested offset; restarting from scratch"
                )));
            }
            // Append to what we already have.
            let f = tokio::fs::OpenOptions::new()
                .append(true)
                .open(part_path)
                .await
                .map_err(|e| fatal(e, "failed to open partial file"))?;
            (f, existing)
        } else if existing > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
            // The part is at/past the full length. If it is exactly the full length
            // it is already complete; otherwise the part is stale/corrupt (bigger
            // than a now-shorter remote file). We cannot stream this 416 response —
            // its body is an error page, not file bytes — so truncate the part and
            // let the outer retry loop restart with a plain (no-Range) GET.
            if total > 0 && existing == total {
                return Ok(());
            }
            create_part(part_path).await?;
            return Err(DlError::Retryable(anyhow::anyhow!(
                "partial file did not match server range; restarting from scratch"
            )));
        } else if status.is_success() {
            // 200 OK (range ignored) or a fresh download: start from zero.
            (create_part(part_path).await?, 0)
        } else {
            let retry = is_retryable_status(status) || is_expired_link_status(status);
            let body = resp.text().await.unwrap_or_default();
            let err = anyhow::anyhow!("download failed with status {}: {body}", status.as_u16());
            return Err(if retry {
                DlError::Retryable(err)
            } else {
                DlError::Fatal(err)
            });
        };

    loop {
        match resp.chunk().await {
            Ok(Some(chunk)) => {
                out.write_all(&chunk)
                    .await
                    .map_err(|e| fatal(e, "failed to write output file"))?;
                downloaded += chunk.len() as u64;
                if show_progress && total > 0 {
                    let pct = (downloaded as f64 / total as f64) * 100.0;
                    eprint!(
                        "\r  {} / {} ({:.1}%)",
                        format_size(downloaded, BINARY),
                        format_size(total, BINARY),
                        pct
                    );
                }
            }
            Ok(None) => break,
            Err(e) => {
                if show_progress {
                    eprintln!();
                }
                // Persist what we have so the retry can resume from here.
                out.flush().await.ok();
                return Err(classify(e, "download stream interrupted"));
            }
        }
    }

    if show_progress && total > 0 {
        eprintln!();
    }
    out.flush()
        .await
        .map_err(|e| fatal(e, "failed to flush output file"))?;

    // A short transfer must never be finalized. Without this check a truncated
    // stream whose own framing is self-consistent (e.g. a response closed early
    // with no length information, or a length shorter than the API-reported
    // size) would be renamed to the final name and look like a success. Framing
    // violations such as a lying Content-Length already error out upstream.
    if total > 0 && downloaded < total {
        return Err(DlError::Retryable(anyhow::anyhow!(
            "incomplete transfer: received {downloaded} of {total} bytes"
        )));
    }
    Ok(())
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

async fn create_part(path: &StdPath) -> std::result::Result<tokio::fs::File, DlError> {
    tokio::fs::File::create(path)
        .await
        .map_err(|e| fatal(e, "failed to create output file"))
}

/// Wrap an I/O error as a fatal download error with context.
fn fatal(err: impl std::error::Error + Send + Sync + 'static, ctx: &'static str) -> DlError {
    DlError::Fatal(anyhow::Error::new(err).context(ctx))
}

/// Classify a reqwest error as retryable (transient network) or fatal.
fn classify(err: reqwest::Error, ctx: &'static str) -> DlError {
    let transient = err.is_timeout() || err.is_connect() || err.is_request() || err.is_body();
    let e = anyhow::Error::new(err).context(ctx);
    if transient {
        DlError::Retryable(e)
    } else {
        DlError::Fatal(e)
    }
}

/// Backoff for a download retry attempt (0-based): 500ms, 1s, 2s, ...
/// capped at 15s.
fn download_backoff(attempt: u32) -> Duration {
    exponential_backoff(Duration::from_millis(500), Duration::from_secs(15), attempt)
}

/// Walk a remote folder tree, creating local directories and collecting each
/// file with its destination directory into `tasks`.
///
/// The walk is breadth-first over an explicit queue rather than recursive, so a
/// deeply nested tree is bounded by memory instead of by the call stack. Note
/// that the whole tree is still enumerated *before* any transfer starts, which
/// keeps directory creation ahead of the concurrent downloads (they never race
/// on `mkdir`) at the cost of delaying the first byte on very large trees.
async fn collect_folder(
    client: &Client,
    folder: &pikpak::FileInfo,
    output_dir: &StdPath,
    tasks: &mut Vec<DownloadTask>,
    claims: &mut NameClaims,
) -> Result<()> {
    let raw_name = if folder.name.trim().is_empty() {
        &folder.id
    } else {
        &folder.name
    };
    let root_name = claims.claim(output_dir, raw_name, false)?;
    let root_dir = output_dir.join(root_name);
    let mut queue = VecDeque::from([FolderToWalk {
        id: folder.id.clone(),
        name: folder.name.clone(),
        dir: root_dir,
    }]);

    while let Some(current) = queue.pop_front() {
        tokio::fs::create_dir_all(&current.dir)
            .await
            .context("failed to create folder")?;

        println!("Scanning folder: {}", current.name);

        for entry in client.list_folder(&current.id).await? {
            match classify_entry(entry) {
                Downloadable::Folder(child) => {
                    let raw = if child.name.trim().is_empty() {
                        &child.id
                    } else {
                        &child.name
                    };
                    let child_name = claims.claim(&current.dir, raw, false)?;
                    queue.push_back(FolderToWalk {
                        dir: current.dir.join(child_name),
                        id: child.id,
                        name: child.name,
                    });
                }
                Downloadable::File(file) => {
                    push_file(tasks, claims, file, &current.dir)?;
                }
                Downloadable::Unsupported(name) => bail!(
                    "refusing to download {name:?}: unsupported entry kind reported by the server"
                ),
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::download_backoff;
    use super::safe_component;
    use std::sync::Arc;

    #[test]
    fn keeps_plain_names() {
        assert_eq!(safe_component("video.mp4").unwrap(), "video.mp4");
        assert_eq!(safe_component("My Pack").unwrap(), "My Pack");
    }

    #[test]
    fn strips_directory_traversal() {
        assert_eq!(safe_component("../../etc/passwd").unwrap(), "passwd");
        assert_eq!(safe_component("a/b/c.txt").unwrap(), "c.txt");
    }

    #[test]
    fn rejects_absolute_paths() {
        assert_eq!(safe_component("/etc/cron.d/x").unwrap(), "x");
    }

    #[test]
    fn rejects_names_without_a_basename() {
        assert!(safe_component("").is_err());
        assert!(safe_component("..").is_err());
        assert!(safe_component(".").is_err());
        assert!(safe_component("/").is_err());
    }

    #[test]
    fn windows_sanitisation_handles_reserved_names_and_trailing_dots() {
        // Ordinary names are untouched.
        assert_eq!(super::sanitize_for_windows("report.txt"), "report.txt");
        // Illegal filesystem characters are replaced with underscores.
        assert_eq!(
            super::sanitize_for_windows("Season 1: Ep? <HD>*|\"2024\".mp4"),
            "Season 1_ Ep_ _HD____2024_.mp4"
        );
        // Reserved device names, with or without an extension.
        assert_eq!(super::sanitize_for_windows("CON"), "_CON");
        assert_eq!(super::sanitize_for_windows("nul.txt"), "_nul.txt");
        assert_eq!(super::sanitize_for_windows("com1"), "_com1");
        assert_eq!(super::sanitize_for_windows("LPT9.log"), "_LPT9.log");
        // Only single digits are device names.
        assert_eq!(super::sanitize_for_windows("com10"), "com10");
        assert_eq!(super::sanitize_for_windows("comx"), "comx");
        // Windows strips trailing dots and spaces.
        assert_eq!(super::sanitize_for_windows("trailing..."), "trailing");
        assert_eq!(super::sanitize_for_windows("trailing "), "trailing");
        assert_eq!(super::sanitize_for_windows("..."), "_");
    }

    #[test]
    fn download_backoff_grows_and_caps() {
        assert!(download_backoff(0) < download_backoff(2));
        assert!(download_backoff(30) <= std::time::Duration::from_millis(15_000));
    }

    #[test]
    fn name_claims_disambiguates_files_and_preserves_extensions() {
        let dir = std::path::Path::new("/tmp/test");
        let mut claims = super::NameClaims::with_case_folding(false);

        // First occurrence gets base name.
        assert_eq!(claims.claim(dir, "video.mp4", true).unwrap(), "video.mp4");
        // Second and third occurrences get " (2)", " (3)" before extension.
        assert_eq!(
            claims.claim(dir, "video.mp4", true).unwrap(),
            "video (2).mp4"
        );
        assert_eq!(
            claims.claim(dir, "video.mp4", true).unwrap(),
            "video (3).mp4"
        );

        // Hidden files (leading dot) do not treat the prefix as stem.
        assert_eq!(claims.claim(dir, ".gitignore", true).unwrap(), ".gitignore");
        assert_eq!(
            claims.claim(dir, ".gitignore", true).unwrap(),
            ".gitignore (2)"
        );

        // Files without extension.
        assert_eq!(claims.claim(dir, "README", true).unwrap(), "README");
        assert_eq!(claims.claim(dir, "README", true).unwrap(), "README (2)");

        // Folders are suffixed as a whole without treating dot as extension.
        assert_eq!(
            claims.claim(dir, "folder.bundle", false).unwrap(),
            "folder.bundle"
        );
        assert_eq!(
            claims.claim(dir, "folder.bundle", false).unwrap(),
            "folder.bundle (2)"
        );

        // Different directories do not collide.
        let other_dir = std::path::Path::new("/tmp/other");
        assert_eq!(
            claims.claim(other_dir, "video.mp4", true).unwrap(),
            "video.mp4"
        );
    }

    #[test]
    fn name_claims_folds_case_when_configured() {
        let dir = std::path::Path::new("/tmp/test");
        let mut claims = super::NameClaims::with_case_folding(true);

        assert_eq!(claims.claim(dir, "file.txt", true).unwrap(), "file.txt");
        // "FILE.txt" collides under case folding, gets disambiguated.
        assert_eq!(claims.claim(dir, "FILE.txt", true).unwrap(), "FILE (2).txt");
    }

    async fn spawn_range_server(full: Vec<u8>) -> std::net::SocketAddr {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let (mut sock, _) = match listener.accept().await {
                    Ok(v) => v,
                    Err(_) => return,
                };
                let full = full.clone();
                tokio::spawn(async move {
                    let req = read_request(&mut sock).await;
                    let start = req
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase()
                                .strip_prefix("range: bytes=")
                                .map(|v| v.trim().trim_end_matches('-').to_string())
                        })
                        .and_then(|s| s.parse::<usize>().ok());

                    match start {
                        Some(s) if s >= full.len() => {
                            let body = b"range not satisfiable";
                            let hdr = format!(
                                "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                body.len()
                            );
                            let _ = sock.write_all(hdr.as_bytes()).await;
                            let _ = sock.write_all(body).await;
                        }
                        Some(s) => {
                            let body = &full[s..];
                            let hdr = format!(
                                "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {}-{}/{}\r\nConnection: close\r\n\r\n",
                                body.len(),
                                s,
                                full.len() - 1,
                                full.len()
                            );
                            let _ = sock.write_all(hdr.as_bytes()).await;
                            let _ = sock.write_all(body).await;
                        }
                        None => {
                            let hdr = format!(
                                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                                full.len()
                            );
                            let _ = sock.write_all(hdr.as_bytes()).await;
                            let _ = sock.write_all(&full).await;
                        }
                    }
                    let _ = sock.flush().await;
                });
            }
        });
        addr
    }

    #[tokio::test]
    async fn download_attempt_resumes_via_range() {
        let full: Vec<u8> = (0u8..32).collect();
        let addr = spawn_range_server(full.clone()).await;
        let client = pikpak::Client::builder()
            .refresh_token("dummy")
            .build()
            .unwrap();

        let dir = std::env::temp_dir().join(format!("pikpak-resume-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");
        std::fs::write(&part, &full[..10]).unwrap();

        let url = format!("http://{addr}/f");
        let res = super::download_attempt(&client, &url, full.len() as u64, &part, false).await;
        assert!(res.is_ok(), "resume attempt should succeed");

        let got = std::fs::read(&part).unwrap();
        assert_eq!(got, full, "resumed file must equal the full content");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_attempt_restarts_on_416() {
        // Remote file is 4 bytes; the local part holds 6 stale bytes, so the
        // server answers 416. total is claimed larger than the part so we hit
        // the restart branch rather than the "already complete" shortcut.
        let full: Vec<u8> = vec![1, 2, 3, 4];
        let addr = spawn_range_server(full.clone()).await;
        let client = pikpak::Client::builder()
            .refresh_token("dummy")
            .build()
            .unwrap();

        let dir = std::env::temp_dir().join(format!("pikpak-416-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");
        std::fs::write(&part, [9u8; 6]).unwrap();

        let url = format!("http://{addr}/f");
        let res = super::download_attempt(&client, &url, 16, &part, false).await;
        assert!(
            matches!(res, Err(super::DlError::Retryable(_))),
            "416 with a shorter remote file must return a retryable restart"
        );
        // The stale part must be truncated so the next attempt starts clean and
        // never streams the 416 error body into the output file.
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Server that writes `body` in `chunk`-sized pieces spaced by `gap`, and
    /// either closes normally or — when `stall_at` is `Some(n)` — goes silent
    /// once `n` bytes have been sent.
    async fn spawn_paced_server(
        body: Vec<u8>,
        chunk: usize,
        gap: std::time::Duration,
        stall_at: Option<usize>,
    ) -> std::net::SocketAddr {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let _ = read_request(&mut sock).await;
            let hdr = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = sock.write_all(hdr.as_bytes()).await;
            let mut sent = 0;
            while sent < body.len() {
                if stall_at == Some(sent) {
                    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                    return;
                }
                let end = (sent + chunk).min(body.len());
                if sock.write_all(&body[sent..end]).await.is_err() {
                    return;
                }
                let _ = sock.flush().await;
                sent = end;
                tokio::time::sleep(gap).await;
            }
        });
        addr
    }

    #[tokio::test]
    async fn download_survives_transfer_longer_than_timeout() {
        // 64 bytes at 8 bytes / 60ms takes ~480ms — far longer than the 100ms
        // timeout configured below. With a total deadline the body is cut off
        // mid-transfer; a per-read stall timeout must let it finish.
        let body: Vec<u8> = (0u8..64).collect();
        let addr =
            spawn_paced_server(body.clone(), 8, std::time::Duration::from_millis(60), None).await;
        let client = pikpak::Client::builder()
            .refresh_token("dummy")
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap();

        let dir = std::env::temp_dir().join(format!("pikpak-paced-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");
        let url = format!("http://{addr}/f");

        let res = super::download_attempt(&client, &url, 64, &part, false).await;
        assert!(
            res.is_ok(),
            "a slow but progressing download must not be killed by the timeout"
        );
        assert_eq!(std::fs::read(&part).unwrap(), body);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_aborts_when_stream_stalls() {
        // Headers plus the first 8 bytes arrive, then the server goes silent.
        // The stall timeout must still fire so a hung transfer is retried.
        let body: Vec<u8> = (0u8..64).collect();
        let addr = spawn_paced_server(body, 8, std::time::Duration::from_millis(10), Some(8)).await;
        let client = pikpak::Client::builder()
            .refresh_token("dummy")
            .timeout(std::time::Duration::from_millis(100))
            .build()
            .unwrap();

        let dir = std::env::temp_dir().join(format!("pikpak-stall-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");
        let url = format!("http://{addr}/f");

        let res = super::download_attempt(&client, &url, 64, &part, false).await;
        assert!(
            matches!(res, Err(super::DlError::Retryable(_))),
            "a stalled stream must abort with a retryable error"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn parse_range_start_reads_offsets() {
        use super::parse_range_start;
        let hv = |s: &str| reqwest::header::HeaderValue::from_str(s).unwrap();
        assert_eq!(parse_range_start(&hv("bytes 10-19/100")), Some(10));
        assert_eq!(parse_range_start(&hv("Bytes 10-19/100")), Some(10));
        assert_eq!(parse_range_start(&hv("BYTES 10-19/100")), Some(10));
        assert_eq!(parse_range_start(&hv("bytes=0-19/100")), Some(0));
        assert_eq!(parse_range_start(&hv("  Bytes 7-7/8  ")), Some(7));
        // 416-style and malformed values yield no usable offset.
        assert_eq!(parse_range_start(&hv("bytes */100")), None);
        assert_eq!(parse_range_start(&hv("nonsense")), None);
    }

    #[test]
    fn only_a_stale_link_is_treated_as_an_expired_link() {
        use reqwest::StatusCode;
        assert!(super::is_expired_link_status(StatusCode::UNAUTHORIZED));
        assert!(super::is_expired_link_status(StatusCode::FORBIDDEN));
        assert!(super::is_expired_link_status(StatusCode::GONE));
        assert!(!super::is_expired_link_status(StatusCode::NOT_FOUND));
        assert!(!super::is_expired_link_status(
            StatusCode::TOO_MANY_REQUESTS
        ));
        assert!(!super::is_expired_link_status(
            StatusCode::INTERNAL_SERVER_ERROR
        ));
        assert!(!super::is_expired_link_status(StatusCode::OK));
    }

    /// Server that answers every request with a fixed raw response.
    async fn spawn_raw_server(response: Vec<u8>) -> std::net::SocketAddr {
        use tokio::io::AsyncWriteExt;
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let response = response.clone();
                tokio::spawn(async move {
                    let _ = read_request(&mut sock).await;
                    let _ = sock.write_all(&response).await;
                    let _ = sock.flush().await;
                });
            }
        });
        addr
    }

    fn test_client() -> pikpak::Client {
        pikpak::Client::builder()
            .refresh_token("dummy")
            .build()
            .unwrap()
    }

    #[tokio::test]
    async fn download_attempt_restarts_when_part_is_oversized() {
        // Remote file is 4 bytes but the stale part holds 6. The 416 handler
        // must discard the part rather than declare the download complete:
        // returning Ok here would rename a 6-byte file over a 4-byte original.
        let full: Vec<u8> = vec![1, 2, 3, 4];
        let addr = spawn_range_server(full.clone()).await;
        let client = test_client();

        let dir = std::env::temp_dir().join(format!("pikpak-oversize-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");
        std::fs::write(&part, [9u8; 6]).unwrap();

        let url = format!("http://{addr}/f");
        let res = super::download_attempt(&client, &url, 4, &part, false).await;
        assert!(
            matches!(res, Err(super::DlError::Retryable(_))),
            "an oversized stale part must restart, not be treated as complete"
        );
        assert_eq!(std::fs::metadata(&part).unwrap().len(), 0);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_attempt_rejects_short_body() {
        // The server closes cleanly after 16 bytes with no framing that could
        // reveal the truncation (no Content-Length, no chunked encoding), while
        // the API reported 64 bytes. This must not be finalized.
        let mut response = b"HTTP/1.1 200 OK\r\nConnection: close\r\n\r\n".to_vec();
        response.extend_from_slice(&[1u8; 16]);
        let addr = spawn_raw_server(response).await;
        let client = test_client();

        let dir = std::env::temp_dir().join(format!("pikpak-short-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");

        let url = format!("http://{addr}/f");
        let res = super::download_attempt(&client, &url, 64, &part, false).await;
        assert!(
            matches!(res, Err(super::DlError::Retryable(_))),
            "a body shorter than the reported size must not count as complete"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_attempt_rejects_offset_mismatch() {
        // We ask to resume at 10, but the server answers 206 starting at 0.
        // Appending would corrupt the file, so the attempt must restart.
        let mut response = b"HTTP/1.1 206 Partial Content\r\nContent-Range: bytes 0-31/32\r\nContent-Length: 32\r\nConnection: close\r\n\r\n".to_vec();
        response.extend_from_slice(&[2u8; 32]);
        let addr = spawn_raw_server(response).await;
        let client = test_client();

        let dir = std::env::temp_dir().join(format!("pikpak-offset-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let part = dir.join("f.part");
        std::fs::write(&part, [1u8; 10]).unwrap();

        let url = format!("http://{addr}/f");
        let res = super::download_attempt(&client, &url, 32, &part, false).await;
        assert!(
            matches!(res, Err(super::DlError::Retryable(_))),
            "a 206 from the wrong offset must not be appended"
        );
        assert_eq!(
            std::fs::metadata(&part).unwrap().len(),
            0,
            "the mismatched part must be discarded"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Read one HTTP request (headers plus the `Content-Length` body) as text.
    ///
    /// A bin-local twin of the library's `test_http` helper: that one is
    /// `#[cfg(test)]`-gated and so invisible from the binary's tests. The
    /// content mocks parse the `Range` header out of the raw text.
    async fn read_request(sock: &mut tokio::net::TcpStream) -> String {
        use tokio::io::AsyncReadExt;
        let mut raw = Vec::new();
        let mut buf = [0u8; 4096];
        loop {
            match sock.read(&mut buf).await {
                Ok(0) => break,
                Ok(n) => {
                    raw.extend_from_slice(&buf[..n]);
                    if let Some(split) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                        let head = String::from_utf8_lossy(&raw[..split]).to_ascii_lowercase();
                        let declared = head
                            .lines()
                            .find_map(|line| {
                                line.strip_prefix("content-length:")
                                    .map(|value| value.trim().to_string())
                            })
                            .and_then(|value| value.parse::<usize>().ok())
                            .unwrap_or(0);
                        if raw.len() >= split + 4 + declared {
                            break;
                        }
                    }
                }
                Err(_) => break,
            }
        }
        String::from_utf8_lossy(&raw).into_owned()
    }

    /// Write a raw HTTP response and close.
    async fn send_response(
        sock: &mut tokio::net::TcpStream,
        status: &str,
        extra_headers: &[(&str, String)],
        body: &[u8],
    ) {
        use tokio::io::AsyncWriteExt;
        let mut head = format!("HTTP/1.1 {status}\r\n");
        for (name, value) in extra_headers {
            head.push_str(&format!("{name}: {value}\r\n"));
        }
        head.push_str("Connection: close\r\n\r\n");
        let _ = sock.write_all(head.as_bytes()).await;
        let _ = sock.write_all(body).await;
    }

    /// A mock PikPak stack on one port: auth, captcha init, the drive file
    /// detail endpoint, and the content URL it hands out.
    ///
    /// Content is served in `chunk`-sized pieces, each response ending cleanly
    /// without a `Content-Length` — an interrupted transfer whose framing hides
    /// nothing, which is what a flaky link looks like to the client.
    async fn spawn_mock_stack(
        content: Vec<u8>,
        chunk: usize,
        content_hits: Arc<std::sync::atomic::AtomicUsize>,
    ) -> std::net::SocketAddr {
        use std::sync::atomic::Ordering;

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let content = content.clone();
                let hits = content_hits.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut sock).await;
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

                    if path.starts_with("/v1/auth/token") {
                        let body = r#"{"access_token":"access","refresh_token":"rotated","expires_in":7200,"sub":"user"}"#;
                        let headers = [("Content-Length", body.len().to_string())];
                        send_response(&mut sock, "200 OK", &headers, body.as_bytes()).await;
                    } else if path.starts_with("/v1/shield/captcha/init") {
                        let body = r#"{"captcha_token":"captcha"}"#;
                        let headers = [("Content-Length", body.len().to_string())];
                        send_response(&mut sock, "200 OK", &headers, body.as_bytes()).await;
                    } else if path.starts_with("/drive/v1/files?") {
                        // A three-level tree: root -> dir -> sub -> big.bin. That
                        // exercises queueing a nested folder, not just the top
                        // level, without ever recursing forever.
                        let parent = path
                            .split("parent_id=")
                            .nth(1)
                            .map(|rest| rest.split('&').next().unwrap_or("").to_string())
                            .unwrap_or_default();
                        let body = if parent.is_empty() {
                            r#"{"files":[{"id":"A","name":"dir","kind":"drive#folder"}]}"#
                                .to_string()
                        } else if parent == "A" {
                            r#"{"files":[{"id":"B","name":"sub","kind":"drive#folder"}]}"#
                                .to_string()
                        } else if parent == "B" {
                            format!(
                                r#"{{"files":[{{"id":"f1","name":"big.bin","size":"{}","kind":"drive#file"}}]}}"#,
                                content.len()
                            )
                        } else if parent == "U" {
                            // An entry whose kind this crate does not model.
                            r#"{"files":[{"id":"u1","name":"weird.link","kind":"drive#shortcut"}]}"#
                                .to_string()
                        } else {
                            r#"{"files":[]}"#.to_string()
                        };
                        let headers = [("Content-Length", body.len().to_string())];
                        send_response(&mut sock, "200 OK", &headers, body.as_bytes()).await;
                    } else if path.starts_with("/drive/v1/files/") {
                        let body = format!(
                            r#"{{"name":"big.bin","size":"{}","web_content_link":"http://{addr}/content/big.bin"}}"#,
                            content.len()
                        );
                        let headers = [("Content-Length", body.len().to_string())];
                        send_response(&mut sock, "200 OK", &headers, body.as_bytes()).await;
                    } else {
                        hits.fetch_add(1, Ordering::SeqCst);
                        let start = request
                            .split("\r\n")
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("range: bytes=")
                                    .map(|v| v.trim().trim_end_matches('-').to_string())
                            })
                            .and_then(|s| s.parse::<usize>().ok())
                            .unwrap_or(0);
                        let start = start.min(content.len());
                        let end = (start + chunk).min(content.len());
                        // Self-consistent partial response: correct Content-Range
                        // start, no Content-Length, then EOF.
                        let headers = [(
                            "Content-Range",
                            format!(
                                "bytes {start}-{}/{len}",
                                end.max(start + 1) - 1,
                                len = content.len()
                            ),
                        )];
                        send_response(
                            &mut sock,
                            "206 Partial Content",
                            &headers,
                            &content[start..end],
                        )
                        .await;
                    }
                });
            }
        });
        addr
    }

    fn file_info(id: &str, name: &str, size: u64) -> pikpak::FileInfo {
        pikpak::FileInfo {
            id: id.to_string(),
            name: name.to_string(),
            size,
            kind: pikpak::FileKind::File,
            parent_id: None,
            mime_type: None,
            created_time: None,
            modified_time: None,
            file_extension: None,
        }
    }

    /// A client whose API and auth endpoints both point at the mock stack.
    fn mock_client(addr: std::net::SocketAddr) -> pikpak::Client {
        pikpak::Client::builder()
            .refresh_token("initial")
            .auth_base_url(format!("http://{addr}"))
            .api_base_url(format!("http://{addr}"))
            .build()
            .unwrap()
    }

    fn budget(max_stalls: u32, max_attempts: u32) -> super::RetryBudget {
        super::RetryBudget {
            max_stalls,
            max_attempts,
            attempts: 0,
            stalls: 0,
        }
    }

    #[test]
    fn retry_budget_stops_after_consecutive_stalls() {
        let mut b = budget(3, 100);
        assert!(b.record_failure(false).is_some());
        assert!(b.record_failure(false).is_some());
        assert!(
            b.record_failure(false).is_none(),
            "the third consecutive stall exhausts a budget of three"
        );
    }

    #[test]
    fn retry_budget_progress_resets_the_stall_count() {
        let mut b = budget(3, 100);
        assert!(b.record_failure(false).is_some());
        assert!(b.record_failure(false).is_some());
        assert!(
            b.record_failure(true).is_some(),
            "transferring bytes must clear the stall count"
        );
        assert!(b.record_failure(false).is_some());
        assert!(b.record_failure(false).is_some());
        assert!(b.record_failure(false).is_none());
    }

    #[test]
    fn retry_budget_has_an_absolute_ceiling() {
        // Progress alone must not license infinite retries.
        let mut b = budget(3, 50);
        for _ in 0..49 {
            assert!(b.record_failure(true).is_some());
        }
        assert!(
            b.record_failure(true).is_none(),
            "the absolute attempt ceiling still applies to progressing downloads"
        );
    }

    #[test]
    fn retry_budget_backoff_grows_with_stalls() {
        let mut b = budget(5, 100);
        let first = b.record_failure(false).unwrap();
        let second = b.record_failure(false).unwrap();
        assert!(second > first, "backoff must grow while the stream stalls");
    }

    #[test]
    fn identity_sidecar_sits_next_to_the_partial() {
        assert_eq!(
            super::identity_path(std::path::Path::new("/tmp/x/big.bin.part")),
            std::path::PathBuf::from("/tmp/x/big.bin.part.meta")
        );
    }

    #[test]
    fn part_identity_tracks_id_size_and_mtime() {
        let mut f = file_info("id-1", "big.bin", 10);
        f.modified_time = Some("2024-01-01T00:00:00Z".to_string());
        let identity = super::PartIdentity::of(&f, 42);
        assert_eq!(identity.id, "id-1");
        assert_eq!(identity.size, 42);
        assert_eq!(
            identity.modified_time.as_deref(),
            Some("2024-01-01T00:00:00Z")
        );
    }

    #[tokio::test]
    async fn download_resumes_a_partial_that_belongs_to_the_remote_file() {
        // 32 of 224 bytes already on disk, delivered 64 bytes at a time from the
        // offset: three requests instead of the four a fresh download needs.
        let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content.clone(), 64, hits.clone()).await;
        let client = mock_client(addr);
        let dir = std::env::temp_dir().join(format!("pikpak-resume-id-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let file = file_info("f1", "big.bin", 224);
        std::fs::write(dir.join("big.bin.part"), &content[..32]).unwrap();
        std::fs::write(
            dir.join("big.bin.part.meta"),
            serde_json::to_vec(&super::PartIdentity::of(&file, 224)).unwrap(),
        )
        .unwrap();

        super::download_file(&client, &file, &dir, &file.name, false)
            .await
            .expect("a matching partial must resume and finish");

        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            3,
            "resuming from 32/224 with 64-byte chunks must take three requests"
        );
        assert!(
            !dir.join("big.bin.part.meta").exists(),
            "the identity sidecar must be cleaned up on success"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_discards_a_partial_from_a_different_file() {
        // The stale bytes are deliberately wrong, so splicing them onto the real
        // tail would be visible in the result.
        let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content.clone(), 64, hits.clone()).await;
        let client = mock_client(addr);
        let dir = std::env::temp_dir().join(format!("pikpak-foreign-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("big.bin.part"), [0xAAu8; 32]).unwrap();
        let other = super::PartIdentity {
            id: "some-other-entry".to_string(),
            size: 999,
            modified_time: None,
        };
        std::fs::write(
            dir.join("big.bin.part.meta"),
            serde_json::to_vec(&other).unwrap(),
        )
        .unwrap();

        let file = file_info("f1", "big.bin", 224);
        super::download_file(&client, &file, &dir, &file.name, false)
            .await
            .expect("a foreign partial must be discarded and the file re-fetched");

        assert_eq!(
            std::fs::read(dir.join("big.bin")).unwrap(),
            content,
            "the foreign partial must not be spliced onto the real tail"
        );
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            4,
            "a discarded partial means a full re-download"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_discards_a_partial_without_an_identity() {
        // Partials left by older versions have no sidecar; they cannot be shown
        // to belong to this file, so they must not be resumed.
        let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content.clone(), 64, hits.clone()).await;
        let client = mock_client(addr);
        let dir = std::env::temp_dir().join(format!("pikpak-nometa-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        std::fs::write(dir.join("big.bin.part"), [0xAAu8; 32]).unwrap();

        let file = file_info("f1", "big.bin", 224);
        super::download_file(&client, &file, &dir, &file.name, false)
            .await
            .expect("a partial without an identity must be refetched");

        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_file_retries_while_it_keeps_making_progress() {
        // 224 bytes delivered 32 at a time needs 7 attempts. A budget that
        // counted every attempt would give up after 5 even though each one
        // advanced the file; only stalls should exhaust it.
        let content: Vec<u8> = (0..224u32).map(|i| i as u8).collect();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content.clone(), 32, hits.clone()).await;

        let client = mock_client(addr);

        let dir = std::env::temp_dir().join(format!("pikpak-progress-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let file = file_info("f1", "big.bin", 224);
        super::download_file(&client, &file, &dir, &file.name, false)
            .await
            .expect("a file that keeps advancing must finish");

        assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
        assert!(
            hits.load(std::sync::atomic::Ordering::SeqCst) > 5,
            "expected more attempts than the old five-attempt budget"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_file_gives_up_when_nothing_is_transferred() {
        // The server accepts the request and returns no bytes at all, so the
        // budget must expire and the error must surface.
        let content: Vec<u8> = vec![0u8; 32];
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content, 0, hits.clone()).await;

        let client = mock_client(addr);

        let dir = std::env::temp_dir().join(format!("pikpak-stallbudget-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();

        let file = file_info("f1", "big.bin", 32);
        let res = super::download_file(&client, &file, &dir, &file.name, false).await;
        assert!(res.is_err(), "a server that never advances must fail");
        assert_eq!(
            hits.load(std::sync::atomic::Ordering::SeqCst),
            super::MAX_STALLS as usize,
            "exactly the stall budget should be spent"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_expands_the_drive_root_into_its_children() {
        // `--path /` used to fail with "path must not be empty". The root has no
        // entry of its own, so it must be expanded into its children instead.
        let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content.clone(), 64, hits).await;
        let client = mock_client(addr);

        let dir = std::env::temp_dir().join(format!("pikpak-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        super::cmd_download(
            &client,
            crate::DownloadArgs {
                path: "/".to_string(),
                output: dir.to_string_lossy().into_owned(),
                jobs: 1,
            },
        )
        .await
        .expect("downloading the drive root must work");

        // The remote tree is root -> dir -> big.bin, so the layout is mirrored.
        assert_eq!(
            std::fs::read(dir.join("dir").join("sub").join("big.bin")).unwrap(),
            content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_walks_nested_folders_into_their_own_directories() {
        let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content.clone(), 64, hits).await;
        let client = mock_client(addr);

        let dir = std::env::temp_dir().join(format!("pikpak-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        super::cmd_download(
            &client,
            crate::DownloadArgs {
                path: "/dir".to_string(),
                output: dir.to_string_lossy().into_owned(),
                jobs: 1,
            },
        )
        .await
        .expect("downloading a folder must walk it");

        assert_eq!(
            std::fs::read(dir.join("dir").join("sub").join("big.bin")).unwrap(),
            content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_unknown_entry_kind_is_refused_not_downloaded() {
        // Entries whose kind the library does not model must be refused with a
        // clear error, not silently treated as files.
        let content: Vec<u8> = vec![0u8; 16];
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content, 16, hits).await;
        let client = mock_client(addr);

        let dir = std::env::temp_dir().join(format!("pikpak-unknown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let mut folder = file_info("U", "weird-dir", 0);
        folder.kind = pikpak::FileKind::Folder;

        let mut tasks = Vec::new();
        let mut claims = super::NameClaims::new();
        let err = super::collect_folder(&client, &folder, &dir, &mut tasks, &mut claims)
            .await
            .expect_err("an unknown entry kind must be refused");

        let text = format!("{err:#}");
        assert!(text.contains("unsupported entry kind"), "{text}");
        assert!(text.contains("weird.link"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_of_a_missing_path_still_reports_the_segment() {
        // The root special-case must not swallow ordinary NotFound errors.
        let content: Vec<u8> = vec![0u8; 16];
        let hits = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let addr = spawn_mock_stack(content, 16, hits).await;
        let client = mock_client(addr);

        let dir = std::env::temp_dir().join(format!("pikpak-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let err = super::cmd_download(
            &client,
            crate::DownloadArgs {
                path: "/No Such Folder".to_string(),
                output: dir.to_string_lossy().into_owned(),
                jobs: 1,
            },
        )
        .await
        .expect_err("a missing path must fail");

        let text = format!("{err:#}");
        assert!(text.contains("No Such Folder"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A stack whose first download link is already stale and whose detail
    /// endpoint hands out a fresh one on the next call.
    ///
    /// `expired_status` is what the stale link answers with. `refresh_fails` makes
    /// the second detail call fail as a rejected account token — the case where
    /// refreshing the link cannot possibly help.
    async fn spawn_expiring_link_server(
        content: Vec<u8>,
        expired_status: &'static str,
        refresh_fails: bool,
    ) -> (std::net::SocketAddr, Arc<std::sync::atomic::AtomicUsize>) {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let detail_calls = Arc::new(AtomicUsize::new(0));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let calls = detail_calls.clone();
        let body = content.clone();
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let content = body.clone();
                let detail_calls = calls.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut sock).await;
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();

                    if path.starts_with("/v1/auth/token") {
                        let body = r#"{"access_token":"access","refresh_token":"rotated","expires_in":7200,"sub":"user"}"#;
                        send_response(
                            &mut sock,
                            "200 OK",
                            &[("Content-Length", body.len().to_string())],
                            body.as_bytes(),
                        )
                        .await;
                    } else if path.starts_with("/v1/shield/captcha/init") {
                        let body = r#"{"captcha_token":"captcha"}"#;
                        send_response(
                            &mut sock,
                            "200 OK",
                            &[("Content-Length", body.len().to_string())],
                            body.as_bytes(),
                        )
                        .await;
                    } else if path.starts_with("/drive/v1/files/f1") {
                        let n = detail_calls.fetch_add(1, Ordering::SeqCst);
                        if refresh_fails && n > 0 {
                            let body = r#"{"error":"invalid_grant","error_code":4126}"#;
                            send_response(
                                &mut sock,
                                "400 Bad Request",
                                &[("Content-Length", body.len().to_string())],
                                body.as_bytes(),
                            )
                            .await;
                        } else {
                            let link = if n == 0 { "expired" } else { "fresh" };
                            let body = format!(
                                r#"{{"name":"big.bin","size":"{}","web_content_link":"http://{addr}/content/{link}"}}"#,
                                content.len()
                            );
                            send_response(
                                &mut sock,
                                "200 OK",
                                &[("Content-Length", body.len().to_string())],
                                body.as_bytes(),
                            )
                            .await;
                        }
                    } else if path.starts_with("/content/expired") {
                        let body = "stale link";
                        send_response(
                            &mut sock,
                            expired_status,
                            &[("Content-Length", body.len().to_string())],
                            body.as_bytes(),
                        )
                        .await;
                    } else if path.starts_with("/content/fresh") {
                        send_response(
                            &mut sock,
                            "200 OK",
                            &[("Content-Length", content.len().to_string())],
                            &content,
                        )
                        .await;
                    } else {
                        send_response(&mut sock, "404 Not Found", &[], b"").await;
                    }
                });
            }
        });
        (addr, detail_calls)
    }

    #[tokio::test]
    async fn download_refreshes_the_link_when_the_cdn_says_it_is_gone() {
        use std::sync::atomic::Ordering;

        // A stale signed link answers 401, 403 or 410 depending on the CDN; each
        // one means the same thing: fetch a fresh link and try again.
        for expired_status in ["401 Unauthorized", "403 Forbidden", "410 Gone"] {
            let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
            let (addr, detail_calls) =
                spawn_expiring_link_server(content.clone(), expired_status, false).await;
            let client = mock_client(addr);
            let dir = std::env::temp_dir().join(format!(
                "pikpak-stale-link-{}-{}",
                std::process::id(),
                expired_status.split(' ').next().unwrap()
            ));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).unwrap();

            let file = file_info("f1", "big.bin", content.len() as u64);
            super::download_file(&client, &file, &dir, &file.name, false)
                .await
                .unwrap_or_else(|e| panic!("{expired_status} must be retried: {e:#}"));

            assert_eq!(std::fs::read(dir.join("big.bin")).unwrap(), content);
            assert!(
                detail_calls.load(Ordering::SeqCst) >= 2,
                "{expired_status}: the link must be refreshed before retrying"
            );
            let _ = std::fs::remove_dir_all(&dir);
        }
    }

    #[tokio::test]
    async fn download_surfaces_a_rejected_token_instead_of_spending_the_budget() {
        use std::sync::atomic::Ordering;

        // The link looks stale, but refreshing it fails because the account token
        // was rejected. No retry can fix that, so the download must report the auth
        // failure rather than exhaust the budget and blame the CDN's status.
        let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let (addr, detail_calls) =
            spawn_expiring_link_server(content.clone(), "401 Unauthorized", true).await;
        let client = mock_client(addr);
        let dir = std::env::temp_dir().join(format!("pikpak-dead-token-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let file = file_info("f1", "big.bin", content.len() as u64);
        let err = super::download_file(&client, &file, &dir, &file.name, false)
            .await
            .expect_err("a rejected token must fail the download");

        let text = format!("{err:#}");
        assert!(text.contains("invalid_grant"), "{text}");
        assert!(
            text.contains("refresh the download link"),
            "the auth failure must be attributed to the link refresh: {text}"
        );
        assert_eq!(
            detail_calls.load(Ordering::SeqCst),
            2,
            "one refresh attempt, then give up — the retry budget must stay untouched"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A mock stack whose folder `A` holds `entries` — `(id, name, content)` —
    /// each served from its own content URL.
    async fn spawn_named_files_server(
        entries: Vec<(&'static str, &'static str, Vec<u8>)>,
    ) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let entries = Arc::new(entries);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let entries = entries.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut sock).await;
                    let path = request.split_whitespace().nth(1).unwrap_or("/").to_string();
                    let json = |body: String| (("200 OK"), body.into_bytes());
                    let (status, body) = if path.starts_with("/v1/auth/token") {
                        json(r#"{"access_token":"access","refresh_token":"rotated","expires_in":7200,"sub":"user"}"#.to_string())
                    } else if path.starts_with("/v1/shield/captcha/init") {
                        json(r#"{"captcha_token":"captcha"}"#.to_string())
                    } else if path.starts_with("/drive/v1/files?") {
                        if path.contains("parent_id=A") {
                            let files: Vec<String> = entries
                                .iter()
                                .map(|(id, name, data)| {
                                    format!(
                                        r#"{{"id":"{id}","name":"{name}","size":"{}","kind":"drive#file"}}"#,
                                        data.len()
                                    )
                                })
                                .collect();
                            json(format!(r#"{{"files":[{}]}}"#, files.join(",")))
                        } else {
                            json(
                                r#"{"files":[{"id":"A","name":"dir","kind":"drive#folder"}]}"#
                                    .to_string(),
                            )
                        }
                    } else if let Some(id) = path.strip_prefix("/drive/v1/files/") {
                        let (_, name, data) = entries.iter().find(|(i, _, _)| *i == id).unwrap();
                        json(format!(
                            r#"{{"name":"{name}","size":"{}","web_content_link":"http://{addr}/content/{id}"}}"#,
                            data.len()
                        ))
                    } else if let Some(id) = path.strip_prefix("/content/") {
                        let (_, _, data) = entries.iter().find(|(i, _, _)| *i == id).unwrap();
                        ("200 OK", data.clone())
                    } else {
                        ("404 Not Found", Vec::new())
                    };
                    let headers = [("Content-Length", body.len().to_string())];
                    send_response(&mut sock, status, &headers, &body).await;
                });
            }
        });
        addr
    }

    /// Contents of every finished file directly inside `dir`, sorted.
    fn finished_contents(dir: &std::path::Path) -> Vec<Vec<u8>> {
        let mut contents: Vec<Vec<u8>> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().unwrap().to_string_lossy();
                path.is_file() && !name.ends_with(".part") && !name.ends_with(".meta")
            })
            .map(|path| std::fs::read(path).unwrap())
            .collect();
        contents.sort();
        contents
    }

    #[tokio::test]
    async fn same_named_remote_files_do_not_overwrite_each_other() {
        // PikPak identifies entries by id, so one folder can hold two files with
        // the same name. Each must land in its own local file: sharing one path
        // (and one `.part`) would silently lose a file, or — with `-j` — splice
        // two transfers into the same partial.
        let one = vec![1u8; 32];
        let two = vec![2u8; 48];
        let addr = spawn_named_files_server(vec![
            ("d1", "dup.bin", one.clone()),
            ("d2", "dup.bin", two.clone()),
        ])
        .await;
        let client = mock_client(addr);

        for jobs in [1, 2] {
            let out = std::env::temp_dir()
                .join(format!("pikpak-dup-names-{}-{jobs}", std::process::id()));
            let _ = std::fs::remove_dir_all(&out);

            super::cmd_download(
                &client,
                crate::DownloadArgs {
                    path: "/dir".to_string(),
                    output: out.to_string_lossy().into_owned(),
                    jobs,
                },
            )
            .await
            .unwrap_or_else(|e| panic!("-j {jobs}: both files must download: {e:#}"));

            assert_eq!(
                finished_contents(&out.join("dir")),
                vec![one.clone(), two.clone()],
                "-j {jobs}: both same-named files must survive"
            );
            let _ = std::fs::remove_dir_all(&out);
        }
    }
}
