#!/usr/bin/env python3
"""
日志系统模块
"""
import logging
import sys
from pathlib import Path
from typing import Optional
from datetime import datetime


class ColoredFormatter(logging.Formatter):
    """彩色日志格式化器"""
    
    # 颜色代码
    COLORS = {
        'DEBUG': '\033[36m',    # 青色
        'INFO': '\033[32m',     # 绿色
        'WARNING': '\033[33m',  # 黄色
        'ERROR': '\033[31m',    # 红色
        'CRITICAL': '\033[35m', # 紫色
        'RESET': '\033[0m'      # 重置
    }
    
    def format(self, record):
        # 添加颜色
        if record.levelname in self.COLORS:
            record.levelname = f"{self.COLORS[record.levelname]}{record.levelname}{self.COLORS['RESET']}"
        return super().format(record)


def setup_logger(name: str = "pikpak_downloader", 
                level: str = "INFO", 
                log_file: Optional[str] = None,
                console_output: bool = True) -> logging.Logger:
    """
    设置日志系统
    
    Args:
        name: 日志器名称
        level: 日志级别
        log_file: 日志文件路径（可选）
        console_output: 是否输出到控制台
    
    Returns:
        配置好的日志器
    """
    logger = logging.getLogger(name)
    logger.setLevel(getattr(logging, level.upper()))
    
    # 清除现有处理器
    logger.handlers.clear()
    
    # 日志格式
    detailed_format = "%(asctime)s - %(name)s - %(levelname)s - %(filename)s:%(lineno)d - %(message)s"
    simple_format = "%(levelname)s - %(message)s"
    
    # 控制台处理器
    if console_output:
        console_handler = logging.StreamHandler(sys.stdout)
        console_handler.setLevel(logging.INFO)
        console_formatter = ColoredFormatter(simple_format)
        console_handler.setFormatter(console_formatter)
        logger.addHandler(console_handler)
    
    # 文件处理器
    if log_file:
        log_path = Path(log_file)
        log_path.parent.mkdir(parents=True, exist_ok=True)
        
        file_handler = logging.FileHandler(log_path, encoding='utf-8')
        file_handler.setLevel(logging.DEBUG)
        file_formatter = logging.Formatter(detailed_format)
        file_handler.setFormatter(file_formatter)
        logger.addHandler(file_handler)
    
    return logger


def get_logger(name: str = "pikpak_downloader") -> logging.Logger:
    """获取日志器实例"""
    return logging.getLogger(name)


class DownloadLogger:
    """下载专用日志类"""
    
    def __init__(self, logger: logging.Logger):
        self.logger = logger
    
    def download_start(self, file_name: str, file_size: int = None):
        """记录下载开始"""
        size_info = f" ({self._format_size(file_size)})" if file_size else ""
        self.logger.info(f"🚀 开始下载: {file_name}{size_info}")
    
    def download_complete(self, file_name: str, duration: float = None):
        """记录下载完成"""
        time_info = f" (耗时: {duration:.2f}s)" if duration else ""
        self.logger.info(f"✅ 下载完成: {file_name}{time_info}")
    
    def download_failed(self, file_name: str, error: str):
        """记录下载失败"""
        self.logger.error(f"❌ 下载失败: {file_name} - {error}")
    
    def download_progress(self, file_name: str, percent: float):
        """记录下载进度"""
        self.logger.debug(f"📊 下载进度: {file_name} - {percent:.1f}%")
    
    def retry_attempt(self, file_name: str, attempt: int, max_attempts: int):
        """记录重试尝试"""
        self.logger.warning(f"🔄 重试下载: {file_name} ({attempt}/{max_attempts})")
    
    def batch_summary(self, success_count: int, total_count: int, total_size: int = None):
        """记录批量下载摘要"""
        size_info = f", 总大小: {self._format_size(total_size)}" if total_size else ""
        self.logger.info(f"📈 批量下载完成: {success_count}/{total_count} 个文件成功{size_info}")
    
    @staticmethod
    def _format_size(size_bytes: int) -> str:
        """格式化文件大小"""
        if size_bytes is None:
            return "未知"
        
        for unit in ['B', 'KB', 'MB', 'GB', 'TB']:
            if size_bytes < 1024.0:
                return f"{size_bytes:.1f} {unit}"
            size_bytes /= 1024.0
        return f"{size_bytes:.1f} PB"


# 创建默认日志器实例
default_logger = None
download_logger = None


def init_logging(level: str = "INFO", log_file: Optional[str] = None):
    """初始化日志系统"""
    global default_logger, download_logger
    
    # 生成默认日志文件名
    if log_file is None:
        timestamp = datetime.now().strftime("%Y%m%d")
        log_file = f"logs/pikpak_downloader_{timestamp}.log"
    
    default_logger = setup_logger(level=level, log_file=log_file)
    download_logger = DownloadLogger(default_logger)
    
    default_logger.info("日志系统初始化完成")
    return default_logger, download_logger