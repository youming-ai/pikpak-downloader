[![License](https://img.shields.io/github/license/youming-ai/pikpak-downloader)](LICENSE)

A high-performance Rust command-line tool (CLI) and client library for **PikPak** cloud storage.

It provides robust support for listing files, checking account quota, and downloading files or directories recursively. It also features automatic captcha solving using MD5 signature generation algorithms ported from `pikpakcli`.

---

## Features

- **CLI & Library**: Use it as a stand-alone command-line tool or integrate it into your own Rust projects as a client library.
- **Recursive Downloads**: Effortlessly download files or entire directory trees, preserving original folder structures.
- **Auto-Captcha & Token Flow**: Implements the PikPak mobile client's captcha signature algorithms (`X-Captcha-Token`) and token rotation automatically under the hood. No manual captcha solving is required.
- **Proxy Support**: Connect via HTTP/HTTPS proxies.
- **Detailed File Info**: Rich file listing with options for detailed view (`-l`) and human-readable file sizes (`--human`).
- **Safe & Atomic Downloads**: Server-provided names are sanitized against path traversal, and each file is streamed to a temporary `.part` sibling that is renamed only once the transfer completes — an interrupted download never leaves a truncated file under its final name.
- **Resumable Downloads**: Interrupted transfers resume from the existing `.part` file via HTTP `Range` requests, and transient network / server errors are retried with exponential backoff — no re-downloading from scratch after a blip.
- **Concurrent Downloads**: Fetch many files in parallel with `-j/--jobs` when downloading a folder.
- **Token Rotation Persistence**: PikPak rotates the refresh token on each auth; the rotated value is written back to your `.env` automatically so stored credentials stay valid.

---

## Installation

### Build from Source

Ensure you have Rust and Cargo installed, then run:

```bash
# Clone the repository
git clone https://github.com/youming-ai/pikpak-downloader.git
cd pikpak-downloader

# Build the release binary
cargo build --release

# The compiled binary will be available at:
./target/release/pikpak --help
```

### Install to System Path

```bash
cargo install --path .
```

---

## Configuration

The application reads configuration from environment variables or a `.env` file. The file is looked up in the current working directory and then in its parents, so running the tool from a subdirectory still finds your project's `.env`. When PikPak rotates the refresh token, the new value is written back to the same file that was loaded.

To set it up:

```bash
# Copy the example environment file
cp .env.example .env
```

Open `.env` and fill in your details:

```env
# Required: Your PikPak refresh token
PIKPAK_REFRESH_TOKEN=your_refresh_token_here

# Optional: HTTP/HTTPS proxy URL (e.g., http://127.0.0.1:7890)
PIKPAK_PROXY=

# Optional: Custom OAuth Client ID and Secret if you wish to override defaults
PIKPAK_CLIENT_ID=
PIKPAK_CLIENT_SECRET=
```

### How to Get `PIKPAK_REFRESH_TOKEN`

1. Go to the [PikPak Web Client](https://mypikpak.com) and log in to your account.
2. Open your browser's Developer Tools (usually `F12` or right-click -> `Inspect`).
3. Navigate to the **Application** tab (Chrome/Edge) or **Storage** tab (Firefox).
4. Select **Local Storage** -> `https://mypikpak.com`.
5. Find the key named `credentials` or search for `refresh_token` in the values. It is a long alphanumeric string.

> **Caveat:** PikPak refresh tokens are single-use (every login rotates them, and each rotation invalidates your copy) and they are issued per client platform — this tool authenticates as the Android client, while the web app issues its own tokens. If a freshly copied token is rejected with `invalid_grant`, see [Troubleshooting](#troubleshooting).

---

## CLI Usage

Run `pikpak --help` to see all available commands and flags.

Every command accepts a global `--verbose` flag, which enables debug logging (useful for diagnosing auth, captcha and retry behaviour). It can go before or after the subcommand:

```bash
pikpak --verbose download --path "/My Pack/video.mp4"
```

### 1. View Quota

Display total, used, and free storage spaces.

```bash
# Human-readable format (default)
pikpak quota

# Output example:
# total: 10.00 TiB
# used:  4.23 TiB
# free:  5.77 TiB
# usage: 42.3%

# Raw byte counts
pikpak quota --raw
```

### 2. List Files & Folders

List files in a given directory path.

```bash
# List files in the root folder (/)
pikpak ls

# List files in a specific path
pikpak ls --path "/My Pack"

# Detailed list (-l) with human-readable file sizes (--human)
pikpak ls --path "/My Pack" -l --human
```

### 3. Download Files & Folders

Download a single file or an entire directory recursively.

```bash
# Download a file to the default folder (./downloads)
pikpak download --path "/My Pack/video.mp4"

# Download a directory recursively
pikpak download --path "/My Pack/Movies"

# Download a directory recursively with 4 concurrent transfers
pikpak download --path "/My Pack/Movies" --jobs 4

# Download to a custom output directory
pikpak download --path "/My Pack/video.mp4" --output "/path/to/local/dir"
```

---

## Library Usage (Rust API)

You can also use `pikpak` as a library crate. Add it to your `Cargo.toml` dependencies, or use a local path dependency.

```rust
use pikpak::{Client, FileKind};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Build the client
    let client = Client::builder()
        .refresh_token("YOUR_REFRESH_TOKEN")
        .timeout(Duration::from_secs(30))
        .proxy("http://127.0.0.1:7890") // Optional proxy
        .build()?;

    // 2. Query storage quota
    let quota = client.quota().await?;
    println!("Total quota: {} bytes, used: {} bytes", quota.total, quota.used);

    // 3. List files in the root directory
    let root_files = client.list_folder("").await?; // Root uses empty string
    for file in root_files {
        println!("- {} (Kind: {:?}, Size: {} bytes)", file.name, file.kind, file.size);
    }

    // 4. Resolve path and get download URL
    let path = "/My Pack/video.mp4";
    let file_info = client.resolve_path_info(path).await?;
    if file_info.kind.is_file() {
        let download_info = client.get_download_url(&file_info.id).await?;
        println!("Download URL: {}", download_info.web_content_link);
    }

    Ok(())
}
```

### Library notes

- **Which HTTP client to use.** `client.http_client()` carries the configured *total* request timeout, which is right for the small JSON API calls. To stream file content, use `client.download_client()` instead: it has no total deadline, only a per-read stall timeout, so a large file is never cut off mid-transfer.
- **Persisting a rotated refresh token.** PikPak invalidates the previous refresh token on every exchange. Register `ClientBuilder::on_refresh_token` to persist each new value the moment the server issues it, rather than when your program happens to exit:

  ```rust
  let client = Client::builder()
      .refresh_token(token)
      .on_refresh_token(|rotated| { /* write it back to your config */ })
      .build()?;
  ```

### Manual smoke test

The automated tests run entirely against local mock servers and never touch the
real service, so a few things can only be checked by hand with a real account:

1. `pikpak quota` prints plausible totals.
2. `pikpak ls --path "/"` and a nested path return the expected entries.
3. `pikpak download --path <single file>` writes the file into `./downloads`.
4. `pikpak download --path <folder> -j 4` mirrors the tree and every file
   completes.
5. Interrupt a large download (Ctrl-C) and run it again: it resumes instead of
   restarting, and the finished file matches the remote size.
6. Confirm a large download is **not** aborted at ~30s.
7. `pikpak --verbose ls` shows auth, captcha and retry activity.
8. After the first command, `PIKPAK_REFRESH_TOKEN` in `.env` has changed, and
   the next command still authenticates.

## Troubleshooting

### `invalid_grant` — "invalid refresh token ... refreshed by other process"

PikPak answers with this (error_code 4126) when it refuses the refresh token at
login. Two everyday causes look identical from the outside:

1. **The token was already used.** PikPak rotates the refresh token on *every*
   login — each exchange invalidates the previous value, so the copy you kept
   anywhere else is dead the moment something else refreshed it: the web client
   refreshing in the background, another copy of this tool running at the same
   time, or an earlier run of this tool whose replacement you did not save.

   - Keep the token in a `.env` file: every rotation is written back there as it
     happens, so the newest value is always the one on disk.
   - If you keep it in your shell environment instead, the tool can only *print*
     the replacement — watch for
     `note: refresh token rotated; set PIKPAK_REFRESH_TOKEN to: …`, and the
     end-of-run reminder that follows.
   - Don't use the web client (or a second copy of this tool) with the same
     account while the CLI works.

2. **The token was issued for a different client platform.** PikPak mints
   refresh tokens per login platform, and this tool authenticates as the
   Android client. A freshly copied web-app token can be rejected outright with
   this same error (see
   [#2](https://github.com/youming-ai/pikpak-downloader/issues/2)). If a
   brand-new token fails immediately and nothing else has touched the account,
   log out of the web app and back in, then copy a fresh token; if it keeps
   failing, that web session's tokens may not be refreshable by this client at
   all.

---

## License

This project is licensed under the MIT License. See the [LICENSE](LICENSE) file for details.

