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

// PikPakClient PikPak client
type PikPakClient struct {
	cliPath    string
	configPath string
	debugMode  bool
}

// FileInfo file information structure
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

// QuotaInfo quota information
type QuotaInfo struct {
	Total int64 `json:"total"`
	Used  int64 `json:"used"`
}

// NewPikPakClient create PikPak client
func NewPikPakClient() *PikPakClient {
	return &PikPakClient{
		cliPath:    filepath.Join(os.Getenv("HOME"), "go", "bin", "pikpakcli"),
		configPath: "config.yml",
		debugMode:  false,
	}
}

// SetDebug set debug mode
func (p *PikPakClient) SetDebug(debug bool) {
	p.debugMode = debug
}

// CheckConfig check configuration
func (p *PikPakClient) CheckConfig() error {
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

	// Validate configuration
	cmd := exec.Command(p.cliPath, "quota")
	if err := cmd.Run(); err != nil {
		return fmt.Errorf("Configuration validation failed: %v", err)
	}

	return nil
}

// ListFiles list files
func (p *PikPakClient) ListFiles(path string, longFormat bool, humanReadable bool) ([]FileInfo, error) {
	var files []FileInfo

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

	cmd := exec.Command(p.cliPath, args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("Failed to list files: %v, output: %s", err, string(output))
	}

	// Parse output
	lines := strings.Split(string(output), "\n")
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

// parseLongFormatLine parse long format output
func (p *PikPakClient) parseLongFormatLine(line string) FileInfo {
	parts := strings.Fields(line)
	if len(parts) < 6 {
		return FileInfo{}
	}

	// Parse size
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

	// Parse time (simplified handling)
	fileName := strings.Join(parts[5:], " ")

	return FileInfo{
		Name: fileName,
		Size: size,
		Type: p.detectFileType(fileName),
	}
}

// detectFileType detect file type
func (p *PikPakClient) detectFileType(filename string) string {
	ext := strings.ToLower(filepath.Ext(filename))

	imageExts := []string{".jpg", ".jpeg", ".png", ".gif", ".bmp", ".webp"}
	videoExts := []string{".mp4", ".avi", ".mkv", ".mov", ".wmv", ".flv", ".webm"}
	docExts := []string{".pdf", ".doc", ".docx", ".txt", ".xlsx", ".pptx"}
	archiveExts := []string{".zip", ".rar", ".7z", ".tar", ".gz"}

	for _, imgExt := range imageExts {
		if ext == imgExt {
			return "Image"
		}
	}

	for _, vidExt := range videoExts {
		if ext == vidExt {
			return "Video"
		}
	}

	for _, docExt := range docExts {
		if ext == docExt {
			return "Document"
		}
	}

	for _, archExt := range archiveExts {
		if ext == archExt {
			return "Archive"
		}
	}

	// If no extension, might be a folder
	if ext == "" {
		return "Folder"
	}

	return "Other"
}

// GetQuota get quota information
func (p *PikPakClient) GetQuota() (*QuotaInfo, error) {
	args := []string{"quota"}
	if p.debugMode {
		args = append(args, "--debug")
	}

	cmd := exec.Command(p.cliPath, args...)
	output, err := cmd.CombinedOutput()
	if err != nil {
		return nil, fmt.Errorf("Failed to get quota: %v, output: %s", err, string(output))
	}

	return p.parseQuotaOutput(string(output))
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

// parseSize parse size string
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
		// Handle scientific notation
		if num, err := strconv.ParseFloat(sizeStr, 64); err == nil {
			return int64(num), nil
		}
	} else {
		return strconv.ParseInt(sizeStr, 10, 64)
	}

	return 0, fmt.Errorf("Unable to parse size: %s", sizeStr)
}

// DownloadFile download file or folder
func (p *PikPakClient) DownloadFile(path string, outputDir string, concurrency int, showProgress bool) error {
	// Create output directory
	if err := os.MkdirAll(outputDir, 0755); err != nil {
		return fmt.Errorf("Failed to create output directory: %v", err)
	}

	// Build command arguments
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

// PrintFiles print file list
func (p *PikPakClient) PrintFiles(files []FileInfo, longFormat bool, humanReadable bool) {
	if len(files) == 0 {
		fmt.Println("Directory is empty")
		return
	}

	if longFormat {
		fmt.Printf("%-10s %-12s %-20s %s\n", "Type", "Size", "Modified", "Name")
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
