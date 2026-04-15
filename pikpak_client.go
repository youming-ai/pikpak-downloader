package main

import (
	"bufio"
	"bytes"
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"
)

// PerformanceMetrics 性能指标收集器
type PerformanceMetrics struct {
	OperationCount  int64         `json:"operation_count"`
	TotalDuration   time.Duration `json:"total_duration"`
	AverageDuration time.Duration `json:"average_duration"`
	MemoryUsage     int64         `json:"memory_usage"`
	ErrorCount      int64         `json:"error_count"`
	LastOperation   string        `json:"last_operation"`
	StartTime       time.Time     `json:"start_time"`
	mutex           sync.RWMutex
}

// NewPerformanceMetrics 创建性能监控器
func NewPerformanceMetrics() *PerformanceMetrics {
	return &PerformanceMetrics{
		StartTime: time.Now(),
	}
}

// Record 记录操作性能
func (pm *PerformanceMetrics) Record(operation string, duration time.Duration, memoryDelta int64, hasError bool) {
	pm.mutex.Lock()
	defer pm.mutex.Unlock()

	atomic.AddInt64(&pm.OperationCount, 1)
	atomic.AddInt64(&pm.ErrorCount, boolToInt64(hasError))
	pm.TotalDuration += duration
	pm.AverageDuration = pm.TotalDuration / time.Duration(pm.OperationCount)
	atomic.AddInt64(&pm.MemoryUsage, memoryDelta)
	pm.LastOperation = operation
}

// GetSnapshot 获取性能快照
func (pm *PerformanceMetrics) GetSnapshot() PerformanceMetrics {
	pm.mutex.RLock()
	defer pm.mutex.RUnlock()

	return PerformanceMetrics{
		OperationCount:  atomic.LoadInt64(&pm.OperationCount),
		TotalDuration:   pm.TotalDuration,
		AverageDuration: pm.AverageDuration,
		MemoryUsage:     atomic.LoadInt64(&pm.MemoryUsage),
		ErrorCount:      atomic.LoadInt64(&pm.ErrorCount),
		LastOperation:   pm.LastOperation,
		StartTime:       pm.StartTime,
	}
}

// PrintStats 打印性能统计
func (pm *PerformanceMetrics) PrintStats() {
	snapshot := pm.GetSnapshot()
	uptime := time.Since(snapshot.StartTime)

	fmt.Printf("\n📊 性能统计:\n")
	fmt.Printf("  总操作数: %d\n", snapshot.OperationCount)
	fmt.Printf("  错误数: %d (%.1f%%)\n", snapshot.ErrorCount,
		float64(snapshot.ErrorCount)/float64(snapshot.OperationCount)*100)
	fmt.Printf("  平均响应时间: %v\n", snapshot.AverageDuration)
	fmt.Printf("  内存使用: %.2f MB\n", float64(snapshot.MemoryUsage)/1024/1024)
	fmt.Printf("  运行时间: %v\n", uptime)
	fmt.Printf("  最后操作: %s\n", snapshot.LastOperation)
}

// boolToInt64 布尔值转int64
func boolToInt64(b bool) int64 {
	if b {
		return 1
	}
	return 0
}

// LimitedWriter 限制写入器，防止输出过大导致内存溢出
type LimitedWriter struct {
	limit int
	buf   *bytes.Buffer
	mu    sync.Mutex
}

func NewLimitedWriter(limit int) *LimitedWriter {
	return &LimitedWriter{
		limit: limit,
		buf:   &bytes.Buffer{},
	}
}

func (lw *LimitedWriter) Write(p []byte) (n int, err error) {
	lw.mu.Lock()
	defer lw.mu.Unlock()

	if lw.buf.Len() >= lw.limit {
		return 0, fmt.Errorf("output exceeds limit of %d bytes", lw.limit)
	}

	remaining := lw.limit - lw.buf.Len()
	if len(p) > remaining {
		p = p[:remaining]
	}

	return lw.buf.Write(p)
}

