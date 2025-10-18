package main

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

// PikPakClient PikPak客户端
type PikPakClient struct {
	cliPath    string
	configPath string
	debugMode  bool
}

// FileInfo 文件信息结构
type FileInfo struct {
	ID          string    `json:"id"`
	Name        string    `json:"name"`
	Size        int64     `json:"size"`
	Type        string    `json:"type"`
	Kind        string    `json:"kind"`
	CreatedAt   time.Time `json:"created_at"`
	UpdatedAt   time.Time `json:"updated_at"`
	ParentID    string    `json:"parent_id"`
	Path        string    `json:"path"`
	Extension   string    `json:"extension"`
	MimeType    string    `json:"mime_type"`
	Thumbnail   string    `json:"thumbnail"`
	URL         string    `json:"url"`
	DownloadURL string    `json:"download_url"`
}

// QuotaInfo 配额信息
type QuotaInfo struct {
	Total int64 `json:"total"`
	Used  int64 `json:"used"`
}

// NewPikPakClient 创建PikPak客户端
func NewPikPakClient() *PikPakClient {
	return &PikPakClient{
		cliPath:    filepath.Join(os.Getenv("HOME"), "go", "bin", "pikpakcli"),
		configPath: "config.yml",
		debugMode:  false,
	}
}

// SetDebug 设置调试模式
func (p *PikPakClient) SetDebug(debug bool) {
	p.debugMode = debug
}

// CheckConfig 检查配置
func (p *PikPakClient) CheckConfig() error {
	// 加载配置
	config, err := LoadConfig()
	if err != nil {
		return fmt.Errorf("加载配置失败: %v", err)
	}

	// 检查是否已配置
	if !config.IsConfigured() {
		return fmt.Errorf("请先在 .env 文件中配置 PikPak 认证信息")
	}

	// 生成配置文件
	if err := config.GeneratePikPakCLIConfig(); err != nil {
		return fmt.Errorf("生成配置文件失败: %v", err)
	}

	// 验证配置
	cmd := exec.Command(p.cliPath, "quota")
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("配置验证失败: %v", err)
	}

	return nil
}

// ListFiles 列出文件
func (p *PikPakClient) ListFiles(path string, longFormat bool, humanReadable bool) ([]FileInfo, error) {
	var files []FileInfo

	// 构建命令参数
	args := []string{"ls", "--path", path}
	if longFormat {
		args = append(args, "--long")
	}
	if humanReadable {
		args = append(args, "--human")
	}
	if p.debugMode {
		args = append(args, "--debug")
	}

	cmd := exec.Command(p.cliPath, args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("列出文件失败: %v, 输出: %s", err, string(output))
	}

	// 解析输出
	lines := strings.Split(string(output), "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "total") {
			continue
		}

		// 如果是长格式，解析详细信息
		if longFormat {
			file := p.parseLongFormatLine(line)
			if file.Name != "" {
				files = append(files, file)
			}
		} else {
			// 简单格式，只解析文件名
			if line != "" {
				files = append(files, FileInfo{
					Name: line,
					Type: p.detectFileType(line),
				})
			}
		}
	}

	return files, nil
}

// parseLongFormatLine 解析长格式输出
func (p *PikPakClient) parseLongFormatLine(line string) FileInfo {
	parts := strings.Fields(line)
	if len(parts) < 6 {
		return FileInfo{}
	}

	// 解析大小
	var size int64
	var err error
	if strings.Contains(parts[2], "GB") {
		sizeStr := strings.TrimSuffix(parts[2], "GB")
		var sizeFloat float64
		if sizeFloat, err = strconv.ParseFloat(sizeStr, 64); err == nil {
			size = int64(sizeFloat * 1024 * 1024 * 1024)
		}
	} else if strings.Contains(parts[2], "MB") {
		sizeStr := strings.TrimSuffix(parts[2], "MB")
		var sizeFloat float64
		if sizeFloat, err = strconv.ParseFloat(sizeStr, 64); err == nil {
			size = int64(sizeFloat * 1024 * 1024)
		}
	} else if strings.Contains(parts[2], "KB") {
		sizeStr := strings.TrimSuffix(parts[2], "KB")
		var sizeFloat float64
		if sizeFloat, err = strconv.ParseFloat(sizeStr, 64); err == nil {
			size = int64(sizeFloat * 1024)
		}
	} else {
		size, _ = strconv.ParseInt(parts[2], 10, 64)
	}

	// 解析时间 (简化处理)
	fileName := strings.Join(parts[5:], " ")

	return FileInfo{
		Name: fileName,
		Size: size,
		Type: p.detectFileType(fileName),
	}
}

// detectFileType 检测文件类型
func (p *PikPakClient) detectFileType(filename string) string {
	ext := strings.ToLower(filepath.Ext(filename))

	imageExts := []string{".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"}
	videoExts := []string{".mp4", ".avi", ".mkv", ".mov", ".wmv", ".flv", ".webm"}
	docExts := []string{".pdf", ".doc", ".docx", ".txt", ".xlsx", ".pptx"}
	archiveExts := []string{".zip", ".rar", ".7z", ".tar", ".gz"}

	for _, imgExt := range imageExts {
		if ext == imgExt {
			return "图片"
		}
	}

	for _, vidExt := range videoExts {
		if ext == vidExt {
			return "视频"
		}
	}

	for _, docExt := range docExts {
		if ext == docExt {
			return "文档"
		}
	}

	for _, archExt := range archiveExts {
		if ext == archExt {
			return "压缩包"
		}
	}

	// 如果没有扩展名，可能是文件夹
	if ext == "" {
		return "文件夹"
	}

	return "其他"
}

