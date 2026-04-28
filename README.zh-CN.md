# pikpak

Rust CLI for PikPak 云盘。

## 配置

```bash
cp .env.example .env
# 编辑 .env: PIKPAK_REFRESH_TOKEN=your_token
```

获取 Refresh Token：[PikPak 网页版](https://mypikpak.com) → 开发者工具 → Application → Local Storage.

## 使用

```bash
# 列出文件
pikpak ls --path "/My Pack" -l -h

# 下载
pikpak download --path "/My Pack/video.mp4"

# 查看配额
pikpak quota
```

## 许可

MIT
