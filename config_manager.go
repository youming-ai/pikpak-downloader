package main

import (
	"bufio"
	"fmt"
	"os"
	"path/filepath"
	"strings"
)

// Config 配置结构
type Config struct {
	Username     string
	Password     string
	RefreshToken string
	Proxy        string
	DeviceID     string
	DeviceName   string
}

// LoadConfig 从环境变量和.env文件加载配置
func LoadConfig() (*Config, error) {
	config := &Config{
		DeviceName: "pikpak-downloader",
	}

	// 尝试加载.env文件
	if err := loadEnvFile(); err != nil {
		fmt.Printf("⚠️  无法加载.env文件: %v\n", err)
	}

	// 从环境变量读取配置
	config.Username = os.Getenv("PIKPAK_USERNAME")
	config.Password = os.Getenv("PIKPAK_PASSWORD")
	config.RefreshToken = os.Getenv("PIKPAK_REFRESH_TOKEN")
	config.Proxy = os.Getenv("PIKPAK_PROXY")
	config.DeviceID = os.Getenv("PIKPAK_DEVICE_ID")

	if deviceName := os.Getenv("PIKPAK_DEVICE_NAME"); deviceName != "" {
		config.DeviceName = deviceName
	}

	return config, nil
}

// loadEnvFile 加载.env文件
func loadEnvFile() error {
	envFile := ".env"
	if _, err := os.Stat(envFile); os.IsNotExist(err) {
		return fmt.Errorf(".env文件不存在")
	}

	file, err := os.Open(envFile)
	if err != nil {
		return err
	}
	defer file.Close()

	scanner := bufio.NewScanner(file)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())

		// 跳过空行和注释
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}

		// 解析KEY=VALUE格式
		parts := strings.SplitN(line, "=", 2)
		if len(parts) != 2 {
			continue
		}

		key := strings.TrimSpace(parts[0])
		value := strings.TrimSpace(parts[1])

		// 移除引号
		if strings.HasPrefix(value, `"`) && strings.HasSuffix(value, `"`) {
			value = strings.Trim(value, `"`)
		} else if strings.HasPrefix(value, `'`) && strings.HasSuffix(value, `'`) {
			value = strings.Trim(value, `'`)
		}

		// 设置环境变量
		os.Setenv(key, value)
	}

	return scanner.Err()
}

// GeneratePikPakCLIConfig 生成pikpakcli配置文件
func (c *Config) GeneratePikPakCLIConfig() error {
	configDir := getPikPakCLIConfigDir()
	if err := os.MkdirAll(configDir, 0755); err != nil {
		return fmt.Errorf("创建配置目录失败: %v", err)
	}

	configFile := filepath.Join(configDir, "config.yml")

	// 生成配置内容
	configContent := fmt.Sprintf(`# PikPak CLI 配置文件 (由 pikpak-downloader 自动生成)
# 请勿手动编辑此文件，请修改 .env 文件

# 账号认证
username: %s
password: %s
refresh_token: %s

# OAuth 配置
client_id: "YNxT9w7GMdWvEOKa"
client_secret: "dbw2OtmVEeuUvIPEbTySgLTW0y6RkTs6"

# 设备信息
device_id: %s
device_name: %s

# 代理设置
proxy: %s

# 下载设置
download_path: "./downloads"
max_concurrent: 3

# 日志设置
log_level: "info"
`,
		quoteString(c.Username),
		quoteString(c.Password),
		quoteString(c.RefreshToken),
		quoteString(c.DeviceID),
		quoteString(c.DeviceName),
		quoteString(c.Proxy),
	)

	return os.WriteFile(configFile, []byte(configContent), 0644)
}

// quoteString 为字符串添加引号
func quoteString(s string) string {
	if s == "" {
		return `""`
	}
	return fmt.Sprintf(`"%s"`, s)
}

// getPikPakCLIConfigDir 获取pikpakcli配置目录
func getPikPakCLIConfigDir() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "Library", "Application Support", "pikpakcli")
}

// ValidateConfig 验证配置是否有效
func (c *Config) ValidateConfig() error {
	if c.RefreshToken == "" && (c.Username == "" || c.Password == "") {
		return fmt.Errorf("请配置 refresh_token 或 username/password")
	}
	return nil
}

// IsConfigured 检查是否已配置认证信息
func (c *Config) IsConfigured() bool {
	return c.RefreshToken != "" || (c.Username != "" && c.Password != "")
}

// PrintConfigStatus 打印配置状态
func (c *Config) PrintConfigStatus() {
	fmt.Println("📋 配置状态检查:")

	if c.RefreshToken != "" {
		fmt.Println("  ✅ RefreshToken: 已配置")
	} else {
		fmt.Println("  ❌ RefreshToken: 未配置")
	}

	if c.Username != "" {
		fmt.Println("  ✅ 用户名: 已配置")
	} else {
		fmt.Println("  ❌ 用户名: 未配置")
	}

	if c.Password != "" {
		fmt.Println("  ✅ 密码: 已配置")
	} else {
		fmt.Println("  ❌ 密码: 未配置")
	}

	if c.Proxy != "" {
		fmt.Printf("  ✅ 代理: %s\n", c.Proxy)
	} else {
		fmt.Println("  ⚪ 代理: 未配置")
	}

	if c.DeviceName != "" {
		fmt.Printf("  📱 设备名: %s\n", c.DeviceName)
	}
}
