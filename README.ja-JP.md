# PikPak 個人クラウド管理ツール

**PikPak API を直接呼び出すネイティブ Rust CLI ツール。**

> 外部 `pikpakcli` バイナリは不要 — すべてネイティブ API 実装です。

## 🌍 言語

- 🇺🇸 **[English](./README.md)** | 🇨🇳 **[中文](./README.zh-CN.md)** | 🇯🇵 **日本語** | 🇰🇷 **[한국어](./README.ko-KR.md)**

## ✨ 機能

### 📁 ファイル管理
- **`ls --path "/My Pack"`** — パスベースのナビゲーション（内部でフォルダIDを解決）
- **長形式** (`-l`) で種類とサイズを表示
- **人間が読みやすいサイズ** (`-h`): KB / MB / GB

### ⬇️ ダウンロード
- **ネイティブダウンロード** — API の直接ダウンロードURLをストリーミング
- **単一ファイル** または **再帰的フォルダ** ダウンロード
- **進捗表示** にパーセンテージ

### 📊 アカウント情報
- **`quota`** — ストレージ使用量（人間が読みやすい形式または生のバイト）

### ⚙️ ビルド
- **単一 ~3.9 MB バイナリ**（静的リンク、ランタイム不要）
- **クロスプラットフォーム** Rust（Linux、macOS、Windows）

## 🚀 クイックスタート

### 前提条件
- **Rust 1.78+**（ソースからビルド）
- **PikPak アカウント** と refresh token

### ソースからインストール

```bash
git clone https://github.com/your-username/pikpak-downloader.git
cd pikpak-downloader
make build
```

バイナリは `rust/target/release/pikpak-cli` に生成されます。

### 認証の設定

```bash
cp .env.example .env
# .env を編集:
PIKPAK_REFRESH_TOKEN=your_refresh_token
# オプション: PIKPAK_PROXY=http://127.0.0.1:7890
```

### 使用法

```bash
# ルートディレクトリを一覧
make run ARGS='ls'

# 詳細ビュー + 人間が読みやすいサイズ
make run ARGS='ls --path "/My Pack" -l -h'

# ファイルをダウンロード
make run ARGS='download --path "/My Pack/video.mp4" --output ./downloads'

# クォータを確認
make run ARGS='quota'
```

或者直接二进制文件：

```bash
./rust/target/release/pikpak-cli help
```

## 📋 CLI リファレンス

### `ls` — ファイルとディレクトリの一覧

| オプション | 説明 |
|------|------|
| `--path <path>` | ディレクトリパス（例: `"/My Pack"`）（デフォルト: `/`） |
| `-l, --long` | 長形式（種類とサイズ） |
| `-h, --human` | 人間が読みやすいサイズ（KB / MB / GB） |

```bash
pikpak-cli ls --path "/My Pack" -l -h
```

### `download` — ファイルまたはフォルダのダウンロード

| オプション | 説明 |
|------|------|
| `--path <path>` | ダウンロードするリモートパス（必須） |
| `--output <dir>` | ローカル出力ディレクトリ（デフォルト: `./downloads`） |
| `--count <n>` | 並行度ヒント（デフォルト: `3`） |

```bash
# 単一ファイル
pikpak-cli download --path "/My Pack/document.pdf" --output ./downloads

# フォルダ全体（再帰）
pikpak-cli download --path "/My Photos" --output ./backups
```

### `quota` — ストレージクォータの表示

| オプション | 説明 |
|------|------|
| `--raw` | 生のバイト数を出力（人間が読みやすい形式ではなく） |

```bash
pikpak-cli quota
pikpak-cli quota --raw
```

## 🏗️ 動作原理

1. **認証** — refresh token で短期 access token を取得。トークンは毎回ローテーションされます。`TokenManager::current_refresh_token` で取得・永続化できます。
2. **キャプチャ** — 各 drive API 呼び出しに対して `X-Captcha-Token` を取得。期限切れ時に自動更新。
3. **パス解決** — `list_folder` で各セグメントを辿り、`"/My Pack/videos"` → `folder_id` を解決。
4. **ダウンロード** — API から `web_content_link` を取得し、ストリーミングでディスクに書き込み、進捗バーを表示。

## 📁 プロジェクト構成

```
pikpak-downloader/
├── rust/
│   ├── Cargo.toml
│   ├── Cargo.lock
│   ├── pikpak-api/          # ライブラリcrate（認証、キャプチャ、APIクライアント）
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── auth.rs        # OAuth2 リフレッシュトークンフロー
│   │       ├── captcha.rs     # MD5 チェーン署名計算
│   │       ├── client.rs      # API: quota, list_folder, resolve_path, download
│   │       ├── error.rs       # エラー列挙型
│   │       └── types.rs       # FileInfo, FileKind, Quota
│   └── pikpak-cli/            # バイナリcrate（CLIフロントエンド）
│       └── src/main.rs        # Clap CLI + tokio 非同期ランタイム
├── .env.example
├── Makefile
├── README.md
└── ...
```

## ⚙️ 設定

### 環境変数（`.env`）

```bash
# 必須
PIKPAK_REFRESH_TOKEN=your_refresh_token

# オプション
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader

# OAuth 上書き（未設定の場合はデフォルト値を使用）
# PIKPAK_CLIENT_ID=
# PIKPAK_CLIENT_SECRET=
```

### Refresh Token の取得方法

1. [PikPak Web版](https://mypikpak.com) にログイン
2. 開発者ツールを開く (F12)
3. **Application → Local Storage → `https://mypikpak.com`** に移動
4. `refresh_token` の値をコピー
5. `.env` に貼り付け: `PIKPAK_REFRESH_TOKEN=...`

## 🔄 最近の変更

| コミット | 変更点 |
|------|------|
| `38f65ac` | **Rust 完全書き換え** — Go wrapper を削除（外部 `pikpakcli` バイナリ不要） |
| — | ネイティブダウンロードとパス解決を追加 |
| — | PrintStats ゼロ除算 panic、quoteString YAML エスケープ bug、PerformanceMetrics 混合同步、detectFileType 重複割り当て、ListFilesStream stdout クローズ問題、Rust token リフレッシュストーム、captcha double-check 競合を修正 |

## 📄 ライセンス

MIT — 詳細は [LICENSE](LICENSE) を参照。

## 🙏 謝辞

- エンドポイントとキャプチャロジックは [`pikpakcli`](https://github.com/52funny/pikpakcli)（Go）からリバースエンジニアリングし、ネイティブ Rust に移植しました。
- PikPak は公式 API を公開していないため、エンドポイントは予告なく変更される可能性があります。
