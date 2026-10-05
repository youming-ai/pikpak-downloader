use std::path::{Path as StdPath, PathBuf};
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use humansize::{format_size, BINARY};
use tracing_subscriber::EnvFilter;

use pikpak::auth::OAuthCredentials;
use pikpak::{Client, FileKind};
use serde::Serialize;

mod download;
mod env_file;
mod jobs;
mod output;

#[derive(Debug, Parser)]
#[command(
    name = "pikpak",
    version,
    about = "PikPak cloud storage CLI",
    after_help = "Config via .env or environment:\n  PIKPAK_REFRESH_TOKEN  (required) refresh token from the web UI\n  PIKPAK_PROXY          (optional) HTTP(S) proxy URL\n  PIKPAK_CLIENT_ID      (optional) override OAuth client id\n  PIKPAK_CLIENT_SECRET  (optional) override OAuth client secret\n  PIKPAK_DEVICE_ID      (optional) device id; saved on first run to the .env\n                        the token came from"
)]
struct Cli {
    #[arg(long, global = true)]
    verbose: bool,

    /// Write one JSON document to stdout: the result, or the failure.
    ///
    /// Narration and progress go to stderr either way, so stdout stays
    /// parseable. Shapes and exit codes are documented in AGENTS.md.
    #[arg(long, global = true)]
    json: bool,

    /// Do not draw byte-level transfer progress.
    #[arg(long, global = true)]
    no_progress: bool,

    /// Internal: the detached job this process is the worker for.
    ///
    /// Set by `download --detach` when it re-executes this binary. Not part of
    /// the interface.
    #[arg(long, hide = true, global = true)]
    internal_job: Option<String>,

    /// Internal: state directory the worker keeps its record in.
    #[arg(long, hide = true, global = true)]
    internal_state: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

impl Command {
    /// The stable name this command reports in JSON documents.
    fn name(&self) -> &'static str {
        match self {
            Command::Ls(_) => "ls",
            Command::Download(_) => "download",
            Command::Quota(_) => "quota",
            Command::Jobs(args) => args.command.name(),
        }
    }
}

#[derive(Debug, Subcommand)]
enum Command {
    /// List files and directories.
    Ls(LsArgs),
    /// Download files or folders.
    Download(DownloadArgs),
    /// View storage quota.
    Quota(QuotaArgs),
    /// Inspect or control detached downloads.
    Jobs(jobs::JobsArgs),
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
    /// Return immediately with a job id instead of waiting for the transfer.
    ///
    /// The job keeps running after this process exits; follow it with
    /// `pikpak jobs status|wait|cancel`.
    #[arg(long)]
    detach: bool,
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

    let state = jobs::state_dir(loaded_env.as_deref());

    let cli = Cli::parse();
    let output = output::Output::new(cli.json, cli.no_progress);
    let command = cli.command.name();

    // A panic is a bug, but still a failure the caller must be able to parse:
    // one failure document and the documented exit 1, not Rust's bare 101. The
    // process ends here, so no second document can follow from `main`.
    //
    // Only on the main thread, where the command itself runs. A panic in a
    // spawned task surfaces through its join handle — one failed file in a
    // download — and must not take the rest of the run down with it.
    let default_hook = std::panic::take_hook();
    let main_thread = std::thread::current().id();
    std::panic::set_hook(Box::new(move |info| {
        default_hook(info);
        if std::thread::current().id() != main_thread {
            return;
        }
        output.fail(command, &anyhow::anyhow!("internal error: {info}"));
        std::process::exit(output::EXIT_UNEXPECTED.into());
    }));

    let filter = if cli.verbose {
        EnvFilter::new("pikpak=debug")
    } else {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"))
    };
    // Logs are narration: on stdout they would corrupt the one document a
    // caller parses.
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    match run(cli, env_path, &state, output).await {
        Ok(()) => ExitCode::from(output::EXIT_OK),
        Err(e) => {
            // The human line always goes to stderr; the JSON document is what a
            // caller parses, and carries the same code the process exits with.
            eprintln!("error: {e:#}");
            output.fail(command, &e);
            ExitCode::from(output::exit_code_for(&e).0)
        }
    }
}

