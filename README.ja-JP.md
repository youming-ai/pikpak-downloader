#[PikPak Downloader](https://github.com/youming-ai/pikpak-downloader)

[![License](https://img.shields.io/github/license/youming-ai/pikpak-downloader)](LICENSE)

[English](README.md) | [繁體中文](README.zh-TW.md) | **日本語** | [한국어](README.ko-KR.md)

Rustで書かれた高性能な**PikPak**クラウドストレージ用のコマンドラインツール（CLI）およびクライアントライブラリです。

ファイル一覧の表示、アカウントの容量（クォータ）の確認、およびファイルやディレクトリ全体の再帰的なダウンロードを強力にサポートします。また、`pikpakcli`から移植されたMD5シグネチャ生成アルゴリズムにより、ログイン時のキャプチャ認証（Captcha Token）をバックグラウンドで自動的に解決します。

---

## 機能特徴

- **CLI & ライブラリ**：独立したコマンドラインツールとしても、Rustプロジェクトに組み込むクライアントライブラリとしても使用可能です。
- **再帰的ダウンロード**：フォルダ構造を保持したまま、ファイルやディレクトリ全体を簡単にダウンロードできます。
- **自動キャプチャ＆トークン管理**：PikPakモバイルクライアントの検証シグネチャアルゴリズム（`X-Captcha-Token`）およびトークンのローテーションを内部で自動処理するため、手動でのキャプチャ解決は不要です。
- **プロキシサポート**：HTTP/HTTPSプロキシの設定に対応しており、ネットワーク制限のある環境でも利用できます。
- **詳細なファイル情報**：詳細表示（`-l`）や読みやすいファイルサイズ表示（`-h`）オプションを備えた、高度な一覧表示機能を提供します。
- **安全なアトミックダウンロード**：サーバーから返されるファイル名をパストラバーサルに対してサニタイズし、各ファイルを一時的な `.part` ファイルにストリーミングして転送完了後にリネームします。ダウンロードが中断されても、最終的なファイル名で破損したファイルが残ることはありません。

---

## インストール

### ソースからのビルド

RustおよびCargoがインストールされていることを確認し、以下を実行します：

```bash
# リポジトリのクローン
git clone https://github.com/youming-ai/pikpak-downloader.git
cd pikpak-downloader

# リリースビルドの実行
cargo build --release

# ビルドされたバイナリは以下から実行できます：
./target/release/pikpak --help
```

### システムパスへのインストール

```bash
cargo install --path .
```

---

## 設定

本アプリケーションは、環境変数またはカレントディレクトリにある `.env` ファイルから設定を読み込みます。

セットアップ手順：

```bash
# 環境設定ファイルのサンプルをコピー
cp .env.example .env
```

`.env` ファイルを開き、必要な情報を記入します：

```env
# 必須：PikPakのリフレッシュトークン
PIKPAK_REFRESH_TOKEN=your_refresh_token_here

# 任意：HTTP/HTTPSプロキシのURL (例: http://127.0.0.1:7890)
PIKPAK_PROXY=

# 任意：デフォルト値を上書きしたい場合のOAuthクライアントIDとシークレット
PIKPAK_CLIENT_ID=
PIKPAK_CLIENT_SECRET=
```

### `PIKPAK_REFRESH_TOKEN` の取得方法

1. [PikPak Web版](https://mypikpak.com) にアクセスし、アカウントにログインします。
2. ブラウザの開発者ツールを開きます（通常は `F12` キーまたは右クリック -> 「検証」）。
3. **Application** タブ（Chrome/Edge）または **ストレージ (Storage)** タブ（Firefox）に移動します。
4. 左メニューから **Local Storage** -> `https://mypikpak.com` を選択します。
5. `credentials` という名前のキーを探すか、値から `refresh_token` を検索します。英数字が混ざった非常に長い文字列です。

---

## CLIの使い方

`pikpak --help` を実行すると、利用可能なすべてのコマンドとフラグが表示されます。

### 1. クォータ（使用容量）の確認

ストレージの総容量、使用量、空き容量を表示します。

```bash
# 読みやすい単位で表示（デフォルト）
pikpak quota

# 出力例：
# total: 10.00 TiB
# used:  4.23 TiB
# free:  5.77 TiB
# usage: 42.3%

# バイト単位で表示
pikpak quota --raw
```

### 2. ファイルとフォルダの一覧表示

指定したパス内のファイルを表示します。

```bash
# ルートフォルダ (/) 内のファイルを表示
pikpak ls

# 特定のパス内のファイルを表示
pikpak ls --path "/My Pack"

# 詳細表示 (-l) と読みやすいファイルサイズ表示 (-h) を併用
pikpak ls --path "/My Pack" -l -h
```

### 3. ファイルとフォルダのダウンロード

ファイルまたはディレクトリ全体を再帰的にダウンロードします。

```bash
# デフォルトのダウンロードフォルダ (./downloads) にファイルをダウンロード
pikpak download --path "/My Pack/video.mp4"

# ディレクトリを再帰的にダウンロード
pikpak download --path "/My Pack/Movies"

# ダウンロード先のディレクトリを指定する
pikpak download --path "/My Pack/video.mp4" --output "/path/to/local/dir"
```

---

## ライブラリとしての使用方法 (Rust API)

`pikpak` はライブラリとしても使用できます。`Cargo.toml` の依存関係に追加するか、ローカルパスで指定してください。

```rust
use pikpak::{Client, FileKind};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. クライアントの構築
    let client = Client::builder()
        .refresh_token("YOUR_REFRESH_TOKEN")
        .timeout(Duration::from_secs(30))
        .proxy("http://127.0.0.1:7890") // 任意のプロキシ
        .build()?;

    // 2. クォータの確認
    let quota = client.quota().await?;
    println!("Total quota: {} bytes, used: {} bytes", quota.total, quota.used);

    // 3. ルートディレクトリのファイル一覧を取得
    let root_files = client.list_folder("").await?; // ルートは空文字列で指定します
    for file in root_files {
        println!("- {} (Kind: {:?}, Size: {} bytes)", file.name, file.kind, file.size);
    }

    // 4. パスを解決してダウンロードURLを取得
    let path = "/My Pack/video.mp4";
    let file_info = client.resolve_path_info(path).await?;
    if file_info.kind.is_file() {
        let download_info = client.get_download_url(&file_info.id).await?;
        println!("Download URL: {}", download_info.web_content_link);
    }

    Ok(())
}
```

---

## ライセンス

このプロジェクトは MIT ライセンスのもとで公開されています。詳細は [LICENSE](LICENSE) ファイルをご覧ください。

