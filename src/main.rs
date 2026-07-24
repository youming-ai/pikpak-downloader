use std::future::Future;
use std::path::{Path as StdPath, PathBuf};
use std::pin::Pin;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use humansize::{format_size, BINARY};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use tracing_subscriber::EnvFilter;

use pikpak::auth::OAuthCredentials;
use pikpak::{Client, FileKind};

#[derive(Debug, Parser)]
#[command(
    name = "pikpak",
    version,
    about = "PikPak cloud storage CLI",
    after_help = "Config via .env or environment:\n  PIKPAK_REFRESH_TOKEN  (required) refresh token from the web UI\n  PIKPAK_PROXY          (optional) HTTP(S) proxy URL\n  PIKPAK_CLIENT_ID      (optional) override OAuth client id\n  PIKPAK_CLIENT_SECRET  (optional) override OAuth client secret"
)]
struct Cli {
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List files and directories.
    Ls(LsArgs),
    /// Download files or folders.
    Download(DownloadArgs),
    /// View storage quota.
    Quota(QuotaArgs),
}

#[derive(Debug, Parser)]
struct LsArgs {
    #[arg(long, default_value = "/")]
    path: String,
    #[arg(short = 'l', long)]
    long: bool,
    #[arg(short = 'h', long)]
    human: bool,
}

#[derive(Debug, Parser)]
struct DownloadArgs {
    /// Remote path to download, e.g. /My Pack/video.mp4.
    #[arg(long)]
    path: String,
    /// Local output directory.
    #[arg(long, default_value = "./downloads")]
    output: String,
    /// Number of files to download concurrently (folders only).
    #[arg(short = 'j', long, default_value_t = 1)]
    jobs: usize,
}

