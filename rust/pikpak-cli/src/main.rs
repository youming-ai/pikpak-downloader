//! `pikpak-cli` — CLI front-end for the `pikpak-api` crate.

use std::path::Path as StdPath;
use std::process::ExitCode;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use humansize::{format_size, BINARY};
use pikpak_api::{Client, FileKind};
use tokio::io::AsyncWriteExt;
use tracing_subscriber::EnvFilter;

#[derive(Debug, Parser)]
#[command(
    name = "pikpak-cli",
    version,
    about = "PikPak Personal Cloud Storage Management Tool",
    long_about = None
)]
struct Cli {
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List files and folders under a given path.
    Ls(LsArgs),
    /// Download a file or folder from PikPak.
    Download(DownloadArgs),
    /// Show storage quota for the account.
    Quota(QuotaArgs),
    /// Show help information.
    Help,
}

#[derive(Debug, Parser)]
struct LsArgs {
    /// Directory path to list (e.g. "/My Pack"). Defaults to root.
    #[arg(long, default_value = "/")]
    path: String,

    /// Show a long-format listing with size and kind.
    #[arg(short = 'l', long)]
    long: bool,

    /// Render sizes in human-readable units (KB/MB/GB).
    #[arg(short = 'h', long)]
    human: bool,
}

#[derive(Debug, Parser)]
struct DownloadArgs {
    /// Remote path to download (file or folder), e.g. "/My Pack/video.mp4".
    #[arg(long)]
    path: String,

    /// Local output directory.
    #[arg(long, default_value = "./downloads")]
    output: String,

    /// Number of concurrent downloads.
    #[arg(long, default_value = "3")]
    count: usize,
}

#[derive(Debug, Parser)]
struct QuotaArgs {
    /// Print raw byte counts instead of human-readable units.
    #[arg(long)]
    raw: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    let _ = dotenvy::dotenv();

    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("pikpak_cli=debug,pikpak_api=debug")
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
    match cli.command {
        Command::Help => {
            print_help();
            Ok(())
        }
        _ => {
            let client = build_client()?;
            match cli.command {
                Command::Ls(args) => cmd_ls(&client, args).await,
                Command::Download(args) => cmd_download(&client, args).await,
                Command::Quota(args) => cmd_quota(&client, args).await,
                Command::Help => unreachable!(),
            }
        }
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
            builder =
                builder.credentials(pikpak_api::auth::OAuthCredentials::new(id, secret));
        }
    }

    builder.build().context("failed to build API client")
}

async fn cmd_ls(client: &Client, args: LsArgs) -> Result<()> {
    let parent_id = client.resolve_path(&args.path).await?;

    let files = client
        .list_folder(&parent_id)
        .await
        .context("list_folder failed")?;

    if files.is_empty() {
        println!("Directory is empty");
        return Ok(());
    }

    if args.long {
        println!("{:<10} {:>12} {}", "Type", "Size", "Name");
        println!("{}", "-".repeat(50));
        for f in &files {
            let kind = if f.kind.is_folder() { "Folder" } else { "File" };
            let size = if args.human {
                format_size(f.size, BINARY)
            } else {
                f.size.to_string()
            };
            println!("{kind:<10} {size:>12} {name}", name = f.name);
        }
    } else {
        for f in &files {
            let marker = if f.kind == FileKind::Folder { "/" } else { "" };
            println!("{}{marker}", f.name);
        }
    }

    Ok(())
}

async fn cmd_download(client: &Client, args: DownloadArgs) -> Result<()> {
    tokio::fs::create_dir_all(&args.output)
        .await
        .context("failed to create output directory")?;

    let info = client.resolve_path_info(&args.path).await?;

    if info.kind.is_folder() {
        download_folder(client, &info, &args.output).await
    } else {
        download_file(client, &info, &args.output).await
    }
}

