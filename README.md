# pikpak

Rust CLI for PikPak cloud storage. Native API — no external binary.

## Install

```bash
git clone https://github.com/youming-ai/pikpak-downloader.git
cd pikpak-downloader
cargo build --release
# Binary: target/release/pikpak
```

## Setup

```bash
cp .env .env.example   # edit: PIKPAK_REFRESH_TOKEN=your_token
```

Get refresh token from [PikPak web](https://mypikpak.com) → DevTools → Application → Local Storage.

## Commands

```bash
# List files
pikpak ls --path "/My Pack" -l -h

# Download
pikpak download --path "/My Pack/video.mp4" --output ./dl

# Quota
pikpak quota
```

## License

MIT
