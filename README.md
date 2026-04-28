# PikPak Personal Cloud Management Tool

**A native Rust CLI for managing PikPak personal cloud storage, built on a reverse-engineered API.**

> No external `pikpakcli` binary is required — everything is implemented as native API calls.

## 🌍 Languages

- 🇺🇸 **English** | 🇨🇳 **[中文](./README.zh-CN.md)** | 🇯🇵 **[日本語](./README.ja-JP.md)** | 🇰🇷 **[한국어](./README.ko-KR.md)**

## ✨ Features

### 📁 File Management
- **`ls --path "/My Pack"`** — Path-based navigation (folder id resolution behind the scenes)
- **Long format** (`-l`) with size and type
- **Human-readable sizes** (`-h`): KB / MB / GB

### ⬇️ Download
- **Native download** via direct API URL streaming
- **Single file** or **recursive folder** downloads
- **Progress display** with percentage

### 📊 Account Info
- **`quota`** — Storage usage with human-readable or raw byte output

### ⚙️ Build
- **Single ~3.9 MB release binary** (statically linked, no runtime dependencies)
- **Cross-platform** via Rust (Linux, macOS, Windows)

## 🚀 Quick Start

### Prerequisites
- **Rust 1.78+** (for building from source)
- **PikPak Account** with a refresh token

### Installation from Source

```bash
git clone https://github.com/your-username/pikpak-downloader.git
cd pikpak-downloader
make build       # cargo build --release
```

The binary is produced at `rust/target/release/pikpak-cli`.

### Configure Authentication

```bash
cp .env.example .env
# Edit .env:
PIKPAK_REFRESH_TOKEN=your_refresh_token
# Optional: PIKPAK_PROXY=http://127.0.0.1:7890
```

### Usage

```bash
# List root directory
make run ARGS='ls'

# Detailed view with human-readable sizes
make run ARGS='ls --path "/My Pack" -l -h'

# Download a file
make run ARGS='download --path "/My Pack/video.mp4" --output ./downloads'

# View storage quota
make run ARGS='quota'
```

Or run the binary directly:

```bash
./rust/target/release/pikpak-cli help
```

## 📋 CLI Reference

### `ls` — List files and directories

| Option | Description |
|--------|-------------|
| `--path <path>` | Directory path, e.g. `"/My Pack"` (default: `/`) |
| `-l, --long` | Long format with kind and size |
| `-h, --human` | Human-readable sizes (KB / MB / GB) |

```bash
pikpak-cli ls --path "/My Pack" -l -h
```

### `download` — Download files or folders

| Option | Description |
|--------|-------------|
| `--path <path>` | Remote path to download (required) |
| `--output <dir>` | Local output directory (default: `./downloads`) |
| `--count <n>` | Concurrency hint (default: `3`) |

```bash
# Single file
pikpak-cli download --path "/My Pack/document.pdf" --output ./downloads

# Entire folder (recursive)
pikpak-cli download --path "/My Photos" --output ./backups
```

### `quota` — View storage quota

| Option | Description |
|--------|-------------|
| `--raw` | Print raw byte counts instead of human-readable units |

```bash
pikpak-cli quota
pikpak-cli quota --raw
```

## 🏗️ How it works

1. **Auth** — Exchanges the refresh token for a short-lived access token. The refresh token is rotated on each exchange; callers can persist it via [`TokenManager::current_refresh_token`](rust/pikpak-api/src/auth.rs).
2. **Captcha** — Requests an `X-Captcha-Token` from PikPak's shield endpoint for every drive API call. Auto-refreshes on expiry.
3. **Path resolution** — Walks each path segment via `list_folder` to resolve `"/My Pack/videos"` → `folder_id`.
4. **Download** — Fetches `web_content_link` from the API, then streams bytes to disk with an in-band progress bar.

## 📁 Project Structure

```
pikpak-downloader/
├── rust/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── pikpak-api/          # Library crate (auth, captcha, API client)
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs        # OAuth2 refresh-token flow
│   │       ├── captcha.rs     # MD5-chained captcha sign
│   │       ├── client.rs      # API: quota, list_folder, resolve_path, download
│   │       ├── error.rs       # Error enum
│   │       └── types.rs       # FileInfo, FileKind, Quota
│   └── pikpak-cli/            # Binary crate (CLI frontend)
│       └── src/main.rs        # Clap CLI + tokio async runtime
├── .env.example
├── Makefile                   # `make build`, `make run ARGS=...`
├── README.md                  # This file
├── README.zh-CN.md
├── README.ja-JP.md
└── README.ko-KR.md
```

## ⚙️ Configuration

### Environment Variables (`.env`)

```bash
# Required
PIKPAK_REFRESH_TOKEN=your_refresh_token

# Optional
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader

# OAuth overrides (leave unset for defaults)
# PIKPAK_CLIENT_ID=
# PIKPAK_CLIENT_SECRET=
```

### How to get a Refresh Token

1. Log in to [PikPak web version](https://mypikpak.com)
2. Open Developer Tools (F12)
3. Navigate to **Application → Local Storage → `https://mypikpak.com`**
4. Copy the `refresh_token` value
5. Paste into `.env` as `PIKPAK_REFRESH_TOKEN=...`

## 🔄 Recent Changes

| Commit | Change |
|--------|--------|
| `38f65ac` | **Rust rewrite** — removed Go wrapper (`pikpakcli` external binary no longer needed) |
| — | Added native download and path resolution |
| — | Fixed `PrintStats` divide-by-zero panic, `quoteString` YAML escaping bug, `PerformanceMetrics` mixed sync, `detectFileType` repeated allocations, `ListFilesStream` unsafe stdout close, Rust token refresh-storm, and captcha double-check race |

## 📄 License

MIT — see [LICENSE](LICENSE) for details.

## 🙏 Acknowledgments

- Endpoints and captcha logic are reverse-engineered from [`pikpakcli`](https://github.com/52funny/pikpakcli) (Go) and ported to native Rust.
- PikPak does not publish an official API; endpoints may change without notice.
