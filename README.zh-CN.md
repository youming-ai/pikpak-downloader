# PikPak 个人云盘管理工具 v4.0 🚀

一个用Go语言编写的PikPak个人云盘管理工具，提供完整的命令行界面来管理您的PikPak云盘文件。

## ✨ 核心特性

### 🎯 个人云盘管理
- **文件列表** - 浏览和查看个人云盘中的文件和文件夹
- **配额查看** - 实时查看云盘容量使用情况
- **文件下载** - 下载个人云盘中的任意文件或整个文件夹
- **智能分类** - 自动识别文件类型（视频、图片、文档等）

### 🔧 技术优势
- **Go语言开发** - 高性能，低资源占用
- **命令行界面** - 简洁易用的CLI工具
- **环境变量配置** - 安全的配置管理方案
- **并发下载** - 支持多线程并发下载
- **进度显示** - 实时显示下载进度

## 🚀 快速开始

### 1. 安装依赖
```bash
make deps
```

### 2. 配置认证信息

创建配置文件：
```bash
cp .env.example .env
```

编辑 `.env` 文件，填入你的PikPak认证信息：
```bash
# 方式1: 使用账号密码
PIKPAK_USERNAME=[你的邮箱地址]
PIKPAK_PASSWORD=[你的密码]

# 方式2: 使用RefreshToken (推荐)
PIKPAK_REFRESH_TOKEN=[你的refresh_token]
```

### 3. 编译程序
```bash
make build-cli
```

### 4. 开始使用
```bash
# 查看帮助
./pikpak-cli help

# 查看云盘配额
./pikpak-cli quota

# 列出根目录文件
./pikpak-cli ls

# 列出指定目录
./pikpak-cli ls -path "/My Pack"

# 详细列表
./pikpak-cli ls -path "/My Pack" -l -h

# 下载文件
./pikpak-cli download -path "/My Pack/document.pdf"

# 下载整个文件夹
./pikpak-cli download -path "/My Pack" -output "./downloads"
```

## 📋 命令详解

### `ls` - 列出文件
```bash
./pikpak-cli ls [选项]

选项:
  -path string     目录路径 (默认: "/")
  -l               长格式显示
  -h               人类可读格式

示例:
  ./pikpak-cli ls                          # 列出根目录
  ./pikpak-cli ls -path "/My Pack"        # 列出指定目录
  ./pikpak-cli ls -l -h                     # 详细格式
```

### `quota` - 查看配额
```bash
./pikpak-cli quota [选项]

选项:
  -h               人类可读格式 (默认: true)

示例:
  ./pikpak-cli quota                       # 查看配额信息
```

### `download` - 下载文件
```bash
./pikpak-cli download [选项]

选项:
  -path string     下载路径 (默认: "/")
  -output string   输出目录 (默认: "./downloads")
  -count int       并发数 (默认: 3)
  -progress        显示进度 (默认: true)

示例:
  ./pikpak-cli download -path "/My Pack/video.mp4"                    # 下载单个文件
  ./pikpak-cli download -path "/My Pack" -output "./my_downloads"   # 下载到指定目录
  ./pikpak-cli download -path "/My Pack" -count 5                    # 设置并发数
```

## 🛠️ 开发构建

### 编译
```bash
make build-cli
```

### 运行
```bash
# 使用Makefile
make run-cli ls
make run-cli quota
make run-cli download -path "/My Pack"

# 直接运行
./pikpak-cli ls
```

### 清理
```bash
make clean
```

## 📊 功能演示

### 查看配额信息
```bash
$ ./pikpak-cli quota
📊 云盘配额信息:
总容量: 6.0GB
已使用: 604.2MB
使用率: 9.8%
```

### 列出文件
```bash
$ ./pikpak-cli ls
文件夹        My Pack
文件夹        Pack From Shared

$ ./pikpak-cli ls -path "/Pack From Shared"
文件夹        onlyfans chaeira 34V
```

### 详细列表
```bash
$ ./pikpak-cli ls -l -h
类型        大小       修改时间            文件名
文件夹      -          2025-01-02 15:04   My Pack
文件夹      -          2025-01-01 10:30   Pack From Shared
```

## 📁 项目结构

```
pikpak-downloader/
├── pikpak_cli.go           # CLI命令行界面
├── pikpak_client.go        # PikPak客户端核心功能
├── config_manager.go       # 配置管理
├── .env                     # 用户配置文件
├── .env.example            # 配置文件模板
├── pikpak-cli              # 可执行文件
├── Makefile                 # 构建脚本
├── go.mod                   # Go模块文件
├── go.sum                   # 依赖校验文件
├── README.zh-CN.md          # 中文项目说明
└── .gitignore               # Git忽略文件
```

## ⚙️ 配置说明

### 环境变量配置
在 `.env` 文件中配置以下信息：

```bash
# PikPak 账号认证
PIKPAK_USERNAME=[你的邮箱地址]
PIKPAK_PASSWORD=[你的密码]

# 或使用 RefreshToken (推荐)
PIKPAK_REFRESH_TOKEN=[你的refresh_token]

# 代理设置 (可选)
# PIKPAK_PROXY=http://127.0.0.1:7890

# 设备设置 (可选)
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### 获取 RefreshToken
1. 登录 PikPak网页版
2. 按 `F12` 打开开发者工具
3. 进入 `Application` → `Local Storage`
4. 查找 `refresh_token` 字段并复制其值
5. 填入 `.env` 文件中的 `PIKPAK_REFRESH_TOKEN`

## 🔄 版本历史

### v4.0.0 (2025-10-18) 🎯
- ✨ **重新设计项目定位** - 专注于个人云盘管理
- 🎯 **复刻 pikpakcli 功能** - 文件列表、配额查看、文件下载
- 🔧 **完整的CLI界面** - 参数解析、帮助系统
- 📋 **智能文件分类** - 自动识别文件类型
- ⚙️ **配置管理优化** - 环境变量配置方案

### v3.1.0 (2025-10-18) 🌟
- ✨ **.env 配置支持** - 新增环境变量配置方案，更安全便捷
- 🔄 **自动配置生成** - 程序自动读取.env并生成pikpakcli配置文件
- 📋 **配置状态检查** - 详细的配置验证和状态显示
- 🔧 **配置管理器** - 新增config_manager.go模块
- 🎯 **默认CLI模式** - 混合工具默认使用CLI模式，支持三种模式选择
- 📁 **配置文件模板** - 提供.env.example模板文件

### v3.x.x (分享链接下载)
- 混合下载模式
- 网页爬虫功能
- 分享链接处理

### v2.x.x (分享链接下载)
- Go版本重写
- 基础下载功能

### v1.x.x (Python版本)
- 初始实现

## 🤝 贡献

欢迎提交Issue和Pull Request！

1. Fork本项目
2. 创建特性分支 (`git checkout -b feature/AmazingFeature`)
3. 提交更改 (`git commit -m 'Add some AmazingFeature'`)
4. 推送到分支 (`git push origin feature/AmazingFeature`)
5. 开启Pull Request

## 📄 许可证

本项目采用MIT许可证 - 查看 [LICENSE](LICENSE) 文件了解详情。

## ⚠️ 免责声明

本工具仅用于个人云盘管理。请遵守PikPak的服务条款，不要用于商业用途或违反版权法的内容。开发者不承担任何法律责任。

## 🙏 致谢

- [pikpakcli](https://github.com/52funny/pikpakcli) - 核心功能参考
- Go语言社区 - 优秀的开发工具和库

---

如果这个项目对你有帮助，请给个⭐️！