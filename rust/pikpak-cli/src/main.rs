//! `pikpak-cli` — CLI front-end for the `pikpak-api` crate.

use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use humansize::{format_size, BINARY};
use pikpak_api::{Client, FileKind};
use tracing_subscriber::EnvFilter;

/// CLI for managing PikPak personal cloud storage.
#[derive(Debug, Parser)]
#[command(version, about, long_about = None)]
struct Cli {
    /// Enable verbose (debug) logging.
    #[arg(long, global = true)]
    verbose: bool,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List files and folders under a given path.
    Ls(LsArgs),
    /// Show storage quota for the account.
    Quota(QuotaArgs),
}

#[derive(Debug, Parser)]
struct LsArgs {
    /// Folder id to list. Use "" (empty) or omit for the root.
    #[arg(long, default_value = "")]
    parent_id: String,

    /// Show a long-format listing with size and kind.
    #[arg(short = 'l', long)]
    long: bool,

    /// Render sizes in human-readable units (KB/MB/GB).
    #[arg(short = 'H', long)]
    human: bool,
}

#[derive(Debug, Parser)]
struct QuotaArgs {
    /// Print raw byte counts instead of human-readable units.
    #[arg(long)]
    raw: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    // Load .env before parsing, so env-backed clap args can see values.
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
    let client = build_client()?;

    match cli.command {
        Command::Ls(args) => cmd_ls(&client, args).await,
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
            builder =
                builder.credentials(pikpak_api::auth::OAuthCredentials::new(id, secret));
        }
    }

    builder.build().context("failed to build API client")
}

async fn cmd_ls(client: &Client, args: LsArgs) -> Result<()> {
    let files = client
        .list_folder(&args.parent_id)
        .await
        .context("list_folder failed")?;

    if files.is_empty() {
        println!("(empty)");
        return Ok(());
    }

    if args.long {
        println!("{:<8} {:>12} name", "kind", "size");
        println!("{}", "-".repeat(50));
        for f in &files {
            let kind = if f.kind.is_folder() { "folder" } else { "file" };
            let size = if args.human {
                format_size(f.size, BINARY)
            } else {
                f.size.to_string()
            };
            println!("{kind:<8} {size:>12} {name}", name = f.name);
        }
    } else {
        for f in &files {
            let marker = if f.kind == FileKind::Folder { "/" } else { "" };
            println!("{}{marker}", f.name);
        }
    }

    Ok(())
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

    println!("total: {}", fmt(q.total));
    println!("used:  {}", fmt(q.used));
    println!("free:  {}", fmt(q.free()));
    if let Some(r) = q.ratio() {
        println!("usage: {:.1}%", r * 100.0);
    }

    Ok(())
}
