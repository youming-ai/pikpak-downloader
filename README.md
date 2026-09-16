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

```bash
cargo install --path .        # or: cargo build --release
pikpak --help
```

Rust 1.86 or newer (`rust-version` in `Cargo.toml`, pinned by the CI MSRV job).

## Configure

```bash
cp .env.example .env
```

Read from the environment, or from the first `.env` found in the working
directory and then its parents. `PIKPAK_REFRESH_TOKEN` is required;
`PIKPAK_PROXY`, `PIKPAK_CLIENT_ID` and `PIKPAK_CLIENT_SECRET` are optional.

PikPak refresh tokens are **single-use**: every login rotates them, and the CLI
writes each replacement back into the file it loaded — so keep the token in
`.env` rather than your shell environment, where the tool can only *print* the
replacement. Two clients refreshing one account at once is handled as well: the
CLI re-reads that file and retries once.

<details>
<summary>Where to get the token</summary>

1. Log in at [mypikpak.com](https://mypikpak.com) and open DevTools.
2. **Application** → **Local Storage** → `https://mypikpak.com`.
3. Find `credentials`, or search the values for `refresh_token`.

The web app issues platform-bound tokens, so a freshly copied one can be refused
outright by this Android-client tool.
</details>

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

```console
$ pikpak quota
total: 10.00 TiB
used:  4.23 TiB
free:  5.77 TiB
usage: 42.3%
```

| flag | meaning |
| --- | --- |
| `--path <p>` | remote path, `/` = drive root |
| `--output <d>` | local directory (default `./downloads`) |
| `-j, --jobs <n>` | files fetched in parallel (folders only) |
| `-l, --long` | long listing (`ls`) |
| `--human` | human-readable sizes (`ls`; long flag only — `-h` is `--help`) |
| `--raw` | byte counts instead of sizes (`quota`) |
| `--verbose` | debug logs, before or after the subcommand |

## Library

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
