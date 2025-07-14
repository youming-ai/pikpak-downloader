#!/usr/bin/env python3
import os
import sys
import json
import time
import requests
from pathlib import Path
from typing import Dict, List, Optional
from urllib.parse import urlparse, parse_qs
from dotenv import load_dotenv
from tqdm import tqdm
import threading
from concurrent.futures import ThreadPoolExecutor, as_completed

# 导入优化模块
from config import config
from exceptions import *
from logger import init_logging, get_logger, DownloadLogger
from utils import validate_share_url, sanitize_filename, format_size

class PikPakDownloader:
    BASE_URL = "https://api-drive.mypikpak.com/v1"
    SHARE_BASE_URL = BASE_URL
    def __init__(self, max_workers: int = 2):
        self.session = requests.Session()
        self.session.headers.update({
            'User-Agent': 'Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36'
        })
        self.max_workers = max_workers

    def get_share_info(self, share_url: str) -> Dict:
        """获取分享链接信息"""
        parsed_url = urlparse(share_url)
        # /s/<share_id>/<file_id>
        path_parts = parsed_url.path.strip('/').split('/')
        if len(path_parts) < 3 or path_parts[0] != 's':
            raise ValueError("分享链接格式不正确")
        share_id = path_parts[1]
        file_id = path_parts[2]
        
        url = f"{self.SHARE_BASE_URL}/share/{share_id}"
        
        # 添加重试机制
        for attempt in range(3):
            try:
                response = self.session.get(url, timeout=30)
                response.raise_for_status()
                info = response.json()
                info['share_id'] = share_id
                info['file_id'] = file_id
                return info
            except requests.exceptions.RequestException as e:
                if attempt == 2:
                    raise e
                print(f"获取分享信息失败，重试中... ({attempt + 1}/3)")
                time.sleep(2)
    
    def get_share_files(self, share_id: str, parent_id: str = "root") -> List[Dict]:
        """获取分享文件列表，支持分页"""
        url = f"{self.SHARE_BASE_URL}/share/{share_id}/files"
        params = {
            "parent_id": parent_id,
            "page_token": "",
            "page_size": 100,
            "with_audit": "true",
            "thumbnail_size": "SIZE_MEDIUM"
        }
        
        files = []
        page_count = 0
        
        while True:
            try:
                response = self.session.get(url, params=params, timeout=30)
                response.raise_for_status()
                data = response.json()
                
                current_files = data.get("files", [])
                files.extend(current_files)
                page_count += 1
                
                print(f"已获取第 {page_count} 页，共 {len(current_files)} 个文件")
                
                if not data.get("next_page_token"):
                    break
                params["page_token"] = data["next_page_token"]
                
            except requests.exceptions.RequestException as e:
                print(f"获取文件列表失败: {e}")
                break
                
        return files
    
    def get_download_url(self, share_id: str, file_id: str) -> str:
        """获取文件下载链接"""
        url = f"{self.SHARE_BASE_URL}/share/{share_id}/download"
        params = {"file_id": file_id}
        
        for attempt in range(3):
            try:
                response = self.session.get(url, params=params, timeout=30)
                response.raise_for_status()
                return response.json()["download_url"]
            except requests.exceptions.RequestException as e:
                if attempt == 2:
                    raise e
                print(f"获取下载链接失败，重试中... ({attempt + 1}/3)")
                time.sleep(1)
    
    def download_file(self, url: str, filepath: Path, chunk_size: int = 8192) -> bool:
        """下载单个文件，支持断点续传"""
        try:
            # 确保目录存在
            filepath.parent.mkdir(parents=True, exist_ok=True)
            
            # 获取文件大小
            response = self.session.head(url, timeout=30)
            total_size = int(response.headers.get("content-length", 0))
            
            # 检查是否已存在部分下载的文件
            initial_pos = filepath.stat().st_size if filepath.exists() else 0
            
            if initial_pos >= total_size and total_size > 0:
                print(f"文件 {filepath.name} 已完整下载")
                return True
            
            # 设置断点续传头
            headers = {"Range": f"bytes={initial_pos}-"} if initial_pos > 0 else {}
            
            with self.session.get(url, headers=headers, stream=True, timeout=30) as response:
                response.raise_for_status()
                
                # 创建进度条
                progress = tqdm(
                    total=total_size,
                    initial=initial_pos,
                    unit="iB",
                    unit_scale=True,
                    desc=filepath.name[:50]  # 限制文件名长度
                )
                
                mode = "ab" if initial_pos > 0 else "wb"
                with open(filepath, mode) as f:
                    for chunk in response.iter_content(chunk_size=chunk_size):
                        if chunk:
                            size = f.write(chunk)
                            progress.update(size)
                
                progress.close()
                print(f"✓ 下载完成: {filepath.name}")
                return True
                
        except Exception as e:
            print(f"✗ 下载失败 {filepath.name}: {e}")
            return False
    
    def download_files_batch(self, share_id: str, files: List[Dict], output_path: Path) -> None:
        def download_single_file(file_info):
            try:
                file_path = output_path / file_info["name"]
                download_url = self.get_download_url(share_id, file_info["id"])
                return self.download_file(download_url, file_path), file_info["name"]
            except Exception as e:
                print(f"下载文件 {file_info['name']} 时出错: {e}")
                return False, file_info["name"]
        with ThreadPoolExecutor(max_workers=self.max_workers) as executor:
            results = list(tqdm(executor.map(download_single_file, files), total=len(files), desc="批量下载"))
        success_count = sum(1 for success, _ in results if success)
        print(f"\n批量下载完成: {success_count}/{len(files)} 个文件成功下载")

    def download_folder_recursive(self, share_id: str, folder_id: str, output_path: Path) -> None:
        files = self.get_share_files(share_id, parent_id=folder_id)
        if not files:
            print(f"文件夹为空: {output_path}")
            return
        file_items = [f for f in files if f["kind"] == "drive#file"]
        folder_items = [f for f in files if f["kind"] == "drive#folder"]
        if file_items:
            self.download_files_batch(share_id, file_items, output_path)
        for folder in folder_items:
            self.download_folder_recursive(share_id, folder["id"], output_path / folder["name"])

    def process_share(self, share_url: str, output_dir: str = "Download") -> None:
        try:
            share_info = self.get_share_info(share_url)
            share_id = share_info["share_id"]
            share_name = share_info.get("share_name", "PikPak_Download")
            output_path = Path(output_dir) / share_name
            output_path.mkdir(parents=True, exist_ok=True)
            self.download_folder_recursive(share_id, share_info["file_id"], output_path)
            print(f"\n🎉 所有文件已下载到: {output_path}")
        except Exception as e:
            print(f"❌ 错误: {e}")
            sys.exit(1)

def main():
    if len(sys.argv) < 2:
        print("用法: python pikpak_downloader.py <分享链接> [下载目录]")
        sys.exit(1)
    share_url = sys.argv[1]
    output_dir = sys.argv[2] if len(sys.argv) > 2 else "Download"
    PikPakDownloader().process_share(share_url, output_dir)

if __name__ == "__main__":
    main()