func (lw *LimitedWriter) String() string {
	lw.mu.Lock()
	defer lw.mu.Unlock()
	return lw.buf.String()
}

// PikPakClient PikPak client
type PikPakClient struct {
	cliPath    string
	configPath string
	debugMode  bool
	metrics    *PerformanceMetrics
}

// FileType 文件类型枚举
type FileType string

const (
	TypeImage    FileType = "Image"
	TypeVideo    FileType = "Video"
	TypeDocument FileType = "Document"
	TypeArchive  FileType = "Archive"
	TypeFolder   FileType = "Folder"
	TypeOther    FileType = "Other"
)

// FileInfo file information structure with optimized memory usage
type FileInfo struct {
	ID          string     `json:"id,omitempty"`
	Name        string     `json:"name"`
	Size        int64      `json:"size"`
	Type        FileType   `json:"type"`
	Kind        string     `json:"kind,omitempty"`
	CreatedAt   *time.Time `json:"created_at,omitempty"` // 使用指针节省内存
	UpdatedAt   *time.Time `json:"updated_at,omitempty"` // 使用指针节省内存
	ParentID    string     `json:"parent_id,omitempty"`
	Path        string     `json:"path,omitempty"`
	Extension   string     `json:"extension,omitempty"`
	MimeType    string     `json:"mime_type,omitempty"`
	Thumbnail   string     `json:"thumbnail,omitempty"`
	URL         string     `json:"url,omitempty"`
	DownloadURL string     `json:"download_url,omitempty"`
}

// QuotaInfo quota information
type QuotaInfo struct {
	Total int64 `json:"total"`
	Used  int64 `json:"used"`
}

// NewPikPakClient create PikPak client
func NewPikPakClient() *PikPakClient {
	home, _ := os.UserHomeDir()
	return &PikPakClient{
		cliPath:    filepath.Join(home, "go", "bin", "pikpakcli"),
		configPath: "config.yml",
		debugMode:  false,
		metrics:    NewPerformanceMetrics(),
	}
}

// executeCommand 执行命令，包含可选超时控制和输出限制。
// 当 timeout <= 0 时不施加额外的截止时间（只受传入 ctx 约束）。
func (p *PikPakClient) executeCommand(ctx context.Context, args []string, outputLimit int, timeout time.Duration) (string, error) {
	if timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, timeout)
		defer cancel()
	}

	cmd := exec.CommandContext(ctx, p.cliPath, args...)

	// 创建限制写入器来控制输出大小
	outputWriter := NewLimitedWriter(outputLimit)
	errorWriter := NewLimitedWriter(outputLimit)

	cmd.Stdout = outputWriter
	cmd.Stderr = errorWriter

	err := cmd.Run()

	// 检查是否超时
	if ctx.Err() == context.DeadlineExceeded {
		return "", fmt.Errorf("command timed out after %s", timeout)
	}

	if err != nil {
		// 如果有错误输出，包含在错误信息中
		errorOutput := errorWriter.String()
		if errorOutput != "" {
			return "", fmt.Errorf("command failed: %v, error output: %s", err, errorOutput)
		}
		return "", fmt.Errorf("command failed: %v", err)
	}

	return outputWriter.String(), nil
}

// WithMetrics 包装操作并记录性能指标
func (p *PikPakClient) WithMetrics(operation string, fn func() error) error {
	var m1, m2 runtime.MemStats
	runtime.ReadMemStats(&m1)

	start := time.Now()
	err := fn()
	duration := time.Since(start)

	runtime.ReadMemStats(&m2)
	memoryDelta := int64(m2.Alloc - m1.Alloc)

	p.metrics.Record(operation, duration, memoryDelta, err != nil)

	return err
}

