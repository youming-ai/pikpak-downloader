#[PikPak Downloader](https://github.com/youming-ai/pikpak-downloader)

[![License](https://img.shields.io/github/license/youming-ai/pikpak-downloader)](LICENSE)

基於 Rust 編寫的高性能 **PikPak** 雲端硬碟命令列工具（CLI）和用戶端開發函式庫。

它提供了對檔案清單展示、帳戶配額查詢以及檔案或資料夾遞迴下載的完善支援。同時，它還實作了移植自 `pikpakcli` 的 MD5 簽章生成演算法，能夠自動處理人機驗證（Captcha Token）。

---

## 功能特性

- **CLI 與開發函式庫**：既可以直接作為獨立的命令列工具使用，也可以作為用戶端 Rust crate 接入到你自己專案中。
- **遞迴下載**：支援一鍵下載單個檔案或整個目錄樹，並自動保持原有的資料夾結構。
- **自動人機驗證與 Token 刷新**：在底層自動實作 PikPak 行動端的驗證簽章演算法（`X-Captcha-Token`）和 Token 輪轉，無需手動進行人機驗證。
- **代理支援**：支援 HTTP/HTTPS 代理設定，方便在受限的網路環境中使用。
- **詳細的檔案資訊**：豐富的檔案清單展示，支援詳細模式（`-l`）和易讀的檔案大小單位（`-h`）。

---

## 安裝

### 從原始碼編譯

確保你的系統已安裝 Rust 和 Cargo，然後執行：

```bash
# 複製儲存庫
git clone https://github.com/youming-ai/pikpak-downloader.git
cd pikpak-downloader

# 編譯 Release 版本二進位檔案
cargo build --release

# 編譯完成的二進位檔案位於：
./target/release/pikpak --help
```

### 安裝到系統路徑

```bash
cargo install --path .
```

---

## 配置

該程式會從環境變數或目前工作目錄下的 `.env` 檔案讀取配置。

設定方式如下：

```bash
# 複製環境設定檔案範例
cp .env.example .env
```

開啟 `.env` 檔案並填入相關參數：

```env
# 必填：你的 PikPak refresh token
PIKPAK_REFRESH_TOKEN=your_refresh_token_here

# 選填：HTTP/HTTPS 代理位址 (例如 http://127.0.0.1:7890)
PIKPAK_PROXY=

# 選填：如果需要覆蓋預設值，可以自訂 OAuth Client ID 和 Secret
PIKPAK_CLIENT_ID=
PIKPAK_CLIENT_SECRET=
```

### 如何取得 `PIKPAK_REFRESH_TOKEN`

1. 瀏覽 [PikPak 網頁版](https://mypikpak.com) 並登入你的帳戶。
2. 開啟瀏覽器的開發者工具（通常按 `F12` 或右鍵選擇「檢查」）。
3. 切換到 **Application** 面板（Chrome/Edge）或 **儲存 (Storage)** 面板（Firefox）。
4. 展開左側的 **Local Storage**，選擇 `https://mypikpak.com`。
5. 在右側的值中找到名為 `credentials` 的鍵，或者在其中搜尋 `refresh_token`。那是一串很長的字母與數字混合的字串。

---

## 命令列（CLI）使用指南

執行 `pikpak --help` 查看所有可用的指令和參數。

### 1. 查看空間配額 (Quota)

展示帳號的總容量、已用空間和剩餘空間。

```bash
# 易讀的單位格式（預設）
pikpak quota

# 輸出範例：
# total: 10.00 TiB
# used:  4.23 TiB
# free:  5.77 TiB
# usage: 42.3%

# 原始位元組大小格式
pikpak quota --raw
```

### 2. 列出檔案與資料夾 (List)

列出指定目錄路徑下的檔案。

```bash
# 列出根目錄 (/) 下的所有檔案
pikpak ls

# 列出指定路徑下的檔案
pikpak ls --path "/My Pack"

# 使用詳細模式展示 (-l) 並將檔案大小轉換為易讀單位 (-h)
pikpak ls --path "/My Pack" -l -h
```

### 3. 下載檔案與資料夾 (Download)

支援遞迴下載單個檔案或整個目錄。

```bash
# 下載單個檔案到預設資料夾 (./downloads)
pikpak download --path "/My Pack/video.mp4"

# 遞迴下載整個資料夾
pikpak download --path "/My Pack/Movies"

# 下載到自訂的本地目錄
pikpak download --path "/My Pack/video.mp4" --output "/path/to/local/dir"
```

---

## 開發函式庫使用指南 (Rust API)

你也可以將 `pikpak` 作為 Rust 函式庫匯入。在你的 `Cargo.toml` 中新增相依性，或使用本機路徑相依性。

```rust
use pikpak::{Client, FileKind};
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. 建構用戶端
    let client = Client::builder()
        .refresh_token("YOUR_REFRESH_TOKEN")
        .timeout(Duration::from_secs(30))
        .proxy("http://127.0.0.1:7890") // 選填代理
        .build()?;

    // 2. 查詢儲存空間配額
    let quota = client.quota().await?;
    println!("總空間: {} 位元組, 已使用: {} 位元組", quota.total, quota.used);

    // 3. 列出根目錄下的檔案
    let root_files = client.list_folder("").await?; // 根目錄使用空字串表示
    for file in root_files {
        println!("- {} (類型: {:?}, 大小: {} 位元組)", file.name, file.kind, file.size);
    }

    // 4. 解析路徑並取得下載連結
    let path = "/My Pack/video.mp4";
    let file_info = client.resolve_path_info(path).await?;
    if file_info.kind.is_file() {
        let download_info = client.get_download_url(&file_info.id).await?;
        println!("下載位址: {}", download_info.web_content_link);
    }

    Ok(())
}
```

---

## 開源授權

本專案基於 MIT 授權條款開源。詳情請參閱 [LICENSE](LICENSE) 檔案。
