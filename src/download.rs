//! Downloading: planning local names, then fetching each file through the
//! library's resumable transfer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::path::{Path as StdPath, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use anyhow::{bail, Context, Result};
use humansize::{format_size, BINARY};
use serde::Serialize;
use tokio::sync::Semaphore;

use crate::output::Output;
use crate::DownloadArgs;
use pikpak::{is_drive_root, Client, FileKind};

/// One file to fetch, and the local path it lands at.
struct DownloadTask {
    file: pikpak::FileInfo,
    /// Unique among the run's destinations; see [`NameClaims`].
    dest: PathBuf,
}

/// A folder still to be walked, and where its contents belong.
struct FolderToWalk {
    id: String,
    name: String,
    dir: PathBuf,
}

/// Maximum byte length for a single filesystem path component across common OSes.
const MAX_COMPONENT_LEN: usize = 255;

/// Directory, directly inside the output directory, that holds every transfer
/// in progress as `<file id>.part` beside its `.part.meta` identity sidecar.
///
/// Keyed by id rather than by local name, a partial is found again however the
/// names are handed out next time, and no remote name can land on one.
const PARTS_DIR: &str = ".pikpak-parts";

/// Truncate `s` to at most `max_bytes`, ending on a valid UTF-8 character boundary.
fn truncate_utf8(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut boundary = max_bytes;
    while boundary > 0 && !s.is_char_boundary(boundary) {
        boundary -= 1;
    }
    &s[..boundary]
}

/// Local names already handed out, per directory.
///
/// PikPak identifies entries by id, so one folder may hold several entries with
/// the same name — and names that differ only in case, or only in characters
/// Windows replaces, collide on the local filesystem too. Every entry claims its
/// local name here before anything is written; a taken name gets a ` (2)`,
/// ` (3)`, … suffix.
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
    /// are suffixed as a whole. Generated names are guaranteed to fit within
    /// [`MAX_COMPONENT_LEN`] bytes, truncating the stem without splitting UTF-8
    /// characters.
    fn claim(&mut self, dir: &StdPath, raw: &str, is_file: bool) -> Result<String> {
        let base = safe_component(raw)?;
        let taken = self.taken.entry(dir.to_path_buf()).or_default();
        let case_insensitive = self.case_insensitive;
        let mut take = |name: &str| taken.insert(fold(name, case_insensitive));
        let max_len = MAX_COMPONENT_LEN;

        // Split stem and extension so suffixes insert before the extension.
        // A leading dot marks a hidden name, not an extension.
        let (raw_stem, raw_ext) = match base.rfind('.') {
            Some(dot) if is_file && dot > 0 => base.split_at(dot),
            _ => (base.as_str(), ""),
        };

        let ext = truncate_utf8(raw_ext, max_len - 1);
        let stem = truncate_utf8(raw_stem, max_len - ext.len());

        let initial = format!("{stem}{ext}");
        if take(&initial) {
            return Ok(initial);
        }

        let mut n = 2u32;
        loop {
            let suffix = format!(" ({n})");
            // An extension long enough to leave no room for the suffix gives
            // way too, or the name would overrun `max_len`.
            let fit_ext = truncate_utf8(ext, max_len - suffix.len());
            let fit_stem = truncate_utf8(stem, max_len - fit_ext.len() - suffix.len());
            let candidate = format!("{fit_stem}{suffix}{fit_ext}");
            if take(&candidate) {
                return Ok(candidate);
            }
            n += 1;
        }
    }
}

/// The key a name is claimed under: folded where the filesystem folds case.
fn fold(name: &str, case_insensitive: bool) -> String {
    if case_insensitive {
        name.to_lowercase()
    } else {
        name.to_string()
    }
}

/// Everything a run will fetch, named before any byte moves.
///
/// Folders are walked breadth-first over an explicit queue, so a deep tree is
/// bounded by memory rather than the call stack. The whole tree is enumerated —
/// and its directories created — before any transfer starts, so concurrent
/// downloads never race on `mkdir`, at the cost of delaying the first byte on
/// very large trees.
struct Plan {
    tasks: Vec<DownloadTask>,
    folders: VecDeque<FolderToWalk>,
    claims: NameClaims,
    /// Names of entries whose kind this crate does not model: refused, not
    /// guessed at — each one failed file, so the rest still download.
    refused: Vec<String>,
}

