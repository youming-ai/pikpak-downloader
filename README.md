# PikPak Personal Cloud Management Tool

**CLI wrapper around [`pikpakcli`](https://github.com/52funny/pikpakcli) that reads credentials from `.env`, generates the upstream config, and exposes a small, consistent command surface.**

> Note: this is a thin wrapper. The actual download/list/quota work is performed by the upstream `pikpakcli` binary. See "How it works" below.

## 🌍 Languages

- 🇺🇸 **English** | 🇨🇳 **[中文](./README.zh-CN.md)** | 🇯🇵 **[日本語](./README.ja-JP.md)** | 🇰🇷 **[한국어](./README.ko-KR.md)**

## ✨ Features

### 🔧 What this wrapper provides
- **`.env` based configuration** — keeps your refresh token out of the upstream YAML config
- **Atomic config generation** — writes `config.yml` via tempfile + rename to prevent partial-write corruption
- **Concurrency pass-through** — `--count` flag is forwarded to `pikpakcli` for folder downloads
- **Per-command timeouts** — `ls` and `quota` have deadlines so a hung network doesn't block forever
- **Output size caps** — captured output is bounded to avoid runaway memory on pathological directories

### 📁 File Management
- **📋 Advanced File Listing** - Paginated lists with detailed file information
- **🎯 Smart File Classification** - Automatic file type detection with O(1) lookup
- **📱 Human-Readable Formats** - Easy-to-read file sizes and timestamps
- **🔍 Efficient Search** - Fast directory traversal and file discovery

### 💾 Download Features
- **📁 Folder & single-file downloads** - `pikpakcli download` is wrapped with a configurable output directory
- **📈 Progress output** - Upstream's `--progress` is exposed behind `-progress`
- **🔄 Resume / retry** - Delegated to upstream `pikpakcli` behavior

### 📊 Monitoring & Control
- **⏱️ Timeout Protection** - `ls` / `quota` run with per-command deadlines
- **🔒 Environment-based Configuration** - refresh token and optional proxy read from `.env`

### 🌐 Enhanced CLI
- **🎯 Intuitive Commands** - Clean, consistent command structure
- **📖 Comprehensive Help** - Detailed help system with examples
- **🎨 Rich Output** - Formatted tables and progress indicators
- **⚡ Fast Response** - Optimized command execution with caching

## 🚀 Quick Start

### Prerequisites
- **Go 1.21+** - For building from source
- **Git** - For cloning the repository
- **PikPak Account** - For cloud storage access

### 1. Clone Repository
```bash
git clone https://github.com/your-username/pikpak-downloader.git
cd pikpak-downloader
```

### 2. Install Dependencies
```bash
make deps
```

### 3. Configure Authentication
```bash
cp .env.example .env
```

Edit `.env` file with your credentials:
```bash
# RefreshToken authentication
PIKPAK_REFRESH_TOKEN=your_refresh_token

# Optional: Proxy settings (if needed)
PIKPAK_PROXY=http://127.0.0.1:7890
```

### 4. Build & Run
```bash
make build-cli
./pikpak-cli help
```

### 5. Verify Installation
```bash
./pikpak-cli quota              # Check storage quota
./pikpak-cli ls                 # List files
```

## 🐳 Alternative Installation

### Download Pre-built Binary
```bash
# For macOS (Intel)
curl -L -o pikpak-cli https://github.com/your-username/pikpak-downloader/releases/latest/download/pikpak-cli-darwin-amd64
chmod +x pikpak-cli

# For macOS (Apple Silicon)
curl -L -o pikpak-cli https://github.com/your-username/pikpak-downloader/releases/latest/download/pikpak-cli-darwin-arm64
chmod +x pikpak-cli
```

### Using Go Install
```bash
go install github.com/your-username/pikpak-downloader@latest
```

## 🎯 Usage Examples

### Basic File Operations
```bash
# List root directory
./pikpak-cli ls

# Detailed view with human-readable sizes
./pikpak-cli ls -l -h

# Browse specific folder
./pikpak-cli ls -path "/My Documents" -l
```

### Download Operations
```bash
# Download single file with optimal settings
./pikpak-cli download -path "/important/document.pdf"

# Download entire folder with smart concurrency
./pikpak-cli download -path "/My Photos" -output "./backups" -progress

# High-performance download for many small files
./pikpak-cli download -path "/Downloads" -count 8 -progress
```

### Monitoring and Management
```bash
# Check storage usage
./pikpak-cli quota -h

# Monitor downloads in real-time
./pikpak-cli download -path "/large-folder" -count 5 -progress
```

## 📋 Commands

### File Listing
```bash
./pikpak-cli ls                               # Root directory
./pikpak-cli ls -path "/My Pack"              # Specific folder
./pikpak-cli ls -l -h                         # Detailed view with human-readable sizes
./pikpak-cli ls -path "/folder" -l            # Long format for specific folder
```

### Storage Quota
```bash
./pikpak-cli quota                            # View storage usage
./pikpak-cli quota -h                         # Human-readable format
```

### Download Files
```bash
./pikpak-cli download -path "/file.pdf"                           # Single file
./pikpak-cli download -path "/My Pack" -output "./downloads"      # Entire folder
./pikpak-cli download -path "/My Pack" -count 5                   # Set concurrency (1-10)
./pikpak-cli download -path "/My Pack" -progress                  # Show progress
```

## 🏗️ How it works

This tool does **not** implement the PikPak API itself. It:

1. Loads `PIKPAK_REFRESH_TOKEN` (and optional `PIKPAK_PROXY`, `PIKPAK_CLIENT_ID`, `PIKPAK_CLIENT_SECRET`) from `.env`.
2. Writes a `config.yml` for upstream [`pikpakcli`](https://github.com/52funny/pikpakcli) via an atomic tempfile rename.
3. Shells out to the `pikpakcli` binary for every operation (`ls`, `quota`, `download`).
4. For metadata commands, wraps the call with a deadline and an output size cap; for `download`, streams stdout/stderr through unchanged so upstream progress indicators work.

There is no separate concurrency scheduler in this wrapper — `-count` is forwarded to `pikpakcli --count` verbatim.

## 📁 Project Structure

```
pikpak-downloader/
├── pikpak_cli.go             # CLI entry point (flag parsing + dispatch)
├── pikpak_client.go          # Wrapper around upstream pikpakcli (exec + output parsing)
├── pikpak_client_test.go     # Parser unit tests
├── config_manager.go         # .env loading and config.yml generation
├── .env.example              # Configuration template
├── Makefile                  # Build automation
└── README*.md                # Documentation
```

## ⚙️ Configuration

### Environment Variables (.env)
```bash
# Authentication
PIKPAK_REFRESH_TOKEN=[your_refresh_token]

# Optional
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader

# OAuth overrides (optional). Leave unset to use upstream pikpakcli defaults.
# PIKPAK_CLIENT_ID=
# PIKPAK_CLIENT_SECRET=
```

### How to Get RefreshToken
1. Login to PikPak web version
2. Open Developer Tools (F12)
3. Navigate to `Application` → `Local Storage`
4. Copy `refresh_token` value
5. Add to `.env` file

## 🔄 Version History

See `git log` for the authoritative change history. Recent notable changes:

- Removed an inert "SmartDownloader" layer whose dynamic-concurrency logic ran *after* each download with a hardcoded 50 MB size estimate and was never in the actual I/O path.
- Made per-command timeouts configurable; `download` no longer inherits the 30s metadata timeout.
- Stopped hardcoding OAuth `client_id`/`client_secret` in the generated config; they are now optional env overrides, otherwise upstream `pikpakcli` defaults apply.
- `ls -l` no longer prints a fabricated "modified" column (the original value was `time.Now()` for every row).

---

## 📈 Recommended concurrency

The `-count` flag is forwarded to `pikpakcli --count` unchanged. The upstream tool is authoritative on what this number does.

```bash
# Many small files
./pikpak-cli download -path "/downloads" -count 8

# One large file
./pikpak-cli download -path "/large-files" -progress

# General use
./pikpak-cli download -path "/my-folder" -progress
```

## 🛠️ Development

```bash
make build-cli    # Build the CLI tool
make clean        # Clean build artifacts
make run-cli ls   # Run with example command
```

## 🤝 Contributing

1. Fork the project
2. Create feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit changes (`git commit -m 'Add AmazingFeature'`)
4. Push to branch (`git push origin feature/AmazingFeature`)
5. Open Pull Request

## 📄 License

MIT License - see [LICENSE](LICENSE) file for details.

## 🔧 Troubleshooting

### Common Issues

#### Configuration Problems
```bash
# Error: "Configuration check failed"
# Solution: Verify your .env file credentials
./pikpak-cli quota  # Test configuration
```

#### Download Issues
```bash
# Error: "not found pikpak folder"
# Solution: Check file path and permissions
./pikpak-cli ls -path "/"  # Browse available folders
```

#### Performance Issues
```bash
# Slow downloads: Try adjusting concurrency
./pikpak-cli download -path "/folder" -count 1  # Reduce concurrency
# or
./pikpak-cli download -path "/folder" -count 8  # Increase concurrency
```

### Debug Mode
```bash
# Enable debug output for troubleshooting
# Edit .env file:
PIKPAK_DEBUG=true
```

### Getting Help
- 🐛 **Report bugs**: [GitHub Issues](https://github.com/your-username/pikpak-downloader/issues)
- 💬 **Discussions**: [GitHub Discussions](https://github.com/your-username/pikpak-downloader/discussions)

## ⚠️ Disclaimer

This tool is for personal cloud management only. Please comply with PikPak's terms of service and copyright laws. The developer assumes no legal liability.

## 🙏 Acknowledgments

- [pikpakcli](https://github.com/52funny/pikpakcli) — all real PikPak API work is done by this upstream project; this repo is a configuration/UX wrapper on top of it.