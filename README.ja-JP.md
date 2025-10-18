# PikPak 個人クラウド管理ツール v4.0 🚀

Go言語で記述されたPikPak個人クラウド管理ツール。PikPakクラウドファイルを管理するための完全なコマンドラインインターフェースを提供します。

## ✨ コア機能

### 🎯 個人クラウド管理
- **ファイル一覧** - 個人クラウド内のファイルやフォルダを閲覧・表示
- **クォータ確認** - クラウド容量の使用状況をリアルタイムで表示
- **ファイルダウンロード** - 個人クラウド内の任意のファイルまたはフォルダ全体をダウンロード
- **スマート分類** - ファイルタイプを自動識別（動画、画像、ドキュメントなど）

### 🔧 技術的優位性
- **Go言語開発** - 高性能、低リソース消費
- **コマンドラインインターフェース** - シンプルで使いやすいCLIツール
- **環境変数設定** - 安全な設定管理ソリューション
- **同時ダウンロード** - マルチスレッド同時ダウンロードをサポート
- **進捗表示** - リアルタイムでダウンロード進捗を表示

## 🚀 クイックスタート

### 1. 依存関係のインストール
```bash
make deps
```

### 2. 認証情報の設定

設定ファイルを作成：
```bash
cp .env.example .env
```

`.env` ファイルを編集し、PikPak認証情報を入力：
```bash
# 方式1: アカウントとパスワードを使用
PIKPAK_USERNAME=[あなたのメールアドレス]
PIKPAK_PASSWORD=[あなたのパスワード]

# 方式2: RefreshTokenを使用（推奨）
PIKPAK_REFRESH_TOKEN=[あなたのrefresh_token]
```

### 3. プログラムのコンパイル
```bash
make build-cli
```

### 4. 使用開始
```bash
# ヘルプを表示
./pikpak-cli help

# クラウドクォータを確認
./pikpak-cli quota

# ルートディレクトリのファイルを一覧表示
./pikpak-cli ls

# 指定ディレクトリを一覧表示
./pikpak-cli ls -path "/My Pack"

# 詳細な一覧
./pikpak-cli ls -path "/My Pack" -l -h

# ファイルをダウンロード
./pikpak-cli download -path "/My Pack/document.pdf"

# フォルダ全体をダウンロード
./pikpak-cli download -path "/My Pack" -output "./downloads"
```

## 📋 コマンド詳細

### `ls` - ファイル一覧
```bash
./pikpak-cli ls [オプション]

オプション:
  -path string     ディレクトリパス (デフォルト: "/")
  -l               詳細表示
  -h               人間が読みやすい形式

例:
  ./pikpak-cli ls                          # ルートディレクトリを一覧表示
  ./pikpak-cli ls -path "/My Pack"        # 指定ディレクトリを一覧表示
  ./pikpak-cli ls -l -h                     # 詳細形式
```

### `quota` - クォータ確認
```bash
./pikpak-cli quota [オプション]

オプション:
  -h               人間が読みやすい形式 (デフォルト: true)

例:
  ./pikpak-cli quota                       # クォータ情報を確認
```

### `download` - ファイルダウンロード
```bash
./pikpak-cli download [オプション]

オプション:
  -path string     ダウンロードパス (デフォルト: "/")
  -output string   出力ディレクトリ (デフォルト: "./downloads")
  -count int       同時実行数 (デフォルト: 3)
  -progress        進捗表示 (デフォルト: true)

例:
  ./pikpak-cli download -path "/My Pack/video.mp4"                    # 単一ファイルをダウンロード
  ./pikpak-cli download -path "/My Pack" -output "./my_downloads"   # 指定ディレクトリにダウンロード
  ./pikpak-cli download -path "/My Pack" -count 5                    # 同時実行数を設定
```

## 🛠️ 開発・ビルド

### コンパイル
```bash
make build-cli
```

### 実行
```bash
# Makefileを使用
make run-cli ls
make run-cli quota
make run-cli download -path "/My Pack"

# 直接実行
./pikpak-cli ls
```

### クリーンアップ
```bash
make clean
```

## 📊 機能デモ

### クォータ情報の確認
```bash
$ ./pikpak-cli quota
📊 クラウドクォータ情報:
総容量: 6.0GB
使用量: 604.2MB
使用率: 9.8%
```