// ListFilesStream 流式处理文件列表，避免一次性加载所有文件到内存
func (p *PikPakClient) ListFilesStream(ctx context.Context, path string, longFormat bool, humanReadable bool, callback func(FileInfo) error) error {
	// Build command arguments
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

	// Create command with context
	cmd := exec.CommandContext(ctx, p.cliPath, args...)

	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return fmt.Errorf("failed to create stdout pipe: %w", err)
	}
	defer stdout.Close()

	// Redirect stderr to buffer for error checking
	var stderrBuf bytes.Buffer
	cmd.Stderr = &stderrBuf

	// Start command
	if err := cmd.Start(); err != nil {
		return fmt.Errorf("failed to start command: %w", err)
	}

	// Process output line by line
	scanner := bufio.NewScanner(stdout)
	for scanner.Scan() {
		line := strings.TrimSpace(scanner.Text())
		if line == "" || strings.HasPrefix(line, "total") {
			continue
		}

		var file FileInfo
		if longFormat {
			file = p.parseLongFormatLine(line)
		} else {
			file = FileInfo{
				Name: line,
				Type: p.detectFileType(line),
			}
		}

		if file.Name != "" {
			if err := callback(file); err != nil {
				return fmt.Errorf("callback failed: %w", err)
			}
		}
	}

	// Wait for command to finish
	if err := cmd.Wait(); err != nil {
		errorOutput := stderrBuf.String()
		if errorOutput != "" {
			return fmt.Errorf("command failed: %v, error output: %s", err, errorOutput)
		}
		return fmt.Errorf("command failed: %v", err)
	}

	if err := scanner.Err(); err != nil {
		return fmt.Errorf("error reading output: %w", err)
	}

	return nil
}

// ListFilesPaginated 分页获取文件列表，用于处理大量文件
func (p *PikPakClient) ListFilesPaginated(ctx context.Context, path string, longFormat bool, humanReadable bool, pageSize int, pageCallback func([]FileInfo, int) error) error {
	var page []FileInfo
	pageCount := 0

	callback := func(file FileInfo) error {
		page = append(page, file)

		if len(page) >= pageSize {
			pageCount++
			if err := pageCallback(page, pageCount); err != nil {
				return err
			}
			page = page[:0] // Clear slice but keep capacity
		}

		return nil
	}

	// Use stream processing
	if err := p.ListFilesStream(ctx, path, longFormat, humanReadable, callback); err != nil {
		return err
	}

	// Process last page if it has items
	if len(page) > 0 {
		pageCount++
		return pageCallback(page, pageCount)
	}

	return nil
}

// SetDebug set debug mode
func (p *PikPakClient) SetDebug(debug bool) {
	p.debugMode = debug
}

// CheckConfig check configuration
func (p *PikPakClient) CheckConfig() error {
	ctx := context.Background()

	// Load configuration
	config, err := LoadConfig()
	if err != nil {
		return fmt.Errorf("Failed to load configuration: %v", err)
	}

	// Check if configured
	if !config.IsConfigured() {
		return fmt.Errorf("Please configure PikPak authentication in .env file")
	}

	// Generate configuration file
	if err := config.GeneratePikPakCLIConfig(); err != nil {
		return fmt.Errorf("Failed to generate configuration file: %v", err)
	}

	// Validate configuration with timeout control
	_, err = p.executeCommand(ctx, []string{"quota"}, 1024*1024, 30*time.Second) // 1MB limit for quota command
	if err != nil {
		return fmt.Errorf("Configuration validation failed: %v", err)
	}

	return nil
}

