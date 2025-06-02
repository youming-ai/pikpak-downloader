# PikPak Downloader

[English README](./README_EN.md)

一个用于从 PikPak 分享链接批量下载文件夹和文件的 Python 工具。

## 功能特点

- 🚀 递归下载 PikPak 分享链接中的整个文件夹及所有文件
- ⚡ 多线程并发下载，提升下载速度
- 🔄 支持断点续传与自动重试
- 📊 显示详细的下载进度条和统计信息
- 📁 支持自定义下载目录（默认 `/Download`）
- 🔐 无需将文件保存到自己的 PikPak 账号
- 🛡️ 智能错误处理和网络异常恢复

## 系统要求

- Python 3.7+
- macOS / Linux / Windows
- 稳定的网络连接

## 安装

1. 克隆此仓库：
   ```bash
   git clone https://github.com/your-username/pikpak-downloader.git
   cd pikpak-downloader
   ```
2. 安装依赖：
   ```bash
   pip install -r requirements.txt
   ```

## 使用方法

1. 创建 `.env` 文件并设置你的 PikPak 账号信息：
   ```env
   PIKPAK_USERNAME=your_username
   PIKPAK_PASSWORD=your_password
   ```
2. 运行下载器：
   ```bash
   python pikpak_downloader.py "https://mypikpak.com/s/your-share-link"
   ```

## 注意事项

- 请确保有足够的磁盘空间
- 下载大文件时建议使用稳定的网络连接
- 请遵守 PikPak 的使用条款和限制