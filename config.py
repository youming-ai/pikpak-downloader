#!/usr/bin/env python3
"""
配置管理模块
"""
import os
from pathlib import Path
from typing import Optional
from dataclasses import dataclass


@dataclass
class DownloadConfig:
    """下载配置类"""
    # 网络配置
    max_workers: int = 4
    timeout: int = 30
    max_retries: int = 3
    retry_delay: float = 2.0
    chunk_size: int = 8192
    
    # 文件配置
    output_dir: str = "Download"
    create_subdirs: bool = True
    overwrite_existing: bool = False
    
    # API 配置
    base_url: str = "https://api-drive.mypikpak.com/v1"
    user_agent: str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/91.0.4472.124 Safari/537.36"
    
    # 分页配置
    page_size: int = 100
    
    # 日志配置
    log_level: str = "INFO"
    log_file: Optional[str] = None
    
    @classmethod
    def from_env(cls) -> "DownloadConfig":
        """从环境变量创建配置"""
        return cls(
            max_workers=int(os.getenv("PIKPAK_MAX_WORKERS", cls.max_workers)),
            timeout=int(os.getenv("PIKPAK_TIMEOUT", cls.timeout)),
            max_retries=int(os.getenv("PIKPAK_MAX_RETRIES", cls.max_retries)),
            chunk_size=int(os.getenv("PIKPAK_CHUNK_SIZE", cls.chunk_size)),
            output_dir=os.getenv("PIKPAK_OUTPUT_DIR", cls.output_dir),
            log_level=os.getenv("PIKPAK_LOG_LEVEL", cls.log_level),
            log_file=os.getenv("PIKPAK_LOG_FILE"),
        )
    
    def validate(self) -> None:
        """验证配置参数"""
        if self.max_workers < 1:
            raise ValueError("max_workers 必须大于 0")
        if self.timeout < 1:
            raise ValueError("timeout 必须大于 0")
        if self.max_retries < 0:
            raise ValueError("max_retries 不能小于 0")
        if self.chunk_size < 1024:
            raise ValueError("chunk_size 不能小于 1024")
        if self.page_size < 1 or self.page_size > 1000:
            raise ValueError("page_size 必须在 1-1000 之间")


# 全局配置实例
config = DownloadConfig.from_env()