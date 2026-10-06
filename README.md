[![License](https://img.shields.io/github/license/youming-ai/pikpak-downloader)](LICENSE)
![rustc 1.86+](https://img.shields.io/badge/rustc-1.86%2B-blue)

PikPak from the terminal. Listing, quota and downloading, as a CLI or a Rust
library. Captcha signing and refresh-token rotation are handled for you.

- **Resumable + concurrent.** `.part` + `Range` — resumed only while the partial
  still matches the remote file (id/size/mtime) — jittered backoff, `-j N` trees,
  and a transfer is never finalized short of the reported size.
- **Defensive.** Server names are sanitized (traversal, `..`, Windows `CON`/`NUL`)
  and each file is renamed into place only once it is complete.
- **Automatic.** Captcha and token rotation; rotated tokens are written back to
  your `.env`, and one another process already rotated is recovered.

## Install

Prebuilt for Linux (x86_64, aarch64; static) and macOS (Apple silicon, Intel):

```bash
curl -fsSL "https://github.com/youming-ai/pikpak-downloader/releases/latest/download/pikpak-$(uname -m | sed s/arm64/aarch64/)-$(case $(uname -s) in Darwin) echo apple-darwin;; *) echo unknown-linux-musl;; esac).tar.gz" | sudo tar -xzC /usr/local/bin pikpak
pikpak --help
```

Checksums are in each release's `SHA256SUMS`. Or build from source, with Rust
1.86 or newer (`rust-version` in `Cargo.toml`, pinned by the CI MSRV job):

```bash
cargo install --locked --git https://github.com/youming-ai/pikpak-downloader
# or, in a clone: cargo install --locked --path .
```

## Get started

**1. Copy your refresh token.** Log in at [mypikpak.com](https://mypikpak.com),
open DevTools (F12), go to **Application** → **Local Storage** →
`https://mypikpak.com`, and copy `refresh_token` from the `credentials` entry.
Then log out of the web app: it refreshes the token in the background, which
would make your copy stale.

**2. Put it in a `.env`** in the directory you will work from:

```bash
mkdir -p ~/pikpak && cd ~/pikpak
echo 'PIKPAK_REFRESH_TOKEN=paste-your-token-here' > .env
```

(In a clone of this repository, `cp .env.example .env` gives you a template
with every setting.)

**3. Check that it works:**

```console
$ pikpak quota
total: 10.00 TiB
used:  4.23 TiB
free:  5.77 TiB
usage: 42.3%
```

Run `pikpak` from that directory or any directory below it: it reads the first
`.env` it finds there or in a parent.

### What to know about the token

- **It is single-use.** Every run rotates it, and the CLI writes the
  replacement back into the `.env` it loaded. Keep it in `.env`, not in your
  shell environment — from there the CLI can only *print* the replacement, and
  missing it leaves you with a dead token.
- **One client at a time.** The web app, or a second copy of this tool on the
  same account, rotates the token too. If another process got there first, the
  CLI re-reads `.env` and retries once.
- **Exit code 3 (`invalid_grant`) means the token is dead.** Copy a fresh one
  into `.env` and log the web app out. The web app issues platform-bound tokens,
  so even a fresh copy can occasionally be refused by this Android-client tool.

### Other settings

All optional, in `.env` or the environment:

| variable | meaning |
| --- | --- |
| `PIKPAK_PROXY` | HTTP(S) proxy URL, e.g. `http://127.0.0.1:7890` |
| `PIKPAK_DEVICE_ID` | device id; filled in on the first run, so every run reports the same device |
| `PIKPAK_CLIENT_ID`, `PIKPAK_CLIENT_SECRET` | override the OAuth client |
| `PIKPAK_STATE_DIR` | where background jobs are kept (default `.pikpak` beside your `.env`) |

## CLI

```bash
pikpak quota                                  # totals; --raw for byte counts
pikpak ls                                     # root
pikpak ls --path "/My Pack" -l                # long listing; --human for sizes
pikpak download --path "/My Pack/Movies" -j 4
pikpak download --path "/My Pack/video.mp4" --output /data
pikpak download --path /                      # drive root, expanded into its children
pikpak --verbose download --path "/My Pack/video.mp4"
```

Re-running an interrupted download resumes it, and files already complete on
disk are skipped, so the same command can simply be run again.

| flag | meaning |
| --- | --- |
| `--path <p>` | remote path, `/` = drive root |
| `--output <d>` | local directory (default `./downloads`) |
| `-j, --jobs <n>` | files fetched in parallel (folders only) |
| `-l, --long` | long listing (`ls`) |
| `--human` | human-readable sizes (`ls`; long flag only — `-h` is `--help`) |
| `--raw` | byte counts instead of sizes (`quota`) |
| `--verbose` | debug logs, before or after the subcommand |
| `--json` | one JSON document on stdout: the result, or the failure |
| `--no-progress` | never draw byte-level progress |
| `--detach` | `download`: return a job id instead of waiting |

### Machine-readable output

`--json` writes exactly one JSON document to stdout — the result, or the failure —
and keeps every narration line (progress, retries, `Saved:`) on stderr, so a
script or an agent can parse stdout without scraping prose. Exit codes separate
the cases worth branching on: `3` auth, `4` not found, `5` network, `6` refused,
`7` local I/O. The full contract — document shapes, codes, recipes — is in
[AGENTS.md](AGENTS.md).

```console
$ pikpak --json quota
{"command":"quota","ok":true,"result":{"free":6345116277760,"total":10995116277760,"usage_percent":42.3,"used":4650000000000}}
```

### Long downloads

`download --detach` returns a job id straight away and keeps transferring in the
background, so a transfer can outlive the command that started it:

```bash
id=$(pikpak download --detach --path "/My Pack/Movies" --output /data)
pikpak jobs list                 # id, state, progress, path
pikpak jobs status "$id"
pikpak jobs wait "$id"           # blocks; exits with the job's own outcome
pikpak jobs cancel "$id"         # the worker stops within a few seconds
```

State lives under `PIKPAK_STATE_DIR` (default `.pikpak` beside your `.env`):
`jobs/<id>.json`, `jobs/<id>.cancel`, and the worker's output in
`logs/<id>.log`.

## Library

Depend on it with `default-features = false` to leave out the CLI's own
dependencies (clap, dotenvy, tracing-subscriber, humansize, anyhow).

```rust,no_run
use pikpak::Client;
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

- `client.http_client()` for API calls (total request timeout);
  `client.download_client()` for file content — no total deadline, only a
  per-read stall timeout, so large transfers are not cut off.
- Persist each rotation as it happens with `ClientBuilder::on_refresh_token`;
  recover from another process's rotation with
  `ClientBuilder::refresh_token_source`.
- The library never writes your files.

## License

MIT — see [LICENSE](https://github.com/youming-ai/pikpak-downloader/blob/main/LICENSE).