async fn run(
    cli: Cli,
    env_path: Option<PathBuf>,
    state: &StdPath,
    output: output::Output,
) -> Result<()> {
    // Set by the rotation hook when a rotation happened that could not be
    // written anywhere, so the run can end with a nudge about it.
    let rotated_unpersisted = Arc::new(AtomicBool::new(false));
    // Built per command rather than up front: `jobs` needs no credentials, and
    // asking what is already running should not fail because the token has since
    // been rotated away.
    let client = || build_client(env_path.as_deref(), rotated_unpersisted.clone());
    let result = match cli.command {
        Command::Ls(args) => cmd_ls(&client()?, args, output).await,
        Command::Download(args) => match (&cli.internal_job, args.detach) {
            // This process *is* the worker for a detached job.
            (Some(id), _) => jobs::run_worker_download(
                &client()?,
                args,
                output,
                id.clone(),
                // The worker's parent always passes its own state directory.
                cli.internal_state.as_deref().unwrap_or(state),
            )
            .await
            .map(|_| ()),
            // Fail here rather than start a job that cannot authenticate.
            (None, true) => {
                validate_refresh_token(std::env::var("PIKPAK_REFRESH_TOKEN").ok())?;
                jobs::start_detached(&args, output, env_path.is_some(), state).map(|_| ())
            }
            (None, false) => download::cmd_download(&client()?, args, output)
                .await
                .map(|_| ()),
        },
        Command::Quota(args) => cmd_quota(&client()?, args, output).await,
        Command::Jobs(args) => jobs::cmd_jobs(args.command, output, state).await,
    };
    if let Some(note) = rotation_reminder(rotated_unpersisted.load(Ordering::SeqCst)) {
        eprintln!("{note}");
    }
    result
}

/// The end-of-run nudge for a rotation that could not be persisted anywhere.
///
/// The rotation hook already printed the replacement token when it happened;
/// this is the last line for a user whose terminal has scrolled past it. Only
/// the environment-sourced case needs it — with a `.env`, the rotation is
/// persisted as it happens.
fn rotation_reminder(unpersisted_rotation: bool) -> Option<&'static str> {
    unpersisted_rotation.then_some(
        "PIKPAK_REFRESH_TOKEN came from your environment, so the rotated replacement printed \
         above could not be saved automatically. Update your environment with it — or put the \
         token in a .env file, where every rotation is persisted for you.",
    )
}

/// Validate the configured refresh token.
///
/// A present-but-empty value is a configuration mistake worth naming: sending it
/// would come back as an opaque auth failure from the server.
fn validate_refresh_token(value: Option<String>) -> Result<String> {
    match value {
        Some(token) if !token.trim().is_empty() => Ok(token),
        // A missing or blank token is a configuration failure, not a mystery:
        // report it as such so a caller gets the `auth` exit code and can tell
        // "fix your credentials" from "the service broke".
        Some(_) => {
            // Reads as "not configured: missing PIKPAK_REFRESH_TOKEN (set, but blank)".
            Err(pikpak::Error::NotConfigured("PIKPAK_REFRESH_TOKEN (set, but blank)").into())
        }
        None => Err(pikpak::Error::NotConfigured("PIKPAK_REFRESH_TOKEN").into()),
    }
}

fn build_client(
    env_path: Option<&StdPath>,
    rotated_unpersisted: Arc<AtomicBool>,
) -> Result<Client> {
    let refresh_token = validate_refresh_token(std::env::var("PIKPAK_REFRESH_TOKEN").ok())?;

    let device_id = env_file::device_id(env_path, &refresh_token);
    let env_path = env_path.map(StdPath::to_path_buf);
    let source_path = env_path.clone();
    let mut builder = Client::builder()
        .refresh_token(refresh_token)
        // Persist the moment the server rotates, not when the command ends: a
        // long download can be interrupted at any point, and by then the
        // previous refresh token is already invalid server-side.
        .on_refresh_token(move |token| {
            if env_path.is_none() {
                rotated_unpersisted.store(true, Ordering::SeqCst);
            }
            env_file::persist_rotated_token(env_path.as_deref(), token);
        })
        // If another process rotated the token first, the replacement it saved is
        // the one that still works — pick it up rather than failing the run.
        .refresh_token_source(move || env_file::read_env_token(source_path.as_deref()));

    if let Some(id) = device_id {
        builder = builder.device_id(id);
    }

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

/// One entry as it appears in `--json` output.
#[derive(Debug, Serialize)]
struct LsEntry<'a> {
    id: &'a str,
    name: &'a str,
    /// `"file"`, `"folder"` or `"unknown"` — the server's own notion of kind.
    kind: &'static str,
    size: u64,
    modified_time: Option<&'a str>,
    mime_type: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct LsResult<'a> {
    path: &'a str,
    entries: Vec<LsEntry<'a>>,
}

