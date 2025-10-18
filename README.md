# PikPak Personal Cloud Management Tool v4.0 🚀

A PikPak personal cloud management tool written in Go, providing a complete command-line interface to manage your PikPak cloud files.

## 🌍 Language / 语言 / 言語 / 언어

- 🇺🇸 **English** - Current document
- 🇨🇳 **[简体中文](./README.zh-CN.md)** - 中文文档
- 🇯🇵 **[日本語](./README.ja-JP.md)** - 日本語ドキュメント  
- 🇰🇷 **[한국어](./README.ko-KR.md)** - 한국어 문서

## ✨ Core Features

### 🎯 Personal Cloud Management
- **File Listing** - Browse and view files and folders in your personal cloud
- **Quota Viewing** - Real-time cloud storage capacity usage
- **File Downloading** - Download any file or entire folder from your personal cloud
- **Smart Classification** - Automatic file type recognition (videos, images, documents, etc.)

### 🔧 Technical Advantages
- **Go Language Development** - High performance, low resource usage
- **Command Line Interface** - Simple and easy-to-use CLI tool
- **Environment Variable Configuration** - Secure configuration management solution
- **Concurrent Downloading** - Multi-threaded concurrent download support
- **Progress Display** - Real-time download progress display

## 🚀 Quick Start

### 1. Install Dependencies
```bash
make deps
```

### 2. Configure Authentication

Create configuration file:
```bash
cp .env.example .env
```

Edit the `.env` file and fill in your PikPak authentication information:
```bash
# Method 1: Use account and password
PIKPAK_USERNAME=[your_email_address]
PIKPAK_PASSWORD=[your_password]

# Method 2: Use RefreshToken (recommended)
PIKPAK_REFRESH_TOKEN=[your_refresh_token]
```

### 3. Compile Program
```bash
make build-cli
```

### 4. Start Using
```bash
# View help
./pikpak-cli help

# View cloud quota
./pikpak-cli quota

# List root directory files
./pikpak-cli ls

# List specified directory
./pikpak-cli ls -path "/My Pack"

# Detailed listing
./pikpak-cli ls -path "/My Pack" -l -h

# Download file
./pikpak-cli download -path "/My Pack/document.pdf"

# Download entire folder
./pikpak-cli download -path "/My Pack" -output "./downloads"
```

## 📋 Command Details

### `ls` - List Files
```bash
./pikpak-cli ls [options]

Options:
  -path string     Directory path (default: "/")
  -l               Long format display
  -h               Human readable format

Examples:
  ./pikpak-cli ls                          # List root directory
  ./pikpak-cli ls -path "/My Pack"        # List specified directory
  ./pikpak-cli ls -l -h                     # Detailed format
```

### `quota` - View Quota
```bash
./pikpak-cli quota [options]

Options:
  -h               Human readable format (default: true)

Examples:
  ./pikpak-cli quota                       # View quota information
```

### `download` - Download Files
```bash
./pikpak-cli download [options]

Options:
  -path string     Download path (default: "/")
  -output string   Output directory (default: "./downloads")
  -count int       Concurrent count (default: 3)
  -progress        Show progress (default: true)

Examples:
  ./pikpak-cli download -path "/My Pack/video.mp4"                    # Download single file
  ./pikpak-cli download -path "/My Pack" -output "./my_downloads"   # Download to specified directory
  ./pikpak-cli download -path "/My Pack" -count 5                    # Set concurrent count
```

## 🛠️ Development & Build

### Compile
```bash
make build-cli
```

### Run
```bash
# Using Makefile
make run-cli ls
make run-cli quota
make run-cli download -path "/My Pack"

# Direct execution
./pikpak-cli ls
```

### Clean
```bash
make clean
```

## 📊 Feature Demonstration

### View Quota Information
```bash
$ ./pikpak-cli quota
📊 Cloud Quota Information:
Total: 6.0GB
Used: 604.2MB
Usage: 9.8%
```

