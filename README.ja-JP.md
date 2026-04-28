# pikpak

Rust CLI for PikPak.

## インストール

```bash
cargo build --release
./target/release/pikpak --help
```

## セットアップ

```bash
cp .env.example .env
# 編集: PIKPAK_REFRESH_TOKEN=your_token
```

Refresh Token の取得: [PikPak Web](https://mypikpak.com) → DevTools → Application → Local Storage.

## 使い方

```bash
# ファイル一覧
pikpak ls --path "/My Pack" -l -h

# ダウンロード
pikpak download --path "/My Pack/video.mp4"

# クォータ
pikpak quota
```

## ライセンス

MIT
