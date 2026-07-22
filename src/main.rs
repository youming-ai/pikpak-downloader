use std::future::Future;
use std::path::Path as StdPath;
use std::pin::Pin;
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use humansize::{format_size, BINARY};
use tokio::io::AsyncWriteExt;
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
    let client = build_client()?;
    match cli.command {
        Command::Ls(args) => cmd_ls(&client, args).await,
        Command::Download(args) => cmd_download(&client, args).await,
        Command::Quota(args) => cmd_quota(&client, args).await,
    }
}

fn build_client() -> Result<Client> {
    let refresh_token = std::env::var("PIKPAK_REFRESH_TOKEN")
        .context("PIKPAK_REFRESH_TOKEN must be set in the environment or .env")?;

    let mut builder = Client::builder().refresh_token(refresh_token);

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

    builder.build().context("failed to build API client")
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

    if info.kind.is_folder() {
        download_folder(client, &info, output).await
    } else {
        download_file(client, &info, output).await
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

async fn download_file(
    client: &Client,
    file: &pikpak::FileInfo,
    output_dir: &StdPath,
) -> Result<()> {
    let dl: pikpak::DownloadInfo = client
        .get_download_url(&file.id)
        .await
        .context("failed to get download URL")?;

    println!("Downloading: {}", dl.name);
    println!("Size: {}", format_size(dl.size, BINARY));

    let file_path = output_dir.join(safe_component(&dl.name)?);
    let mut resp = client
        .http_client()
        .get(&dl.web_content_link)
        .send()
        .await
        .context("download request failed")?;

    if !resp.status().is_success() {
        bail!(
            "download failed with status {}: {}",
            resp.status(),
            resp.text().await.unwrap_or_default()
        );
    }

    // Stream to a `.part` sibling and rename on success so an interrupted
    // download never leaves a truncated file under its final name.
    let mut part_path = file_path.clone().into_os_string();
    part_path.push(".part");
    let part_path = std::path::PathBuf::from(part_path);

    let mut out = tokio::fs::File::create(&part_path)
        .await
        .context("failed to create output file")?;

    let mut downloaded: u64 = 0;
    while let Some(chunk) = resp.chunk().await? {
        out.write_all(&chunk).await?;
        downloaded += chunk.len() as u64;
        if dl.size > 0 {
            let pct = (downloaded as f64 / dl.size as f64) * 100.0;
            eprint!(
                "\r  {} / {} ({:.1}%)",
                format_size(downloaded, BINARY),
                format_size(dl.size, BINARY),
                pct
            );
        }
    }
    eprintln!();

    out.flush().await.context("failed to flush output file")?;
    tokio::fs::rename(&part_path, &file_path)
        .await
        .context("failed to finalize output file")?;

    println!("Saved: {}", file_path.display());
    Ok(())
}

fn download_folder<'a>(
    client: &'a Client,
    folder: &'a pikpak::FileInfo,
    output_dir: &'a StdPath,
) -> Pin<Box<dyn Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        let dir_path = output_dir.join(safe_component(&folder.name)?);
        tokio::fs::create_dir_all(&dir_path)
            .await
            .context("failed to create folder")?;

        println!("Downloading folder: {}", folder.name);

        let files: Vec<pikpak::FileInfo> = client.list_folder(&folder.id).await?;
        for f in &files {
            if f.kind.is_folder() {
                download_folder(client, f, &dir_path).await?;
            } else {
                download_file(client, f, &dir_path).await?;
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
}
