# PikPak 個人クラウド管理ツール v4.0 🚀

PikPak個人クラウドストレージを管理する高性能なGo CLIツール。

## ✨ 機能

- **📁 ファイル管理** - クラウドファイルの一覧表示、閲覧、整理
- **💾 ファイルダウンロード** - 個別ファイルまたはフォルダ全体をダウンロード
- **📊 ストレージ監視** - リアルタイムのクォータと使用量情報
- **⚡ 高性能** - 進捗表示付き同時ダウンロード
- **🔒 セキュア設定** - 環境変数ベースの認証

## 🚀 クイックスタート

### 1. 依存関係のインストール
```bash
make deps
```

### 2. 認証情報の設定
```bash
cp .env.example .env
```

`.env` ファイルを編集：
```bash
# 方式1: アカウントとパスワード
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]

# 方式2: RefreshToken（推奨）
PIKPAK_REFRESH_TOKEN=[your_refresh_token]
```

### 3. ビルドと実行
```bash
make build-cli
./pikpak-cli help
```

## 📋 コマンド

### ファイル一覧
```bash
./pikpak-cli ls                    # ルートディレクトリ
./pikpak-cli ls -path "/My Pack"   # 特定のフォルダ
./pikpak-cli ls -l -h              # 詳細表示
```

### ストレージクォータ
```bash
./pikpak-cli quota                 # ストレージ使用量を表示
```

### ファイルダウンロード
```bash
./pikpak-cli download -path "/My Pack/file.pdf"                    # 単一ファイル
./pikpak-cli download -path "/My Pack" -output "./downloads"      # フォルダ全体
./pikpak-cli download -path "/My Pack" -count 5                   # 同時実行数設定
```

## 📁 プロジェクト構造

```
pikpak-downloader/
├── pikpak_cli.go           # CLIインターフェース
├── pikpak_client.go        # コアクライアント機能
├── config_manager.go       # 設定管理
├── .env.example            # 設定テンプレート
├── Makefile                # ビルド自動化
└── README*.md              # ドキュメント
```

## ⚙️ 設定

### 環境変数 (.env)
```bash
# 認証
PIKPAK_USERNAME=[your_email]
PIKPAK_PASSWORD=[your_password]
# または
PIKPAK_REFRESH_TOKEN=[your_refresh_token]

# オプション
PIKPAK_PROXY=http://127.0.0.1:7890
PIKPAK_DEVICE_NAME=pikpak-downloader
```

### RefreshTokenの取得方法
1. PikPakウェブ版にログイン
2. 開発者ツールを開く（F12）
3. `Application` → `Local Storage` に移動
4. `refresh_token` の値をコピー
5. `.env` ファイルに追加

## 🔄 バージョン履歴

### v4.0.0 (2025-10-18) 🎯
- **個人クラウド管理** - ファイル管理に特化した完全な書き直し
- **CLIインターフェース** - ヘルプシステム付き完全なコマンドラインインターフェース
- **スマートファイル分類** - 自動ファイルタイプ認識
- **環境変数設定** - セキュアな.envベース設定

### v3.1.0 (2025-10-18) 🌟
- .env設定サポートを追加
- 自動設定生成
- セキュリティと使いやすさの向上

## 🛠️ 開発

```bash
make build-cli    # CLIツールをビルド
make clean        # ビルドアーティファクトをクリア
make run-cli ls   # 例コマンドで実行
```

## 🤝 貢献

1. プロジェクトをFork
2. 機能ブランチを作成 (`git checkout -b feature/AmazingFeature`)
3. 変更をコミット (`git commit -m 'Add AmazingFeature'`)
4. ブランチにプッシュ (`git push origin feature/AmazingFeature`)
5. Pull Requestを開く

## 📄 ライセンス

MITライセンス - 詳細は [LICENSE](LICENSE) ファイルを確認してください。

## ⚠️ 免責事項

このツールは個人クラウド管理専用です。PikPakのサービス規約と著作権法を遵守してください。開発者は一切の法的責任を負いません。

## 🙏 謝辞

- [pikpakcli](https://github.com/52funny/pikpakcli) - コア機能の参考
- Go言語コミュニティ - 優れた開発ツールとライブラリ

---

このプロジェクトが役に立った場合は、⭐️を付けてください！