// ListFiles list files
func (p *PikPakClient) ListFiles(path string, longFormat bool, humanReadable bool) ([]FileInfo, error) {
	ctx := context.Background()
	var files []FileInfo

	// Pre-allocate slice with reasonable capacity to avoid multiple allocations
	files = make([]FileInfo, 0, 100) // Start with capacity for 100 files

	// Build command arguments
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

	// For small to medium file lists, use the optimized method
	// For very large lists, we could switch to streaming in the future
	output, err := p.executeCommand(ctx, args, 10*1024*1024, 60*time.Second)
	if err != nil {
		return nil, fmt.Errorf("Failed to list files: %v", err)
	}

	// Parse output with optimized string processing
	lines := strings.Split(output, "\n")
	for _, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "total") {
			continue
		}

		// If long format, parse detailed information
		if longFormat {
			file := p.parseLongFormatLine(line)
			if file.Name != "" {
				files = append(files, file)
			}
		} else {
			// Simple format, only parse filename
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

// parseLongFormatLine parse long format output with optimized string processing
func (p *PikPakClient) parseLongFormatLine(line string) FileInfo {
	// Use strings.FieldsN to avoid unnecessary allocations
	parts := strings.Fields(line)
	if len(parts) < 6 {
		return FileInfo{}
	}

	// Parse size with optimized logic
	var size int64
	sizeStr := parts[2]

	// Use efficient string operations
	switch {
	case len(sizeStr) > 2 && sizeStr[len(sizeStr)-2:] == "GB":
		if sizeFloat, err := strconv.ParseFloat(sizeStr[:len(sizeStr)-2], 64); err == nil {
			size = int64(sizeFloat * 1024 * 1024 * 1024)
		}
	case len(sizeStr) > 2 && sizeStr[len(sizeStr)-2:] == "MB":
		if sizeFloat, err := strconv.ParseFloat(sizeStr[:len(sizeStr)-2], 64); err == nil {
			size = int64(sizeFloat * 1024 * 1024)
		}
	case len(sizeStr) > 2 && sizeStr[len(sizeStr)-2:] == "KB":
		if sizeFloat, err := strconv.ParseFloat(sizeStr[:len(sizeStr)-2], 64); err == nil {
			size = int64(sizeFloat * 1024)
		}
	default:
		size, _ = strconv.ParseInt(sizeStr, 10, 64)
	}

	// Efficiently join remaining parts
	var fileName strings.Builder
	fileName.Grow(len(line) / 2) // Pre-allocate reasonable capacity
	for i := 5; i < len(parts); i++ {
		if i > 5 {
			fileName.WriteByte(' ')
		}
		fileName.WriteString(parts[i])
	}

	fileNameStr := fileName.String()
	return FileInfo{
		Name: fileNameStr,
		Size: size,
		Type: p.detectFileType(fileNameStr),
	}
}

// detectFileType detect file type with optimized lookup
func (p *PikPakClient) detectFileType(filename string) FileType {
	// Use a map for O(1) lookup instead of O(n) slice iteration
	var ext = strings.ToLower(filepath.Ext(filename))

	if ext == "" {
		return TypeFolder
	}

	// Use maps for efficient lookup
	imageExts := map[string]bool{
		".jpg": true, ".jpeg": true, ".png": true, ".gif": true,
		".bmp": true, ".webp": true, ".svg": true,
	}

	videoExts := map[string]bool{
		".mp4": true, ".avi": true, ".mkv": true, ".mov": true,
		".wmv": true, ".flv": true, ".webm": true, ".m4v": true,
	}

	docExts := map[string]bool{
		".pdf": true, ".doc": true, ".docx": true, ".txt": true,
		".xlsx": true, ".pptx": true, ".odt": true, ".rtf": true,
	}

	archiveExts := map[string]bool{
		".zip": true, ".rar": true, ".7z": true, ".tar": true,
		".gz": true, ".bz2": true, ".xz": true,
	}

	switch {
	case imageExts[ext]:
		return TypeImage
	case videoExts[ext]:
		return TypeVideo
	case docExts[ext]:
		return TypeDocument
	case archiveExts[ext]:
		return TypeArchive
	default:
		return TypeOther
	}
}

// GetQuota get quota information
func (p *PikPakClient) GetQuota() (*QuotaInfo, error) {
	ctx := context.Background()

	args := []string{"quota"}
	if p.debugMode {
		args = append(args, "--debug")
	}

	// Execute with timeout and output limit (1MB for quota)
	output, err := p.executeCommand(ctx, args, 1024*1024, 30*time.Second)
	if err != nil {
		return nil, fmt.Errorf("Failed to get quota: %v", err)
	}

	return p.parseQuotaOutput(output)
}

// parseQuotaOutput parse quota output
func (p *PikPakClient) parseQuotaOutput(output string) (*QuotaInfo, error) {
	lines := strings.Split(output, "\n")

	// Find header row and data row
	var totalStr, usedStr string
	for i, line := range lines {
		line = strings.TrimSpace(line)
		if line == "" {
			continue
		}

		// Find header row containing "total" and "used"
		if strings.Contains(line, "total") && strings.Contains(line, "used") {
			// Next row should be data row
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

	// Parse size
	total, _ := p.parseSize(totalStr)
	used, _ := p.parseSize(usedStr)

	return &QuotaInfo{
		Total: total,
		Used:  used,
	}, nil
}

// parseSize parse size string with optimized parsing
func (p *PikPakClient) parseSize(sizeStr string) (int64, error) {
	sizeStr = strings.TrimSpace(sizeStr)
	if sizeStr == "" {
		return 0, nil
	}

	length := len(sizeStr)

	// Use efficient string operations based on length
	switch {
	case length > 2 && sizeStr[length-2:] == "GB":
		if num, err := strconv.ParseFloat(sizeStr[:length-2], 64); err == nil {
			return int64(num * 1024 * 1024 * 1024), nil
		}
	case length > 2 && sizeStr[length-2:] == "MB":
		if num, err := strconv.ParseFloat(sizeStr[:length-2], 64); err == nil {
			return int64(num * 1024 * 1024), nil
		}
	case length > 2 && sizeStr[length-2:] == "KB":
		if num, err := strconv.ParseFloat(sizeStr[:length-2], 64); err == nil {
			return int64(num * 1024), nil
		}
	case strings.Contains(sizeStr, "e+") || strings.Contains(sizeStr, "E+"):
		// Handle scientific notation
		if num, err := strconv.ParseFloat(sizeStr, 64); err == nil {
			return int64(num), nil
		}
	default:
		// Try to parse as integer
		if num, err := strconv.ParseInt(sizeStr, 10, 64); err == nil {
			return num, nil
		}
	}

	return 0, fmt.Errorf("Unable to parse size: %s", sizeStr)
}

// DownloadFile download file or folder via upstream pikpakcli.
// 并发数直接透传给 pikpakcli 的 --count，让上游自行调度。
func (p *PikPakClient) DownloadFile(path string, outputDir string, concurrency int, showProgress bool) error {
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		return fmt.Errorf("Failed to create output directory: %v", err)
	}

	if concurrency < 1 {
		concurrency = 1
	}

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

// GetPerformanceStats 获取性能统计信息
func (p *PikPakClient) GetPerformanceStats() PerformanceMetrics {
	if p.metrics == nil {
		return PerformanceMetrics{}
	}
	return p.metrics.GetSnapshot()
}

// PrintPerformanceStats 打印性能统计信息
func (p *PikPakClient) PrintPerformanceStats() {
	if p.metrics != nil {
		p.metrics.PrintStats()
	}
}

// PrintFiles print file list with optimized formatting.
// 注：长格式不展示修改时间，因为 pikpakcli 文本输出未可靠解析到 mtime；
// 如需修改时间请走 pikpakcli 的原生 JSON/API，再把 UpdatedAt 填进 FileInfo。
func (p *PikPakClient) PrintFiles(files []FileInfo, longFormat bool, humanReadable bool) {
	if len(files) == 0 {
		fmt.Println("Directory is empty")
		return
	}

	if longFormat {
		fmt.Printf("%-10s %-12s %s\n", "Type", "Size", "Name")
		fmt.Println(strings.Repeat("-", 50))

		for _, file := range files {
			sizeStr := p.formatSize(file.Size, humanReadable)
			fmt.Printf("%-10s %-12s %s\n", string(file.Type), sizeStr, file.Name)
		}
	} else {
		for _, file := range files {
			fmt.Printf("%-10s %s\n", string(file.Type), file.Name)
		}
	}
}

// formatSize format size display
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
