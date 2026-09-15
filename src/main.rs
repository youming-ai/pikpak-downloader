use std::path::{Path as StdPath, PathBuf};
use std::process::ExitCode;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use humansize::{format_size, BINARY};
use tracing_subscriber::EnvFilter;

use pikpak::auth::OAuthCredentials;
use pikpak::{Client, FileKind};

mod download;
mod env_file;

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
    /// Format sizes in the detailed listing as human-readable units.
    // Deliberately long-only: clap reserves `-h` for `--help`, and declaring it
    // here makes argument parsing panic in debug builds.
    #[arg(long)]
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
    // Record whether the token came from the real environment *before* dotenvy
    // can supply it from a file: only the file-loaded case can be written back.
    // dotenvy never overrides variables already present, so a token taken from
    // the environment must not be persisted into an unrelated `.env`.
    let token_from_env = std::env::var_os("PIKPAK_REFRESH_TOKEN").is_some();
    let loaded_env = dotenvy::dotenv().ok();
    let env_path = if token_from_env {
        None
    } else {
        env_file::env_file_for_persistence(loaded_env.as_deref(), StdPath::new(".env"))
    };

    let cli = Cli::parse();

    let filter = if cli.verbose {
        EnvFilter::new("pikpak=debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    tracing_subscriber::fmt().with_env_filter(filter).init();

    match run(cli, env_path).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e:#}");
            ExitCode::FAILURE
        }
    }
}

async fn run(cli: Cli, env_path: Option<PathBuf>) -> Result<()> {
    let client = build_client(env_path.as_deref())?;
    match cli.command {
        Command::Ls(args) => cmd_ls(&client, args).await,
        Command::Download(args) => download::cmd_download(&client, args).await,
        Command::Quota(args) => cmd_quota(&client, args).await,
    }
}

/// Validate the configured refresh token.
///
/// A present-but-empty value is a configuration mistake worth naming: sending it
/// would come back as an opaque auth failure from the server.
fn validate_refresh_token(value: Option<String>) -> Result<String> {
    match value {
        Some(token) if !token.trim().is_empty() => Ok(token),
        Some(_) => bail!("PIKPAK_REFRESH_TOKEN is set but empty; give it a value or unset it"),
        None => bail!("PIKPAK_REFRESH_TOKEN must be set in the environment or .env"),
    }
}

fn build_client(env_path: Option<&StdPath>) -> Result<Client> {
    let refresh_token = validate_refresh_token(std::env::var("PIKPAK_REFRESH_TOKEN").ok())?;

    let env_path = env_path.map(StdPath::to_path_buf);
    let mut builder = Client::builder()
        .refresh_token(refresh_token)
        // Persist the moment the server rotates, not when the command ends: a
        // long download can be interrupted at any point, and by then the
        // previous refresh token is already invalid server-side.
        .on_refresh_token(move |token| env_file::persist_rotated_token(env_path.as_deref(), token));

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
    #[test]
    fn refresh_token_validation_rejects_empty_values() {
        assert_eq!(
            super::validate_refresh_token(Some("token".to_string())).unwrap(),
            "token"
        );
        // Present but empty (or whitespace) is a configuration mistake, not a
        // request to authenticate with nothing.
        assert!(super::validate_refresh_token(Some(String::new())).is_err());
        assert!(super::validate_refresh_token(Some("   ".to_string())).is_err());
        assert!(super::validate_refresh_token(None).is_err());
    }

    #[test]
    fn cli_definition_is_valid() {
        // clap's own validation. Without this, a duplicate short flag (e.g.
        // `-h` for `--human` colliding with `--help`) only shows up as a panic
        // in debug builds and is silently accepted in release.
        use clap::CommandFactory;
        super::Cli::command().debug_assert();
    }
}
