# pikpak

Rust CLI for PikPak cloud storage.

## Install

```bash
cargo build --release
./target/release/pikpak --help
```

## Setup

```bash
cp .env.example .env
# edit .env: PIKPAK_REFRESH_TOKEN=your_token
```

Get refresh token: [PikPak web](https://mypikpak.com) → DevTools → Application → Local Storage.

## Usage

```bash
# List files
pikpak ls --path "/My Pack" -l -h

# Download
pikpak download --path "/My Pack/video.mp4"

# Quota
pikpak quota
```

## License

MIT