impl Plan {
    fn new(output: &StdPath) -> Self {
        let mut claims = NameClaims::new();
        // Taken before any remote entry can claim it.
        claims
            .claim(output, PARTS_DIR, false)
            .expect("a constant, safe name");
        Self {
            tasks: Vec::new(),
            folders: VecDeque::new(),
            claims,
            refused: Vec::new(),
        }
    }

    /// Queue `entry` under `dir`: a file to fetch, or a folder to walk.
    fn add(&mut self, entry: pikpak::FileInfo, dir: &StdPath) -> Result<()> {
        // The listing name is what the user saw; an entry without one still
        // gets a stable, unique name instead of failing the whole run.
        let raw = if entry.name.trim().is_empty() {
            &entry.id
        } else {
            &entry.name
        };
        match entry.kind {
            FileKind::File => {
                let name = self.claims.claim(dir, raw, true)?;
                self.tasks.push(DownloadTask {
                    dest: dir.join(name),
                    file: entry,
                });
            }
            FileKind::Folder => {
                let name = self.claims.claim(dir, raw, false)?;
                self.folders.push_back(FolderToWalk {
                    dir: dir.join(name),
                    id: entry.id,
                    name: entry.name,
                });
            }
            FileKind::Unknown => self.refused.push(entry.name),
        }
        Ok(())
    }

    /// Walk every queued folder: create its directory, queue its contents.
    async fn walk(&mut self, client: &Client) -> Result<()> {
        while let Some(folder) = self.folders.pop_front() {
            tokio::fs::create_dir_all(&folder.dir)
                .await
                .context("failed to create folder")?;
            eprintln!("Scanning folder: {}", folder.name);
            for entry in client.list_folder(&folder.id).await? {
                self.add(entry, &folder.dir)?;
            }
        }
        Ok(())
    }
}

/// A run's counts as they change, so a detached job's record can report them
/// while the run is still going.
#[derive(Debug, Default)]
pub(crate) struct Tally {
    /// Files the run planned to transfer, once planning is done.
    pub files: AtomicUsize,
    pub downloaded: AtomicUsize,
    pub failed: AtomicUsize,
    pub bytes: AtomicU64,
}

/// What a download run did, as reported in `--json` output.
#[derive(Debug, Serialize)]
pub(crate) struct DownloadSummary {
    /// Output directory, as given on the command line.
    pub output: String,
    /// Files the run planned to transfer.
    pub files: usize,
    /// Files that finished, including ones resumed from an earlier run and ones
    /// already on disk at their listed size, which are skipped.
    pub downloaded: usize,
    /// Files that failed.
    pub failed: usize,
    /// Bytes across the files that finished.
    pub bytes: u64,
}