// GetQuota 获取配额信息
func (p *PikPakClient) GetQuota() (*QuotaInfo, error) {
	args := []string{"quota"}
	if p.debugMode {
		args = append(args, "--debug")
	}

	cmd := exec.Command(p.cliPath, args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("获取配额失败: %v, 输出: %s", err, string(output))
	}

	return p.parseQuotaOutput(string(output))
}

// parseQuotaOutput 解析配额输出
func (p *PikPakClient) parseQuotaOutput(output string) (*QuotaInfo, error) {
	lines := strings.Split(output, "\n")

	// 查找表头行和数据行
	var totalStr, usedStr string
	for i, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		// 找到包含 "total" 和 "used" 的表头行
		if strings.Contains(line, "total") && strings.Contains(line, "used") {
			// 下一行应该是数据行
			if i+1 < len(lines) {
				dataLine := strings.TrimSpace(lines[i+1])
				parts := strings.Fields(dataLine)
				if len(parts) >= 2 {
					totalStr = parts[0]
					usedStr = parts[1]
				}
			}
			break
		}
	}

	// 解析大小
	total, _ := p.parseSize(totalStr)
	used, _ := p.parseSize(usedStr)

	return &QuotaInfo{
		Total: total,
		Used:  used,
	}, nil
}

// parseSize 解析大小字符串
func (p *PikPakClient) parseSize(sizeStr string) (int64, error) {
	sizeStr = strings.TrimSpace(sizeStr)
	if sizeStr == "" {
		return 0, nil
	}

	if strings.Contains(sizeStr, "GB") {
		numStr := strings.TrimSuffix(sizeStr, "GB")
		if num, err := strconv.ParseFloat(numStr, 64); err == nil {
			return int64(num * 1024 * 1024 * 1024), nil
		}
	} else if strings.Contains(sizeStr, "MB") {
		numStr := strings.TrimSuffix(sizeStr, "MB")
		if num, err := strconv.ParseFloat(numStr, 64); err == nil {
			return int64(num * 1024 * 1024), nil
		}
	} else if strings.Contains(sizeStr, "KB") {
		numStr := strings.TrimSuffix(sizeStr, "KB")
		if num, err := strconv.ParseFloat(numStr, 64); err == nil {
			return int64(num * 1024), nil
		}
	} else if strings.Contains(sizeStr, "e+") {
		// 处理科学计数法
		if num, err := strconv.ParseFloat(sizeStr, 64); err == nil {
			return int64(num), nil
		}
	} else {
		return strconv.ParseInt(sizeStr, 10, 64)
	}

	return 0, fmt.Errorf("无法解析大小: %s", sizeStr)
}

// DownloadFile 下载文件或文件夹
func (p *PikPakClient) DownloadFile(path string, outputDir string, concurrency int, showProgress bool) error {
	// 创建输出目录
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		return fmt.Errorf("创建输出目录失败: %v", err)
	}

	// 构建命令参数
	args := []string{"download", "--path", path, "--output", outputDir, "--count", strconv.Itoa(concurrency)}
	if showProgress {
		args = append(args, "--progress")
	}
	if p.debugMode {
		args = append(args, "--debug")
	}

	cmd := exec.Command(p.cliPath, args...)
	cmd.Stdout = os.Stdout
	cmd.Stderr = os.Stderr

	return cmd.Run()
}

// PrintFiles 打印文件列表
func (p *PikPakClient) PrintFiles(files []FileInfo, longFormat bool, humanReadable bool) {
	if len(files) == 0 {
		fmt.Println("目录为空")
		return
	}

	if longFormat {
		fmt.Printf("%-10s %-12s %-20s %s\n", "类型", "大小", "修改时间", "文件名")
		fmt.Println(strings.Repeat("-", 70))

		for _, file := range files {
			sizeStr := p.formatSize(file.Size, humanReadable)
			modTime := time.Now().Format("2006-01-02 15:04")
			fmt.Printf("%-10s %-12s %-20s %s\n", file.Type, sizeStr, modTime, file.Name)
		}
	} else {
		for _, file := range files {
			fmt.Printf("%-10s %s\n", file.Type, file.Name)
		}
	}
}

// formatSize 格式化大小显示
func (p *PikPakClient) formatSize(size int64, humanReadable bool) string {
	if !humanReadable {
		return strconv.FormatInt(size, 10)
	}

	if size >= 1024*1024*1024 {
		return fmt.Sprintf("%.1fGB", float64(size)/1024/1024/1024)
	} else if size >= 1024*1024 {
		return fmt.Sprintf("%.1fMB", float64(size)/1024/1024)
	} else if size >= 1024 {
		return fmt.Sprintf("%.1fKB", float64(size)/1024)
	} else {
		return fmt.Sprintf("%dB", size)
	}
}