async fn download_file(client: &Client, file: &pikpak_api::FileInfo, output_dir: &str) -> Result<()> {
    let dl = client
        .get_download_url(&file.id)
        .await
        .context("failed to get download URL")?;

    println!("Downloading: {}", dl.name);
    println!("Size: {}", format_size(dl.size, BINARY));

    let file_path = StdPath::new(output_dir).join(&dl.name);
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

    let mut out = tokio::fs::File::create(&file_path)
        .await
        .context("failed to create output file")?;

    let mut downloaded: u64 = 0;
    loop {
        let n = resp.chunk().await?;
        match n {
            Some(chunk) => {
                out.write_all(&chunk).await?;
                downloaded += chunk.len() as u64;
                if dl.size > 0 {
                    let pct = (downloaded as f64 / dl.size as f64) * 100.0;
                    eprint!("\r  {} / {} ({:.1}%)", format_size(downloaded, BINARY), format_size(dl.size, BINARY), pct);
                }
            }
            None => break,
        }
    }
    eprintln!();

    println!("Saved: {}", file_path.display());
    Ok(())
}

fn download_folder<'a>(
    client: &'a Client,
    folder: &'a pikpak_api::FileInfo,
    output_dir: &'a str,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + 'a>> {
    Box::pin(async move {
        let dir_path = StdPath::new(output_dir).join(&folder.name);
        tokio::fs::create_dir_all(&dir_path)
            .await
            .context("failed to create folder")?;

        println!("Downloading folder: {}", folder.name);

        let files = client.list_folder(&folder.id).await?;
        for f in &files {
            if f.kind.is_folder() {
                download_folder(client, f, dir_path.to_str().unwrap()).await?;
            } else {
                download_file(client, f, dir_path.to_str().unwrap()).await?;
            }
        }

        Ok(())
    })
}

async fn cmd_quota(client: &Client, args: QuotaArgs) -> Result<()> {
    let q = client.quota().await.context("quota failed")?;

    let fmt = |n: u64| {
        if args.raw {
            n.to_string()
        } else {
            format_size(n, BINARY)
        }
    };

    println!("Cloud storage quota:");
    println!("  Total capacity: {}", fmt(q.total));
    println!("  Used:           {}", fmt(q.used));
    println!("  Free:           {}", fmt(q.free()));
    if let Some(r) = q.ratio() {
        println!("  Usage rate:     {:.1}%", r * 100.0);
    }

    Ok(())
}

fn print_help() {
    println!("PikPak Personal Cloud Storage Management Tool");
    println!();
    println!("Usage: pikpak-cli <command> [options]");
    println!();
    println!("Available commands:");
    println!("  ls        List files and directories");
    println!("  download  Download files or folders");
    println!("  quota     View cloud storage quota");
    println!("  help      Show this help information");
    println!();
    println!("Command details:");
    println!();
    println!("ls - List files and directories");
    println!("  Options:");
    println!("    --path string   Directory path (default: \"/\")");
    println!("    -l              Long format display");
    println!("    -h              Human readable format");
    println!("  Examples:");
    println!("    pikpak-cli ls");
    println!(r#"    pikpak-cli ls --path "/My Pack" -l -h"#);
    println!();
    println!("download - Download files or folders");
    println!("  Options:");
    println!(r#"    --path string   Download path (e.g. "/My Pack/video.mp4")"#);
    println!("    --output string Output directory (default: \"./downloads\")");
    println!("    --count int     Concurrency count (default: 3)");
    println!("  Examples:");
    println!(r#"    pikpak-cli download --path "/My Pack/video.mp4""#);
    println!(r#"    pikpak-cli download --path "/My Pack" --output "./my_downloads""#);
    println!();
    println!("quota - View cloud storage quota");
    println!("  Options:");
    println!("    --raw           Print raw byte counts");
    println!("  Examples:");
    println!("    pikpak-cli quota");
    println!();
    println!("Configuration:");
    println!("  Configure PikPak authentication in .env file:");
    println!("    PIKPAK_REFRESH_TOKEN=your_refresh_token");
    println!();
}
