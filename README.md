# pikpak

A PikPak CLI and Rust library for listing files, checking storage, and downloading
files or folders. Supports concurrent downloads, automatic retries, resumable
transfers, and background jobs.

## Install

Download a binary for Linux or macOS from
[Releases](https://github.com/youming-ai/pikpak-downloader/releases).
Each release includes `SHA256SUMS` for verification.

Or install from source with Rust 1.86+:

```bash
cargo install --locked --git https://github.com/youming-ai/pikpak-downloader
```

## Setup

1. Sign in at [mypikpak.com](https://mypikpak.com). In browser DevTools, open
   **Application → Local Storage → https://mypikpak.com** and copy
   `refresh_token` from `credentials`.
2. Close the tab without logging out. The web app can rotate the token while open.
3. Save the token in a private `.env` in your working directory:

```bash
# For a new .env; prompts without echoing the token (bash/zsh).
printf 'Refresh token: '; read -rs token; echo
(umask 077; printf 'PIKPAK_REFRESH_TOKEN=%s\n' "$token" > .env); unset token
pikpak quota
```

The CLI loads the first `.env` in the current directory or its parents.
Tokens are single-use: authentication rotates them, and the CLI saves replacements
to that file. Keep the token in `.env`; environment-only tokens cannot be saved.
Use one client at a time. If authentication fails with `invalid_grant`, obtain a
fresh token. This tool uses the Android client, which can reject web-issued tokens.

Optional settings: `PIKPAK_PROXY`, `PIKPAK_DEVICE_ID`, `PIKPAK_CLIENT_ID`,
`PIKPAK_CLIENT_SECRET`, and `PIKPAK_STATE_DIR`. A device ID is saved automatically
when using a `.env`. See `.env.example` for a template.

## Usage

```bash
pikpak quota
pikpak ls --path "/My Pack" -l --human
pikpak download --path "/My Pack/video.mp4" --output ./downloads
pikpak download --path "/My Pack/Movies" --output ./downloads -j 4
```

Output defaults to `./downloads`; `--path /` downloads the drive root.
Run the same command again to resume an interrupted transfer. Non-empty local
files matching the listed size are skipped; empty files are fetched again.
Partials stay in `<output>/.pikpak-parts/` and resume only when their remote
identity still matches. Names are sanitized and completed files are renamed
into place after transfer.

For a background download:

```bash
id=$(pikpak download --detach --path "/My Pack/Movies" --output ./downloads)
pikpak jobs status "$id"
pikpak jobs wait "$id" --timeout-seconds 1800
pikpak jobs cancel "$id"
pikpak jobs list
```

Jobs and logs live in `.pikpak` beside the loaded `.env`, or in the current
directory if none is loaded. Set `PIKPAK_STATE_DIR` to override this location.
Cancellation preserves partials for the next run.

Use `--help` on any command for options, `--verbose` for debug logs, and
`--no-progress` to disable progress. Scripts and agents should use `--json`;
see [AGENTS.md](AGENTS.md) for output shapes and exit codes.

## Rust library

Use `default-features = false` to omit CLI dependencies.

```rust,no_run
use pikpak::Client;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client = Client::builder()
        .refresh_token("YOUR_REFRESH_TOKEN")
        .build()?;
    let quota = client.quota().await?;
    println!("Used: {} / {} bytes", quota.used, quota.total);

    let file = client.resolve_path_info("/My Pack/video.mp4").await?;
    client.download_file(
        &file,
        std::path::Path::new("video.mp4"),
        std::path::Path::new("video.mp4.part"),
        &|_done, _total| {},
    ).await?;
    Ok(())
}
```

Library users persist rotated tokens with `ClientBuilder::on_refresh_token` and
reload external rotations with `ClientBuilder::refresh_token_source`.
Downloads use a stall timeout rather than a total transfer deadline.

## License

[MIT](LICENSE)
