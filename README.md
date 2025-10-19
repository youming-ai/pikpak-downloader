# PikPak Personal Cloud Management Tool v4.0 🚀

A high-performance Go CLI tool for managing your PikPak personal cloud storage.

## 🌍 Languages

- 🇺🇸 **English** | 🇨🇳 **[中文](./README.zh-CN.md)** | 🇯🇵 **[日本語](./README.ja-JP.md)** | 🇰🇷 **[한국어](./README.ko-KR.md)**

## ✨ Features

- **📁 File Management** - List, browse, and organize cloud files
- **💾 Download Files** - Download individual files or entire folders
- **📊 Storage Monitor** - Real-time quota and usage information
- **⚡ High Performance** - Concurrent downloads with progress tracking
- **🔒 Secure Config** - Environment variable-based authentication

## 🚀 Quick Start

### 1. Install Dependencies
```bash
make deps
```

### 2. Configure Authentication
```bash
cp .env.example .env
```

Edit `.env` file:
```bash
# Method 1: Account & Password
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]

# Method 2: RefreshToken (Recommended)
PIKPAK_REFRESH_TOKEN=[your_refresh_token]
```

### 3. Build & Run
```bash
make build-cli
./pikpak-cli help
```

## 📋 Commands

### File Listing
```bash
./pikpak-cli ls                    # Root directory
./pikpak-cli ls -path "/My Pack"   # Specific folder
./pikpak-cli ls -l -h              # Detailed view
```

### Storage Quota
```bash
./pikpak-cli quota                 # View storage usage
```

### Download Files
```bash
./pikpak-cli download -path "/My Pack/file.pdf"                    # Single file
./pikpak-cli download -path "/My Pack" -output "./downloads"      # Entire folder
./pikpak-cli download -path "/My Pack" -count 5                   # Set concurrency
```

## 📁 Project Structure

```
pikpak-downloader/
├── pikpak_cli.go           # CLI interface
├── pikpak_client.go        # Core client functionality
├── config_manager.go       # Configuration management
├── .env.example            # Configuration template
├── Makefile                # Build automation
└── README*.md              # Documentation
```

## ⚙️ Configuration

### Environment Variables (.env)
```bash
# Authentication
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]
# OR
PIKPAK_REFRESH_TOKEN=[your_refresh_token]

# Optional
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### How to Get RefreshToken
1. Login to PikPak web version
2. Open Developer Tools (F12)
3. Navigate to `Application` → `Local Storage`
4. Copy `refresh_token` value
5. Add to `.env` file

## 🔄 Version History

### v4.0.0 (2025-10-18) 🎯
- **Personal Cloud Management** - Complete rewrite focusing on file management
- **CLI Interface** - Full command-line interface with help system
- **Smart File Classification** - Automatic file type recognition
- **Environment Configuration** - Secure .env-based configuration

### v3.1.0 (2025-10-18) 🌟
- Added .env configuration support
- Automatic configuration generation
- Enhanced security and usability

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

## ⚠️ Disclaimer

This tool is for personal cloud management only. Please comply with PikPak's terms of service and copyright laws. The developer assumes no legal liability.

## 🙏 Acknowledgments

- [pikpakcli](https://github.com/52funny/pikpakcli) - Core functionality reference
- Go language community - Excellent development tools and libraries

---

If this project helps you, please give it a ⭐️!