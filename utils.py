#!/usr/bin/env python3
"""
工具函数模块
"""
import re
from pathlib import Path
from typing import Tuple
from urllib.parse import urlparse
from exceptions import InvalidShareLinkError


def validate_share_url(share_url: str) -> Tuple[str, str]:
    """验证并解析分享链接"""
    if not share_url or not isinstance(share_url, str):
        raise InvalidShareLinkError("分享链接不能为空", share_url)
    
    try:
        parsed_url = urlparse(share_url.strip())
        
        if not parsed_url.netloc.endswith('mypikpak.com'):
            raise InvalidShareLinkError("无效的 PikPak 域名", share_url)
        
        path_parts = parsed_url.path.strip('/').split('/')
        
        if len(path_parts) < 3 or path_parts[0] != 's':
            raise InvalidShareLinkError("分享链接格式不正确", share_url)
        
        share_id = path_parts[1]
        file_id = path_parts[2]
        
        if not share_id or not file_id:
            raise InvalidShareLinkError("分享ID或文件ID为空", share_url)
        
        return share_id, file_id
        
    except Exception as e:
        if isinstance(e, InvalidShareLinkError):
            raise
        raise InvalidShareLinkError(f"解析分享链接失败: {str(e)}", share_url)


def sanitize_filename(filename: str, max_length: int = 255) -> str:
    """清理文件名，移除危险字符"""
    if not filename:
        return "unknown_file"
    
    dangerous_chars = r'[<>:"/\\|?*\x00-\x1f]'
    filename = re.sub(dangerous_chars, '_', filename)
    filename = filename.strip(' .')
    
    if len(filename) > max_length:
        name, ext = filename.rsplit('.', 1) if '.' in filename else (filename, '')
        max_name_len = max_length - len(ext) - 1 if ext else max_length
        filename = name[:max_name_len] + ('.' + ext if ext else '')
    
    return filename or "unknown_file"


def format_size(size_bytes: int) -> str:
    """格式化文件大小"""
    if size_bytes is None or size_bytes < 0:
        return "未知"
    
    for unit in ['B', 'KB', 'MB', 'GB', 'TB']:
        if size_bytes < 1024.0:
            return f"{size_bytes:.1f} {unit}"
        size_bytes /= 1024.0
    return f"{size_bytes:.1f} PB"