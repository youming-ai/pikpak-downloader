# PikPak 个人云盘管理工具

**基于 PikPak API 原生 Rust 命令行工具。**

> 不再需要外部 `pikpakcli` 二进制文件 — 所有功能均通过原生 API 调用实现。

## 🌍 语言

- 🇺🇸 **[English](./README.md)** | 🇨🇳 **中文** | 🇯🇵 **[日本語](./README.ja-JP.md)** | 🇰🇷 **[한국어](./README.ko-KR.md)**

## ✨ 功能

### 📁 文件管理
- **`ls --path "/My Pack"`** — 基于路径的导航（自动解析 folder id）
- **长格式** (`-l`) 显示文件类型和大小
- **人性化大小** (`-h`): KB / MB / GB

### ⬇️ 下载
- **原生下载** — 通过 API 直接下载 URL 流式传输到本地
- **单个文件** 或 **递归文件夹** 下载
- **进度显示** 带百分比

### 📊 账户信息
- **`quota`** — 存储用量，支持人性化或原始字节格式

### ⚙️ 构建
- **单文件 ~3.9 MB 二进制**（静态链接，无运行依赖）
- **跨平台** 通过 Rust（Linux、macOS、Windows）

## 🚀 快速开始

### 前置条件
- **Rust 1.78+**（从源码构建）
- **PikPak 账户** 并拥有 refresh token

### 从源码安装

```bash
git clone https://github.com/your-username/pikpak-downloader.git
cd pikpak-downloader
make build
```

二进制文件生成在 `rust/target/release/pikpak-cli`。

### 配置认证

```bash
cp .env.example .env
# 编辑 .env:
PIKPAK_REFRESH_TOKEN=your_refresh_token
# 可选: PIKPAK_PROXY=http://127.0.0.1:7890
```

### 使用

```bash
# 列出根目录文件
make run ARGS='ls'

# 详细视图加人性化大小
make run ARGS='ls --path "/My Pack" -l -h'

# 下载文件
make run ARGS='download --path "/My Pack/video.mp4" --output ./downloads'

# 查看配额
make run ARGS='quota'
```

或直接运行二进制文件：

```bash
./rust/target/release/pikpak-cli help
```

## 📋 CLI 参考

### `ls` — 列出文件和目录

| 选项 | 说明 |
|------|------|
| `--path <path>` | 目录路径，如 `"/My Pack"`（默认：`/`） |
| `-l, --long` | 长格式显示类型和大小 |
| `-h, --human` | 人性化大小（KB / MB / GB） |

```bash
pikpak-cli ls --path "/My Pack" -l -h
```

### `download` — 下载文件或文件夹

| 选项 | 说明 |
|------|------|
| `--path <path>` | 要下载的远程路径（必须） |
| `--output <dir>` | 本地输出目录（默认：`./downloads`） |
| `--count <n>` | 并发提示（默认：`3`） |

```bash
# 单个文件
pikpak-cli download --path "/My Pack/document.pdf" --output ./downloads

# 整个文件夹（递归）
pikpak-cli download --path "/My Photos" --output ./backups
```

### `quota` — 查看存储配额

| 选项 | 说明 |
|------|------|
| `--raw` | 打印原始字节数而非人性化格式 |

```bash
pikpak-cli quota
pikpak-cli quota --raw
```

## 🏗️ 工作原理

1. **认证** — 用 refresh token 换取短效 access token。每次换发会轮转 refresh token，可通过 `TokenManager::current_refresh_token` 获取并持久化。
2. **验证码** — 为每次驱动 API 调用请求 `X-Captcha-Token`，过期自动刷新。
3. **路径解析** — 通过 `list_folder` 逐级解析 `"/My Pack/videos"` → `folder_id`。
4. **下载** — 从 API 获取 `web_content_link`，然后流式写入磁盘并显示进度。

## 📁 项目结构

```
pikpak-downloader/
├── rust/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── pikpak-api/          # 库 crate（认证、验证码、API 客户端）
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs        # OAuth2 刷新令牌流程
│   │       ├── captcha.rs     # MD5 链计算验证码 sign
│   │       ├── client.rs      # API: quota, list_folder, resolve_path, download
│   │       ├── error.rs       # 错误枚举
│   │       └── types.rs       # FileInfo, FileKind, Quota
│   └── pikpak-cli/            # 二进制 crate（CLI 前端）
│       └── src/main.rs        # Clap CLI + tokio 异步运行时
├── .env.example
├── Makefile
├── README.md
└── ...
```

## ⚙️ 配置

### 环境变量 (`.env`)

```bash
# 必须
PIKPAK_REFRESH_TOKEN=your_refresh_token

# 可选
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader

# OAuth 覆盖（留空则使用默认值）
# PIKPAK_CLIENT_ID=
# PIKPAK_CLIENT_SECRET=
```

### 如何获取 Refresh Token

1. 登录 [PikPak 网页版](https://mypikpak.com)
2. 打开开发者工具 (F12)
3. 进入 **Application → Local Storage → `https://mypikpak.com`**
4. 复制 `refresh_token` 值
5. 粘贴到 `.env`：`PIKPAK_REFRESH_TOKEN=...`

## 🔄 最近变更

| 提交 | 变更 |
|------|------|
| `38f65ac` | **Rust 重写** — 移除 Go wrapper（不再需要外部 `pikpakcli` 二进制） |
| — | 新增原生下载和路径解析 |
| — | 修复 `PrintStats` 除零 panic、`quoteString` YAML 转义 bug、`PerformanceMetrics` 混合同步、`detectFileType` 重复分配、`ListFilesStream` stdout 关闭问题、Rust token 刷新风暴、captcha double-check 竞态 |

## 📄 许可

MIT — 详见 [LICENSE](LICENSE)。

## 🙏 致谢

- 端点和验证码逻辑逆向自 [`pikpakcli`](https://github.com/52funny/pikpakcli)（Go）并移植为原生 Rust。
- PikPak 不发布官方 API，端点可能随时更改。
