# PikPak Downloader

[English](./README.md) | [中文](./README_CN.md) | [日本語](./README_JA.md)

A Python tool for batch downloading folders and files from PikPak share links.

## Features

- 🚀 Recursively download entire folders and all files from PikPak share links
- ⚡ Multi-threaded concurrent downloads for faster speed
- 🔄 Resume interrupted downloads with automatic retry
- 📊 Detailed progress bars and download statistics
- 📁 Customizable download directory (default: `/Download`)
- 🔐 No need to save files to your own PikPak account
- 🛡️ Smart error handling and network exception recovery

## System Requirements

- Python 3.7+
- macOS / Linux / Windows
- Stable internet connection

## Installation

1. Clone this repository:
   ```bash
   git clone https://github.com/your-username/pikpak-downloader.git
   cd pikpak-downloader
   ```
2. Install dependencies:
   ```bash
   pip install -r requirements.txt
   ```

## Usage

1. Create a `.env` file and set your PikPak account credentials:
   ```env
   PIKPAK_USERNAME=your_username
   PIKPAK_PASSWORD=your_password
   ```
2. Run the downloader:
   ```bash
   python pikpak_downloader.py "https://mypikpak.com/s/your-share-link"
   ```

## Notes

- Ensure you have enough disk space
- For large files, use a stable internet connection
- Please comply with PikPak's terms of service and limitations