### ファイル一覧
```bash
$ ./pikpak-cli ls
フォルダ      My Pack
フォルダ      Pack From Shared

$ ./pikpak-cli ls -path "/Pack From Shared"
フォルダ      onlyfans chaeira 34V
```

### 詳細一覧
```bash
$ ./pikpak-cli ls -l -h
タイプ        サイズ     更新時間            ファイル名
フォルダ      -          2025-01-02 15:04   My Pack
フォルダ      -          2025-01-01 10:30   Pack From Shared
```

## 📁 プロジェクト構造

```
pikpak-downloader/
├── pikpak_cli.go           # CLIコマンドラインインターフェース
├── pikpak_client.go        # PikPakクライアントコア機能
├── config_manager.go       # 設定管理
├── .env                     # ユーザー設定ファイル
├── .env.example            # 設定ファイルテンプレート
├── pikpak-cli              # 実行可能ファイル
├── Makefile                 # ビルドスクリプト
├── go.mod                   # Goモジュールファイル
├── go.sum                   # 依存関係検証ファイル
├── README.ja-JP.md          # 日本語プロジェクト説明
└── .gitignore               # Git無視ファイル
```

## ⚙️ 設定説明

### 環境変数設定
`.env` ファイルに以下の情報を設定：

```bash
# PikPak アカウント認証
PIKPAK_USERNAME=[あなたのメールアドレス]
PIKPAK_PASSWORD=[あなたのパスワード]

# または RefreshToken を使用（推奨）
PIKPAK_REFRESH_TOKEN=[あなたのrefresh_token]

# プロキシ設定（オプション）
# PIKPAK_PROXY=http://127.0.0.1:7890

# デバイス設定（オプション）
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### RefreshToken の取得方法
1. PikPakウェブ版にログイン
2. `F12` キーで開発者ツールを開く
3. `Application` → `Local Storage` に移動
4. `refresh_token` フィールドを探して値をコピー
5. `.env` ファイルの `PIKPAK_REFRESH_TOKEN` に設定

## 🔄 バージョン履歴

### v4.0.0 (2025-10-18) 🎯
- ✨ **プロジェクトの再設計** - 個人クラウド管理に特化
- 🎯 **pikpakcli 機能の複製** - ファイル一覧、クォータ確認、ファイルダウンロード
- 🔧 **完全なCLIインターフェース** - 引数解析、ヘルプシステム
- 📋 **スマートファイル分類** - ファイルタイプを自動識別
- ⚙️ **設定管理の最適化** - 環境変数設定ソリューション

### v3.1.0 (2025-10-18) 🌟
- ✨ **.env 設定サポート** - 環境変数設定ソリューションを追加、より安全で便利
- 🔄 **自動設定生成** - プログラムが.envを自動的に読み取り、pikpakcli設定ファイルを生成
- 📋 **設定状態チェック** - 詳細な設定検証と状態表示
- 🔧 **設定マネージャー** - config_manager.goモジュールを追加
- 🎯 **デフォルトCLIモード** - ハイブリッドツールがデフォルトでCLIモードを使用、3つのモード選択をサポート
- 📁 **設定ファイルテンプレート** - .env.exampleテンプレートファイルを提供

### v3.x.x (共有リンクダウンロード)
- ハイブリッドダウンロードモード
- ウェブクローラー機能
- 共有リンク処理

### v2.x.x (共有リンクダウンロード)
- Goバージョンでの書き直し
- 基本的なダウンロード機能

### v1.x.x (Pythonバージョン)
- 初期実装

## 🤝 貢献

IssueとPull Requestを歓迎します！

1. 本プロジェクトをFork
2. 機能ブランチを作成 (`git checkout -b feature/AmazingFeature`)
3. 変更をコミット (`git commit -m 'Add some AmazingFeature'`)
4. ブランチにプッシュ (`git push origin feature/AmazingFeature`)
5. Pull Requestを開く

## 📄 ライセンス

本プロジェクトはMITライセンスを採用しています - 詳細は [LICENSE](LICENSE) ファイルを確認してください。

## ⚠️ 免責事項

このツールは個人クラウド管理専用です。PikPakの利用規約を遵守し、商用目的や著作権法に違反する内容には使用しないでください。開発者は一切の法的責任を負いません。

## 🙏 謝辞

- [pikpakcli](https://github.com/52funny/pikpakcli) - コア機能の参考
- Go言語コミュニティ - 優れた開発ツールとライブラリ

---

このプロジェクトがお役に立てれば、⭐️をお願いします！