pub(crate) async fn cmd_download(
    client: &Client,
    args: DownloadArgs,
    ui: Output,
    tally: &Tally,
) -> Result<DownloadSummary> {
    let output = StdPath::new(&args.output);
    tokio::fs::create_dir_all(output)
        .await
        .context("failed to create output directory")?;

    let mut plan = Plan::new(output);
    if is_drive_root(&args.path) {
        // The virtual root has no entry of its own: walk it as the output
        // directory itself, so its files land there and its folders beside them.
        plan.folders.push_back(FolderToWalk {
            id: String::new(),
            name: "/".to_string(),
            dir: output.to_path_buf(),
        });
    } else {
        plan.add(client.resolve_path_info(&args.path).await?, output)?;
    }
    plan.walk(client).await?;
    let Plan { tasks, refused, .. } = plan;
    let planned = tasks.len() + refused.len();
    tally.files.store(planned, Ordering::Relaxed);

    if planned == 0 {
        eprintln!("(nothing to download)");
        return Ok(DownloadSummary {
            output: args.output,
            files: 0,
            downloaded: 0,
            failed: 0,
            bytes: 0,
        });
    }

    let parts = output.join(PARTS_DIR);
    tokio::fs::create_dir_all(&parts)
        .await
        .context("failed to create the partials directory")?;

    let jobs = args.jobs.max(1);
    // Live byte-level progress only reads cleanly with a single active
    // transfer; with concurrency we fall back to per-file start/finish lines.
    let show_progress = ui.progress(jobs);

    let sem = Arc::new(Semaphore::new(jobs));
    let mut set = tokio::task::JoinSet::new();
    // A panicked task returns no value, only its id: map that back to the file.
    let mut index_of = HashMap::new();
    for (index, task) in tasks.into_iter().enumerate() {
        let client = client.clone();
        let sem = sem.clone();
        let part = parts.join(format!("{}.part", safe_component(&task.file.id)?));
        let handle = set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore is never closed");
            let result = download_file(&client, &task.file, &task.dest, &part, show_progress).await;
            (index, result)
        });
        index_of.insert(handle.id(), index);
    }

    // The failure reported (and so the exit code) is the earliest file's in
    // listing order, not whichever finished failing first: the same run fails
    // the same way every time. Refusals are known before any transfer, so they
    // come first.
    let mut first_err: Option<(usize, anyhow::Error)> = None;
    for name in &refused {
        let e = anyhow::anyhow!(
            "refusing to download {name:?}: unsupported entry kind reported by the server"
        );
        eprintln!("error: {e}");
        tally.failed.fetch_add(1, Ordering::Relaxed);
        first_err.get_or_insert((0, e));
    }
    while let Some(joined) = set.join_next().await {
        // A panicking transfer is one failed file, not the end of the others.
        let (index, result) = joined.unwrap_or_else(|panic| {
            (
                index_of.get(&panic.id()).copied().unwrap_or(usize::MAX),
                Err(anyhow::anyhow!("download task panicked: {panic}")),
            )
        });
        match result {
            Ok(size) => {
                tally.downloaded.fetch_add(1, Ordering::Relaxed);
                tally.bytes.fetch_add(size, Ordering::Relaxed);
            }
            Err(e) => {
                tally.failed.fetch_add(1, Ordering::Relaxed);
                eprintln!("error: {e:#}");
                if first_err.as_ref().is_none_or(|(first, _)| index < *first) {
                    first_err = Some((index, e));
                }
            }
        }
    }
    // Gone once nothing is left to resume; kept, with its partials, otherwise.
    let _ = tokio::fs::remove_dir(&parts).await;
    let downloaded = tally.downloaded.load(Ordering::Relaxed);
    let failed = tally.failed.load(Ordering::Relaxed);
    let bytes = tally.bytes.load(Ordering::Relaxed);
    // A failed run reports only its failure, so the counts travel in its message.
    match first_err {
        Some((_, e)) => Err(e).context(format!(
            "{failed} of {planned} files failed ({downloaded} downloaded, {bytes} bytes)"
        )),
        None => Ok(DownloadSummary {
            output: args.output,
            files: planned,
            downloaded,
            failed,
            bytes,
        }),
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

/// Fetch one planned file, skipping one already on disk at its listed size.
async fn download_file(
    client: &Client,
    file: &pikpak::FileInfo,
    dest: &StdPath,
    part: &StdPath,
    show_progress: bool,
) -> Result<u64> {
    let name = dest.file_name().unwrap_or_default().to_string_lossy();
    // ponytail: "already complete" means the listed size, not a hash — the
    // listing carries none. A size of 0 may mean "not reported", so an empty
    // file is always fetched again (it costs nothing).
    if let Ok(meta) = tokio::fs::metadata(dest).await {
        if meta.is_file() && file.size > 0 && meta.len() == file.size {
            eprintln!("Skipped (already complete): {name}");
            return Ok(meta.len());
        }
    }

    eprintln!("Downloading: {name} ({})", format_size(file.size, BINARY));
    let progress = |done: u64, total: u64| {
        if show_progress && total > 0 {
            let pct = (done as f64 / total as f64) * 100.0;
            eprint!(
                "\r  {} / {} ({pct:.1}%)",
                format_size(done, BINARY),
                format_size(total, BINARY),
            );
            if done >= total {
                eprintln!();
            }
        }
    };
    let size = client
        .download_file(file, dest, part, &progress)
        .await
        .with_context(|| format!("failed to download {name}"))?;
    eprintln!("Saved: {}", dest.display());
    Ok(size)
}

#[cfg(test)]
mod tests {
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

    #[test]
    fn a_colliding_name_with_a_huge_extension_still_fits() {
        let dir = std::path::Path::new("/tmp/test");
        let mut claims = super::NameClaims::with_case_folding(false);
        let name = format!("a.{}", "x".repeat(243));
        claims.claim(dir, &name, true).unwrap();
        let second = claims.claim(dir, &name, true).unwrap();
        assert!(second.len() <= super::MAX_COMPONENT_LEN, "{}", second.len());
    }

    #[test]
    fn name_claims_truncates_long_names_and_preserves_limits() {
        let dir = std::path::Path::new("/tmp/test");
        let mut claims = super::NameClaims::with_case_folding(false);

        // A 251-byte stem + ".txt" (4 bytes) = 255 bytes: exactly the limit.
        let long_stem = "a".repeat(251);
        let name = format!("{long_stem}.txt");
        let first = claims.claim(dir, &name, true).unwrap();
        assert_eq!(first.len(), 255);
        assert_eq!(first, name);

        // Second colliding file must truncate the stem to keep the total length <= 255 bytes.
        let second = claims.claim(dir, &name, true).unwrap();
        assert!(second.len() <= 255);
        assert!(second.ends_with(" (2).txt"));

        // Multi-byte UTF-8 (each character 3 bytes) near 255 bytes must truncate safely
        // without splitting a UTF-8 character boundary.
        let cjk_stem = "测".repeat(84); // 84 * 3 = 252 bytes
        let cjk_name = format!("{cjk_stem}.bin"); // 252 + 4 = 256 bytes (> 255)
        let cjk_first = claims.claim(dir, &cjk_name, true).unwrap();
        assert!(cjk_first.len() <= 255);
        assert!(cjk_first.ends_with(".bin"));

        let cjk_second = claims.claim(dir, &cjk_name, true).unwrap();
        assert!(cjk_second.len() <= 255);
        assert!(cjk_second.ends_with(" (2).bin"));
    }

    #[test]
    fn a_remote_entry_never_takes_the_partials_directory() {
        let out = std::path::Path::new("/tmp/test");
        let mut plan = super::Plan::new(out);
        let mut folder = file_info("p", super::PARTS_DIR, 0);
        folder.kind = pikpak::FileKind::Folder;
        plan.add(folder, out).unwrap();
        assert_eq!(
            plan.folders[0].dir,
            out.join(format!("{} (2)", super::PARTS_DIR))
        );
    }

    #[tokio::test]
    async fn a_file_already_complete_on_disk_is_not_fetched_again() {
        // Nothing listens on the API: reaching it would fail the test.
        let client = pikpak::Client::builder()
            .refresh_token("dummy")
            .auth_base_url("http://127.0.0.1:1")
            .api_base_url("http://127.0.0.1:1")
            .build()
            .unwrap();
        let dir = std::env::temp_dir().join(format!("pikpak-skip-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("done.bin"), b"1234").unwrap();
        let file: pikpak::FileInfo =
            serde_json::from_str(r#"{"id":"f1","name":"done.bin","size":"4","kind":"drive#file"}"#)
                .unwrap();

        let size = super::download_file(
            &client,
            &file,
            &dir.join("done.bin"),
            &dir.join("f1.part"),
            false,
        )
        .await
        .expect("a complete file is skipped, not fetched");
        assert_eq!(size, 4);
        assert_eq!(std::fs::read(dir.join("done.bin")).unwrap(), b"1234");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_download_summary_keeps_its_json_field_names() {
        let summary = super::DownloadSummary {
            output: "./downloads".to_string(),
            files: 3,
            downloaded: 2,
            failed: 1,
            bytes: 4096,
        };
        let json = serde_json::to_value(&summary).unwrap();

        assert_eq!(json["output"], "./downloads");
        assert_eq!(json["files"], 3);
        assert_eq!(json["downloaded"], 2);
        assert_eq!(json["failed"], 1);
        assert_eq!(json["bytes"], 4096);
    }

    /// Read one HTTP request (headers plus the `Content-Length` body) as text.
    ///
    /// The binary's one copy of the library's `test_http` helper, which is
    /// `#[cfg(test)]`-gated and so invisible from here.
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

    /// One remote entry for [`spawn_drive`]: parent id (`""` is the root), id,
    /// name, kind, and the content a file serves.
    type Entry = (
        &'static str,
        &'static str,
        &'static str,
        &'static str,
        Vec<u8>,
    );

    /// A mock PikPak stack serving `tree` on one port: auth, captcha init,
    /// folder listings, file details, and each file's content whole from its
    /// own URL.
    async fn spawn_drive(tree: Vec<Entry>) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let tree = Arc::new(tree);
        tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    return;
                };
                let tree = tree.clone();
                tokio::spawn(async move {
                    let request = read_request(&mut sock).await;
                    let path = request.split_whitespace().nth(1).unwrap_or("/");
                    let find = |id: &str| tree.iter().find(|entry| entry.1 == id).unwrap();
                    let (status, body) = if path.starts_with("/v1/auth/token") {
                        ("200 OK", r#"{"access_token":"access","refresh_token":"rotated","expires_in":7200,"sub":"user"}"#.as_bytes().to_vec())
                    } else if path.starts_with("/v1/shield/captcha/init") {
                        ("200 OK", br#"{"captcha_token":"captcha"}"#.to_vec())
                    } else if let Some(query) = path.strip_prefix("/drive/v1/files?") {
                        let parent = query
                            .split('&')
                            .find_map(|pair| pair.strip_prefix("parent_id="))
                            .unwrap_or("");
                        let files: Vec<String> = tree
                            .iter()
                            .filter(|entry| entry.0 == parent)
                            .map(|(_, id, name, kind, data)| {
                                format!(
                                    r#"{{"id":"{id}","name":"{name}","kind":"{kind}","size":"{}"}}"#,
                                    data.len()
                                )
                            })
                            .collect();
                        (
                            "200 OK",
                            format!(r#"{{"files":[{}]}}"#, files.join(",")).into_bytes(),
                        )
                    } else if let Some(id) = path.strip_prefix("/drive/v1/files/") {
                        let (_, _, name, _, data) = find(id);
                        let body = format!(
                            r#"{{"name":"{name}","size":"{}","web_content_link":"http://{addr}/content/{id}"}}"#,
                            data.len()
                        );
                        ("200 OK", body.into_bytes())
                    } else if let Some(id) = path.strip_prefix("/content/") {
                        ("200 OK", find(id).4.clone())
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

    /// The tree most tests walk: root -> dir -> sub -> big.bin.
    fn nested_tree(content: &[u8]) -> Vec<Entry> {
        vec![
            ("", "A", "dir", "drive#folder", Vec::new()),
            ("A", "B", "sub", "drive#folder", Vec::new()),
            ("B", "f1", "big.bin", "drive#file", content.to_vec()),
        ]
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

    /// Download `path` into `out` the way the CLI does.
    async fn download(
        client: &pikpak::Client,
        path: &str,
        out: &std::path::Path,
        jobs: usize,
    ) -> anyhow::Result<super::DownloadSummary> {
        super::cmd_download(
            client,
            crate::DownloadArgs {
                path: path.to_string(),
                output: out.to_string_lossy().into_owned(),
                jobs,
                detach: false,
            },
            crate::output::Output::new(false, false),
            &super::Tally::default(),
        )
        .await
    }

    #[tokio::test]
    async fn download_expands_the_drive_root_into_its_children() {
        // `--path /` used to fail with "path must not be empty". The root has no
        // entry of its own, so it must be expanded into its children instead.
        let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let client = mock_client(spawn_drive(nested_tree(&content)).await);
        let dir = std::env::temp_dir().join(format!("pikpak-root-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        download(&client, "/", &dir, 1)
            .await
            .expect("downloading the drive root must work");

        // The remote layout is mirrored, and no partials are left behind.
        assert_eq!(
            std::fs::read(dir.join("dir").join("sub").join("big.bin")).unwrap(),
            content
        );
        assert!(!dir.join(super::PARTS_DIR).exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_walks_nested_folders_into_their_own_directories() {
        let content: Vec<u8> = (0..64u32).map(|i| i as u8).collect();
        let client = mock_client(spawn_drive(nested_tree(&content)).await);
        let dir = std::env::temp_dir().join(format!("pikpak-nested-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        download(&client, "/dir", &dir, 1)
            .await
            .expect("downloading a folder must walk it");

        assert_eq!(
            std::fs::read(dir.join("dir").join("sub").join("big.bin")).unwrap(),
            content
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_unknown_entry_kind_fails_alone_while_the_rest_download() {
        // An entry whose kind the library does not model is refused with a
        // clear error, never treated as a file — but as one failed file, not the
        // end of the run: its neighbours still land.
        let data = vec![7u8; 16];
        let client = mock_client(
            spawn_drive(vec![
                ("", "M", "mixed", "drive#folder", Vec::new()),
                ("M", "u1", "weird.link", "drive#shortcut", Vec::new()),
                ("M", "f2", "ok.bin", "drive#file", data.clone()),
            ])
            .await,
        );
        let dir = std::env::temp_dir().join(format!("pikpak-unknown-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let tally = super::Tally::default();
        let err = super::cmd_download(
            &client,
            crate::DownloadArgs {
                path: "/mixed".to_string(),
                output: dir.to_string_lossy().into_owned(),
                jobs: 1,
                detach: false,
            },
            crate::output::Output::new(false, false),
            &tally,
        )
        .await
        .expect_err("a refused entry fails the run");

        let text = format!("{err:#}");
        assert!(text.contains("unsupported entry kind"), "{text}");
        assert!(text.contains("weird.link"), "{text}");
        assert!(text.contains("1 of 2 files failed"), "{text}");
        assert_eq!(
            std::fs::read(dir.join("mixed").join("ok.bin")).unwrap(),
            data
        );
        use std::sync::atomic::Ordering::Relaxed;
        assert_eq!(tally.files.load(Relaxed), 2);
        assert_eq!(tally.downloaded.load(Relaxed), 1);
        assert_eq!(tally.failed.load(Relaxed), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn download_of_a_missing_path_still_reports_the_segment() {
        // The root special-case must not swallow ordinary NotFound errors.
        let client = mock_client(spawn_drive(nested_tree(&[])).await);
        let dir = std::env::temp_dir().join(format!("pikpak-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);

        let err = download(&client, "/No Such Folder", &dir, 1)
            .await
            .expect_err("a missing path must fail");

        let text = format!("{err:#}");
        assert!(text.contains("No Such Folder"), "{text}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Contents of every finished file directly inside `dir`, sorted.
    fn finished_contents(dir: &std::path::Path) -> Vec<Vec<u8>> {
        let mut contents: Vec<Vec<u8>> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| path.is_file())
            .map(|path| std::fs::read(path).unwrap())
            .collect();
        contents.sort();
        contents
    }

    #[tokio::test]
    async fn same_named_remote_files_do_not_overwrite_each_other() {
        // PikPak identifies entries by id, so one folder can hold two files with
        // the same name. Each must land in its own local file: sharing one path
        // would silently lose a file.
        let one = vec![1u8; 32];
        let two = vec![2u8; 48];
        let client = mock_client(
            spawn_drive(vec![
                ("", "A", "dir", "drive#folder", Vec::new()),
                ("A", "d1", "dup.bin", "drive#file", one.clone()),
                ("A", "d2", "dup.bin", "drive#file", two.clone()),
            ])
            .await,
        );

        for jobs in [1, 2] {
            let out = std::env::temp_dir()
                .join(format!("pikpak-dup-names-{}-{jobs}", std::process::id()));
            let _ = std::fs::remove_dir_all(&out);

            download(&client, "/dir", &out, jobs)
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

    #[tokio::test]
    async fn a_partial_resumes_whatever_name_its_file_is_given() {
        // The partial of `d2` was left by a run that named it `dup (2).bin`.
        // Listed first this time, `d2` is named `dup.bin` — its partial is
        // still its own, found by id, and nothing else can claim it.
        let one = vec![1u8; 32];
        let two = vec![2u8; 48];
        let client = mock_client(
            spawn_drive(vec![
                ("", "A", "dir", "drive#folder", Vec::new()),
                ("A", "d2", "dup.bin", "drive#file", two.clone()),
                ("A", "d1", "dup.bin", "drive#file", one.clone()),
            ])
            .await,
        );
        let out = std::env::temp_dir().join(format!("pikpak-part-by-id-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out);
        let parts = out.join(super::PARTS_DIR);
        std::fs::create_dir_all(&parts).unwrap();
        std::fs::write(parts.join("d2.part"), &two[..16]).unwrap();
        std::fs::write(
            parts.join("d2.part.meta"),
            r#"{"id":"d2","size":48,"modified_time":null}"#,
        )
        .unwrap();

        download(&client, "/dir", &out, 1)
            .await
            .expect("both files must download");

        assert_eq!(std::fs::read(out.join("dir").join("dup.bin")).unwrap(), two);
        assert_eq!(
            std::fs::read(out.join("dir").join("dup (2).bin")).unwrap(),
            one
        );
        assert!(!parts.exists(), "an emptied partials directory is removed");
        let _ = std::fs::remove_dir_all(&out);
    }
}
