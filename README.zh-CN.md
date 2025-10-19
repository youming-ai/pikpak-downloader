# PikPak 个人云盘管理工具 v4.0 🚀

用于管理 PikPak 个人云盘存储的高性能 Go CLI 工具。

## ✨ 功能

- **📁 文件管理** - 列出、浏览和整理云盘文件
- **💾 文件下载** - 下载单个文件或整个文件夹
- **📊 存储监控** - 实时配额和使用量信息
- **⚡ 高性能** - 带进度显示的并发下载
- **🔒 安全配置** - 基于环境变量的身份验证

## 🚀 快速开始

### 1. 安装依赖
```bash
make deps
```

### 2. 配置认证信息
```bash
cp .env.example .env
```

编辑 `.env` 文件：
```bash
# 方式1: 账号密码
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]

# 方式2: RefreshToken (推荐)
PIKPAK_REFRESH_TOKEN=[your_refresh_token]
```

### 3. 构建和运行
```bash
make build-cli
./pikpak-cli help
```

## 📋 命令

### 文件列表
```bash
./pikpak-cli ls                    # 根目录
./pikpak-cli ls -path "/My Pack"   # 特定文件夹
./pikpak-cli ls -l -h              # 详细视图
```

### 存储配额
```bash
./pikpak-cli quota                 # 查看存储使用情况
```

### 文件下载
```bash
./pikpak-cli download -path "/My Pack/file.pdf"                    # 单个文件
./pikpak-cli download -path "/My Pack" -output "./downloads"      # 整个文件夹
./pikpak-cli download -path "/My Pack" -count 5                   # 设置并发数
```

## 📁 项目结构

```
pikpak-downloader/
├── pikpak_cli.go           # CLI 界面
├── pikpak_client.go        # 核心客户端功能
├── config_manager.go       # 配置管理
├── .env.example            # 配置模板
├── Makefile                # 构建自动化
└── README*.md              # 文档
```

## ⚙️ 配置

### 环境变量 (.env)
```bash
# 认证
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]
# 或者
PIKPAK_REFRESH_TOKEN=[your_refresh_token]

# 可选
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### 获取 RefreshToken
1. 登录 PikPak 网页版
2. 打开开发者工具 (F12)
3. 导航到 `Application` → `Local Storage`
4. 复制 `refresh_token` 值
5. 添加到 `.env` 文件中

## 🔄 版本历史

### v4.0.0 (2025-10-18) 🎯
- **个人云盘管理** - 专注于文件管理的完全重写
- **CLI 界面** - 带帮助系统的完整命令行界面
- **智能文件分类** - 自动文件类型识别
- **环境变量配置** - 安全的 .env 基础配置

### v3.1.0 (2025-10-18) 🌟
- 添加 .env 配置支持
- 自动配置生成
- 增强安全性和易用性

## 🛠️ 开发

```bash
make build-cli    # 构建 CLI 工具
make clean        # 清理构建产物
make run-cli ls   # 运行示例命令
```

## 🤝 贡献

1. Fork 项目
2. 创建功能分支 (`git checkout -b feature/AmazingFeature`)
3. 提交更改 (`git commit -m 'Add AmazingFeature'`)
4. 推送到分支 (`git push origin feature/AmazingFeature`)
5. 开启 Pull Request

## 📄 许可证

MIT 许可证 - 查看 [LICENSE](LICENSE) 文件了解详情。

## ⚠️ 免责声明

本工具仅供个人云盘管理使用。请遵守 PikPak 的服务条款和版权法。开发者不承担任何法律责任。

## 🙏 致谢

- [pikpakcli](https://github.com/52funny/pikpakcli) - 核心功能参考
- Go 语言社区 - 优秀的开发工具和库

---

如果这个项目对您有帮助，请给它一个 ⭐️！