### List Files
```bash
$ ./pikpak-cli ls
Folder        My Pack
Folder        Pack From Shared

$ ./pikpak-cli ls -path "/Pack From Shared"
Folder        onlyfans chaeira 34V
```

### Detailed Listing
```bash
$ ./pikpak-cli ls -l -h
Type        Size       Modified Time        Name
Folder      -          2025-01-02 15:04   My Pack
Folder      -          2025-01-01 10:30   Pack From Shared
```

## 📁 Project Structure

```
pikpak-downloader/
├── pikpak_cli.go           # CLI command line interface
├── pikpak_client.go        # PikPak client core functionality
├── config_manager.go       # Configuration management
├── .env                     # User configuration file
├── .env.example            # Configuration file template
├── pikpak-cli              # Executable file
├── Makefile                 # Build script
├── go.mod                   # Go module file
├── go.sum                   # Dependency verification file
├── README.zh-CN.md          # Chinese project description
├── README.ja-JP.md          # Japanese project description
├── README.ko-KR.md          # Korean project description
└── .gitignore               # Git ignore file
```

## ⚙️ Configuration Instructions

### Environment Variable Configuration
Configure the following information in the `.env` file:

```bash
# PikPak account authentication
PIKPAK_USERNAME=[your_email_address]
PIKPAK_PASSWORD=[your_password]

# Or use RefreshToken (recommended)
PIKPAK_REFRESH_TOKEN=[your_refresh_token]

# Proxy settings (optional)
# PIKPAK_PROXY=http://127.0.0.1:7890

# Device settings (optional)
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### How to Get RefreshToken
1. Log in to PikPak web version
2. Press `F12` to open developer tools
3. Go to `Application` → `Local Storage`
4. Find the `refresh_token` field and copy its value
5. Fill it into the `PIKPAK_REFRESH_TOKEN` in the `.env` file

## 🔄 Version History

### v4.0.0 (2025-10-18) 🎯
- ✨ **Redesigned project positioning** - Focus on personal cloud management
- 🎯 **Replicate pikpakcli functionality** - File listing, quota viewing, file downloading
- 🔧 **Complete CLI interface** - Parameter parsing, help system
- 📋 **Smart file classification** - Automatic file type recognition
- ⚙️ **Configuration management optimization** - Environment variable configuration solution

### v3.1.0 (2025-10-18) 🌟
- ✨ **.env configuration support** - Add environment variable configuration solution, more secure and convenient
- 🔄 **Automatic configuration generation** - Program automatically reads .env and generates pikpakcli configuration files
- 📋 **Configuration status check** - Detailed configuration verification and status display
- 🔧 **Configuration manager** - Add config_manager.go module
- 🎯 **Default CLI mode** - Hybrid tool defaults to CLI mode, supports three mode selection
- 📁 **Configuration file template** - Provide .env.example template file

### v3.x.x (Share Link Download)
- Hybrid download mode
- Web crawler functionality
- Share link processing

### v2.x.x (Share Link Download)
- Go version rewrite
- Basic download functionality

### v1.x.x (Python Version)
- Initial implementation

## 🤝 Contributing

Welcome to submit Issues and Pull Requests!

1. Fork this project
2. Create feature branch (`git checkout -b feature/AmazingFeature`)
3. Commit changes (`git commit -m 'Add some AmazingFeature'`)
4. Push to branch (`git push origin feature/AmazingFeature`)
5. Open Pull Request

## 📄 License

This project is licensed under the MIT License - see the [LICENSE](LICENSE) file for details.

## ⚠️ Disclaimer

This tool is for personal cloud management only. Please comply with PikPak's terms of service and do not use it for commercial purposes or content that violates copyright laws. The developer assumes no legal liability.

## 🙏 Acknowledgments

- [pikpakcli](https://github.com/52funny/pikpakcli) - Core functionality reference
- Go language community - Excellent development tools and libraries

---

If this project helps you, please give it a ⭐️!