async fn cmd_ls(client: &Client, args: LsArgs, output: output::Output) -> Result<()> {
    let files: Vec<pikpak::FileInfo> = if pikpak::is_drive_root(&args.path) {
        client.list_folder("").await.context("list_folder failed")?
    } else {
        let info = client.resolve_path_info(&args.path).await?;
        if info.kind.is_folder() {
            client
                .list_folder(&info.id)
                .await
                .context("list_folder failed")?
        } else {
            vec![info]
        }
    };

    if !output.json() {
        if files.is_empty() {
            println!("(empty)");
        } else if args.long {
            println!("{:<10} {:>12} name", "kind", "size");
            println!("{}", "-".repeat(50));
            for f in &files {
                let kind = f.kind.label();
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
    }

    let entries = files
        .iter()
        .map(|f| LsEntry {
            id: &f.id,
            name: &f.name,
            kind: f.kind.label(),
            size: f.size,
            modified_time: f.modified_time.as_deref(),
            mime_type: f.mime_type.as_deref(),
        })
        .collect();
    output.ok(
        "ls",
        &LsResult {
            path: &args.path,
            entries,
        },
    )
}

/// The quota as it appears in `--json` output. Always bytes: `--raw` and the
/// human formatting only shape the text a person reads.
#[derive(Debug, Serialize)]
struct QuotaResult {
    total: u64,
    used: u64,
    free: u64,
    usage_percent: Option<f64>,
}

async fn cmd_quota(client: &Client, args: QuotaArgs, output: output::Output) -> Result<()> {
    let q: pikpak::Quota = client.quota().await.context("quota failed")?;

    if !output.json() {
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
    }

    output.ok(
        "quota",
        &QuotaResult {
            total: q.total,
            used: q.used,
            free: q.free(),
            usage_percent: q.ratio().map(|r| r * 100.0),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `--json` shapes are a public contract (see `AGENTS.md`): these pin the
    /// field names a caller parses.
    #[test]
    fn the_json_shapes_keep_their_field_names() {
        let ls = serde_json::to_value(LsResult {
            path: "/My Pack",
            entries: vec![LsEntry {
                id: "f1",
                name: "video.mp4",
                kind: "file",
                size: 123,
                modified_time: Some("2026-01-02T03:04:05Z"),
                mime_type: None,
            }],
        })
        .unwrap();
        assert_eq!(ls["path"], "/My Pack");
        assert_eq!(ls["entries"][0]["id"], "f1");
        assert_eq!(ls["entries"][0]["kind"], "file");
        assert_eq!(ls["entries"][0]["size"], 123);
        assert_eq!(ls["entries"][0]["modified_time"], "2026-01-02T03:04:05Z");
        assert!(ls["entries"][0]["mime_type"].is_null());

        let quota = serde_json::to_value(QuotaResult {
            total: 100,
            used: 40,
            free: 60,
            usage_percent: Some(40.0),
        })
        .unwrap();
        assert_eq!(quota["total"], 100);
        assert_eq!(quota["used"], 40);
        assert_eq!(quota["free"], 60);
        assert_eq!(quota["usage_percent"], 40.0);

        // A quota that cannot be expressed as a ratio still reports the bytes.
        let unknown = serde_json::to_value(QuotaResult {
            total: 0,
            used: 0,
            free: 0,
            usage_percent: None,
        })
        .unwrap();
        assert!(unknown["usage_percent"].is_null());
    }

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

    /// The contract an agent branches on: a credentials problem is `auth`, not
    /// "something went wrong". This is what the CLI actually reports before any
    /// HTTP call happens.
    #[test]
    fn missing_credentials_are_reported_as_an_auth_failure() {
        for problem in [None, Some(String::new()), Some("  ".to_string())] {
            let error = super::validate_refresh_token(problem)
                .expect_err("a blank token must not be accepted");
            assert_eq!(
                crate::output::exit_code_for(&error),
                (crate::output::EXIT_AUTH, "auth")
            );
        }
    }

    #[test]
    fn rotation_reminder_only_fires_when_nothing_was_persisted() {
        let note = super::rotation_reminder(true).expect("must remind");
        assert!(note.contains("could not be saved automatically"), "{note}");
        assert!(note.contains(".env"), "{note}");
        assert!(
            super::rotation_reminder(false).is_none(),
            "a persisted rotation (the .env case) must not nag"
        );
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