#[derive(Debug, Parser)]
struct QuotaArgs {
    /// Print raw byte counts.
    #[arg(long)]
    raw: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("pikpak=debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match run(cli).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli) -> Result<()> {
    let (client, original_token) = build_client()?;
    let result = match cli.command {
        Command::Ls(args) => cmd_ls(&client, args).await,
        Command::Download(args) => cmd_download(&client, args).await,
        Command::Quota(args) => cmd_quota(&client, args).await,
    };
    persist_rotated_token(&client, &original_token).await;
    result
}

fn build_client() -> Result<(Client, String)> {
    let refresh_token = std::env::var("PIKPAK_REFRESH_TOKEN")
        .context("PIKPAK_REFRESH_TOKEN must be set in the environment or .env")?;

    let mut builder = Client::builder().refresh_token(refresh_token.clone());

    if let Ok(proxy) = std::env::var("PIKPAK_PROXY") {
        if !proxy.is_empty() {
            builder = builder.proxy(proxy);
        }
    }

    if let (Ok(id), Ok(secret)) = (
        std::env::var("PIKPAK_CLIENT_ID"),
        std::env::var("PIKPAK_CLIENT_SECRET"),
    ) {
        if !id.is_empty() && !secret.is_empty() {
            builder = builder.credentials(OAuthCredentials::new(id, secret));
        }
    }

    let client = builder.build().context("failed to build API client")?;
    Ok((client, refresh_token))
}

/// After a command runs, PikPak may have rotated the refresh token. Persist the
/// new value to `./.env` if present so the stored credential stays valid;
/// otherwise print it so the user can update their environment.
async fn persist_rotated_token(client: &Client, original: &str) {
    let current = client.tokens().current_refresh_token().await;
    if current == original {
        return;
    }
    match update_env_token(StdPath::new(".env"), &current) {
        Ok(true) => eprintln!("note: refresh token rotated; updated PIKPAK_REFRESH_TOKEN in .env"),
        Ok(false) => {
            eprintln!("note: refresh token rotated; update PIKPAK_REFRESH_TOKEN to:\n  {current}")
        }
        Err(e) => eprintln!(
            "warning: refresh token rotated but .env update failed ({e}); new token:\n  {current}"
        ),
    }
}

/// Rewrite the `PIKPAK_REFRESH_TOKEN=` line in `env_path`, preserving every
/// other line. Returns `Ok(true)` if the key was found and rewritten, or
/// `Ok(false)` if the file is absent or has no such key.
fn update_env_token(env_path: &StdPath, new_token: &str) -> std::io::Result<bool> {
    if !env_path.exists() {
        return Ok(false);
    }
    let content = std::fs::read_to_string(env_path)?;
    let mut found = false;
    let mut out = String::with_capacity(content.len() + new_token.len());
    for line in content.lines() {
        if line.trim_start().starts_with("PIKPAK_REFRESH_TOKEN=") {
            out.push_str("PIKPAK_REFRESH_TOKEN=");
            out.push_str(new_token);
            found = true;
        } else {
            out.push_str(line);
        }
        out.push('\n');
    }
    if !found {
        return Ok(false);
    }
    std::fs::write(env_path, out)?;
    Ok(true)
}

async fn cmd_ls(client: &Client, args: LsArgs) -> Result<()> {
    let parent_id = client.resolve_path(&args.path).await?;

    let files: Vec<pikpak::FileInfo> = client
        .list_folder(&parent_id)
        .await
        .context("list_folder failed")?;

    if files.is_empty() {
        println!("(empty)");
        return Ok(());
    }

    if args.long {
        println!("{:<10} {:>12} name", "kind", "size");
        println!("{}", "-".repeat(50));
        for f in &files {
            let kind = if f.kind.is_folder() { "folder" } else { "file" };
            let size = if args.human {
                format_size(f.size, BINARY)
            } else {
                f.size.to_string()
            };
            println!("{kind:<10} {size:>12} {}", f.name);
        }
    } else {
        for f in &files {
            let marker = if f.kind == FileKind::Folder { "/" } else { "" };
            println!("{}{}", f.name, marker);
        }
    }

    Ok(())
}

async fn cmd_download(client: &Client, args: DownloadArgs) -> Result<()> {
    let output = StdPath::new(&args.output);
    tokio::fs::create_dir_all(output)
        .await
        .context("failed to create output directory")?;

    let info = client.resolve_path_info(&args.path).await?;

    // Flatten into (file, destination dir), creating the directory tree up
    // front so concurrent downloads never race on mkdir.
    let mut tasks: Vec<(pikpak::FileInfo, PathBuf)> = Vec::new();
    if info.kind.is_folder() {
        collect_folder(client, &info, output, &mut tasks).await?;
    } else {
        tasks.push((info, output.to_path_buf()));
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
    for (file, dir) in tasks {
        let client = client.clone();
        let sem = sem.clone();
        set.spawn(async move {
            let _permit = sem.acquire().await.expect("semaphore is never closed");
            download_file(&client, &file, &dir, show_progress).await
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
fn safe_component(name: &str) -> Result<String> {
    match StdPath::new(name).file_name().and_then(|s| s.to_str()) {
        Some(base) => Ok(base.to_string()),
        None => bail!("refusing unsafe remote name: {name:?}"),
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

/// Maximum download attempts (initial try + retries) per file.
const MAX_DOWNLOAD_ATTEMPTS: u32 = 5;

async fn download_file(
    client: &Client,
    file: &pikpak::FileInfo,
    output_dir: &StdPath,
    show_progress: bool,
) -> Result<()> {
    let dl: pikpak::DownloadInfo = client
        .get_download_url(&file.id)
        .await
        .context("failed to get download URL")?;

    let file_path = output_dir.join(safe_component(&dl.name)?);
    let mut part = file_path.clone().into_os_string();
    part.push(".part");
    let part_path = PathBuf::from(part);

    println!("Downloading: {} ({})", dl.name, format_size(dl.size, BINARY));

    // The download link is time-limited; refresh it on each retry.
    let mut link = dl.web_content_link.clone();
    let mut attempt: u32 = 0;
    loop {
        attempt += 1;
        match download_attempt(client, &link, dl.size, &part_path, show_progress).await {
            Ok(()) => break,
            Err(err) => {
                let retryable = matches!(err, DlError::Retryable(_));
                let e = err.into_inner();
                if retryable && attempt < MAX_DOWNLOAD_ATTEMPTS {
                    let delay = download_backoff(attempt - 1);
                    eprintln!(
                        "  {}: attempt {attempt} failed ({e:#}); retrying in {:.1}s",
                        dl.name,
                        delay.as_secs_f64()
                    );
                    tokio::time::sleep(delay).await;
                    if let Ok(fresh) = client.get_download_url(&file.id).await {
                        link = fresh.web_content_link;
                    }
                    continue;
                }
                return Err(e).with_context(|| format!("failed to download {}", dl.name));
            }
        }
    }

    tokio::fs::rename(&part_path, &file_path)
        .await
        .context("failed to finalize output file")?;
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
    let existing = tokio::fs::metadata(part_path)
        .await
        .map(|m| m.len())
        .unwrap_or(0);

    let mut request = client.http_client().get(link);
    if existing > 0 {
        request = request.header(reqwest::header::RANGE, format!("bytes={existing}-"));
    }

    let mut resp = request
        .send()
        .await
        .map_err(|e| classify(e, "download request failed"))?;
    let status = resp.status();

    let (mut out, mut downloaded) = if existing > 0
        && status == reqwest::StatusCode::PARTIAL_CONTENT
    {
        // Server honored the range: append to what we already have.
        let f = tokio::fs::OpenOptions::new()
            .append(true)
            .open(part_path)
            .await
            .map_err(|e| fatal(e, "failed to open partial file"))?;
        (f, existing)
    } else if existing > 0 && status == reqwest::StatusCode::RANGE_NOT_SATISFIABLE {
        // The part is at/past the full length. If the sizes agree it is
        // already complete; otherwise the part is stale/corrupt (bigger than a
        // now-shorter remote file). We cannot stream this 416 response — its
        // body is an error page, not file bytes — so truncate the part and let
        // the outer retry loop restart with a plain (no-Range) GET.
        if total > 0 && existing >= total {
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
        let retry =
            status.is_server_error() || status == reqwest::StatusCode::TOO_MANY_REQUESTS;
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
    Ok(())
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

/// Exponential backoff for download retry attempt `n` (0-based): 500ms, 1s, 2s,
/// ... capped at 15s.
fn download_backoff(attempt: u32) -> Duration {
    let ms = 500u64.saturating_mul(1u64 << attempt.min(5));
    Duration::from_millis(ms.min(15_000))
}

/// Recursively walk a remote folder, creating local directories and collecting
/// each file with its destination directory into `tasks`.
fn collect_folder<'a>(
    client: &'a Client,
    folder: &'a pikpak::FileInfo,
    output_dir: &'a StdPath,
    tasks: &'a mut Vec<(pikpak::FileInfo, PathBuf)>,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        let dir_path = output_dir.join(safe_component(&folder.name)?);
        tokio::fs::create_dir_all(&dir_path)
            .await
            .context("failed to create folder")?;

        println!("Scanning folder: {}", folder.name);

        let files: Vec<pikpak::FileInfo> = client.list_folder(&folder.id).await?;
        for f in files {
            if f.kind.is_folder() {
                collect_folder(client, &f, &dir_path, tasks).await?;
            } else {
                tasks.push((f, dir_path.clone()));
            }
        }
        Ok(())
    })
}

async fn cmd_quota(client: &Client, args: QuotaArgs) -> Result<()> {
    let q: pikpak::Quota = client.quota().await.context("quota failed")?;

    let fmt = |n: u64| {
        if args.raw {
            n.to_string()
        } else {
            format_size(n, BINARY)
        }
    };

    println!("total: {}", fmt(q.total));
    println!("used:  {}", fmt(q.used));
    println!("free:  {}", fmt(q.free()));
    if let Some(r) = q.ratio() {
        println!("usage: {:.1}%", r * 100.0);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::safe_component;

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

    use super::{download_backoff, update_env_token};

    #[test]
    fn update_env_rewrites_token_line() {
        let dir = std::env::temp_dir().join(format!("pikpak-env-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "PIKPAK_REFRESH_TOKEN=old\nPIKPAK_PROXY=http://x\n").unwrap();

        let changed = update_env_token(&env_path, "brand-new").unwrap();
        assert!(changed);

        let content = std::fs::read_to_string(&env_path).unwrap();
        assert!(content.contains("PIKPAK_REFRESH_TOKEN=brand-new"));
        assert!(content.contains("PIKPAK_PROXY=http://x"));
        assert!(!content.contains("=old"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn update_env_absent_file_is_noop() {
        let path = std::env::temp_dir().join("pikpak-does-not-exist-abc.env");
        let _ = std::fs::remove_file(&path);
        assert!(!update_env_token(&path, "x").unwrap());
    }

    #[test]
    fn update_env_without_key_returns_false() {
        let dir = std::env::temp_dir().join(format!("pikpak-env2-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let env_path = dir.join(".env");
        std::fs::write(&env_path, "PIKPAK_PROXY=http://x\n").unwrap();
        assert!(!update_env_token(&env_path, "x").unwrap());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn download_backoff_grows_and_caps() {
        assert!(download_backoff(0) < download_backoff(2));
        assert!(download_backoff(30) <= std::time::Duration::from_millis(15_000));
    }

    async fn spawn_range_server(full: Vec<u8>) -> std::net::SocketAddr {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
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
                    let mut buf = vec![0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    let req = String::from_utf8_lossy(&buf[..n]);
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
}
