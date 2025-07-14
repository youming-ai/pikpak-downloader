#!/usr/bin/env python3
"""
自定义异常类模块
"""


class PikPakError(Exception):
    """PikPak 下载器基础异常类"""
    pass


class ShareLinkError(PikPakError):
    """分享链接相关异常"""
    def __init__(self, message: str, share_url: str = None):
        super().__init__(message)
        self.share_url = share_url


class InvalidShareLinkError(ShareLinkError):
    """无效的分享链接异常"""
    pass


class ShareNotFoundError(ShareLinkError):
    """分享链接不存在异常"""
    pass


class ShareAccessDeniedError(ShareLinkError):
    """分享访问被拒绝异常"""
    pass


class DownloadError(PikPakError):
    """下载相关异常"""
    def __init__(self, message: str, file_name: str = None, file_id: str = None):
        super().__init__(message)
        self.file_name = file_name
        self.file_id = file_id


class FileNotFoundError(DownloadError):
    """文件不存在异常"""
    pass


class InsufficientStorageError(DownloadError):
    """存储空间不足异常"""
    pass


class NetworkError(PikPakError):
    """网络相关异常"""
    def __init__(self, message: str, status_code: int = None, response_data: str = None):
        super().__init__(message)
        self.status_code = status_code
        self.response_data = response_data


class ConnectionTimeoutError(NetworkError):
    """连接超时异常"""
    pass


class TooManyRetriesError(NetworkError):
    """重试次数过多异常"""
    def __init__(self, message: str, retry_count: int = None):
        super().__init__(message)
        self.retry_count = retry_count


class RateLimitError(NetworkError):
    """请求频率限制异常"""
    pass


class AuthenticationError(PikPakError):
    """认证相关异常"""
    pass


class ConfigurationError(PikPakError):
    """配置相关异常"""
    pass


class ValidationError(PikPakError):
    """数据验证异常"""
    def __init__(self, message: str, field_name: str = None, field_value: str = None):
        super().__init__(message)
        self.field_name = field_name
        self.field_value = field_value