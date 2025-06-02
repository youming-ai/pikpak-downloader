# PikPak Downloader

[English](./README.md) | [中文](./README_CN.md) | [日本語](./README_JA.md)

PikPak共有リンクからフォルダやファイルを一括ダウンロードできるPythonツールです。

## 特徴

- 🚀 PikPak共有リンク内のフォルダやファイルを再帰的にダウンロード
- ⚡ マルチスレッドによる高速ダウンロード
- 🔄 中断したダウンロードの再開と自動リトライ対応
- 📊 詳細な進捗バーとダウンロード統計情報
- 📁 ダウンロード先ディレクトリをカスタマイズ可能（デフォルトは `/Download`）
- 🔐 ファイルを自分のPikPakアカウントに保存する必要なし
- 🛡️ 賢いエラー処理とネットワーク障害からの復旧

## システム要件

- Python 3.7以上
- macOS / Linux / Windows
- 安定したインターネット接続

## インストール

1. このリポジトリをクローンします:
   ```bash
   git clone https://github.com/your-username/pikpak-downloader.git
   cd pikpak-downloader
   ```
2. 依存パッケージをインストールします:
   ```bash
   pip install -r requirements.txt
   ```

## 使い方

1. `.env` ファイルを作成し、PikPakアカウント情報を設定します:
   ```env
   PIKPAK_USERNAME=your_username
   PIKPAK_PASSWORD=your_password
   ```
2. ダウンローダーを実行します:
   ```bash
   python pikpak_downloader.py "https://mypikpak.com/s/your-share-link"
   ```

## 注意事項

- 十分なディスク容量を確保してください
- 大きなファイルをダウンロードする場合は安定したネット接続を利用してください
- PikPakの利用規約や制限を遵守してください