package main

import (
	"flag"
	"fmt"
	"log"
	"os"
)

// Command 命令结构
type Command struct {
	Name        string
	Description string
	Handler     func(args []string) error
}

// CLI 命令行界面
type CLI struct {
	client   *PikPakClient
	commands map[string]Command
}

// NewCLI 创建CLI实例
func NewCLI() *CLI {
	client := NewPikPakClient()

	cli := &CLI{
		client:   client,
		commands: make(map[string]Command),
	}

	// 注册命令
	cli.registerCommands()
	return cli
}

// registerCommands 注册所有命令
func (c *CLI) registerCommands() {
	c.commands["ls"] = Command{
		Name:        "ls",
		Description: "列出文件和目录",
		Handler:     c.handleList,
	}

	c.commands["download"] = Command{
		Name:        "download",
		Description: "下载文件或文件夹",
		Handler:     c.handleDownload,
	}

	c.commands["quota"] = Command{
		Name:        "quota",
		Description: "查看云盘配额",
		Handler:     c.handleQuota,
	}

	c.commands["help"] = Command{
		Name:        "help",
		Description: "显示帮助信息",
		Handler:     c.handleHelp,
	}
}

// Run 运行CLI
func (c *CLI) Run(args []string) error {
	if len(args) < 1 {
		return c.handleHelp([]string{})
	}

	commandName := args[0]
	if command, exists := c.commands[commandName]; exists {
		return command.Handler(args[1:])
	}

	return fmt.Errorf("未知命令: %s", commandName)
}

// handleList 处理列表命令
func (c *CLI) handleList(args []string) error {
	flags := flag.NewFlagSet("ls", flag.ExitOnError)
	path := flags.String("path", "/", "目录路径")
	longFormat := flags.Bool("l", false, "长格式显示")
	humanReadable := flags.Bool("h", false, "人类可读格式")

	if err := flags.Parse(args); err != nil {
		return err
	}

	// 检查配置
	if err := c.client.CheckConfig(); err != nil {
		return fmt.Errorf("配置检查失败: %v", err)
	}

	// 列出文件
	files, err := c.client.ListFiles(*path, *longFormat, *humanReadable)
	if err != nil {
		return err
	}

	// 显示文件
	c.client.PrintFiles(files, *longFormat, *humanReadable)
	return nil
}

// handleDownload 处理下载命令
func (c *CLI) handleDownload(args []string) error {
	flags := flag.NewFlagSet("download", flag.ExitOnError)
	path := flags.String("path", "/", "下载路径")
	outputDir := flags.String("output", "./downloads", "输出目录")
	concurrency := flags.Int("count", 3, "并发数")
	progress := flags.Bool("progress", true, "显示进度")

	if err := flags.Parse(args); err != nil {
		return err
	}

	// 检查配置
	if err := c.client.CheckConfig(); err != nil {
		return fmt.Errorf("配置检查失败: %v", err)
	}

	fmt.Printf("📥 开始下载: %s\n", *path)
	fmt.Printf("📁 输出目录: %s\n", *outputDir)
	fmt.Printf("⚡ 并发数: %d\n", *concurrency)

	// 开始下载
	return c.client.DownloadFile(*path, *outputDir, *concurrency, *progress)
}

// handleQuota 处理配额命令
func (c *CLI) handleQuota(args []string) error {
	flags := flag.NewFlagSet("quota", flag.ExitOnError)
	humanReadable := flags.Bool("h", true, "人类可读格式")

	if err := flags.Parse(args); err != nil {
		return err
	}

	// 检查配置
	if err := c.client.CheckConfig(); err != nil {
		return fmt.Errorf("配置检查失败: %v", err)
	}

	// 获取配额信息
	quota, err := c.client.GetQuota()
	if err != nil {
		return err
	}

	// 显示配额信息
	fmt.Println("📊 云盘配额信息:")
	fmt.Printf("总容量: %s\n", c.client.formatSize(quota.Total, *humanReadable))
	fmt.Printf("已使用: %s\n", c.client.formatSize(quota.Used, *humanReadable))

	if quota.Total > 0 {
		percentage := float64(quota.Used) / float64(quota.Total) * 100
		fmt.Printf("使用率: %.1f%%\n", percentage)
	}

	return nil
}

// handleHelp 处理帮助命令
func (c *CLI) handleHelp(args []string) error {
	fmt.Println("PikPak 个人云盘管理工具")
	fmt.Println("")
	fmt.Println("用法: pikpak-downloader <命令> [参数]")
	fmt.Println("")
	fmt.Println("可用命令:")

	for name, cmd := range c.commands {
		fmt.Printf("  %-10s %s\n", name, cmd.Description)
	}

	fmt.Println("")
	fmt.Println("命令详情:")
	fmt.Println("")

	// ls 命令详情
	fmt.Println("ls - 列出文件和目录")
	fmt.Println("  选项:")
	fmt.Println("    -path string     目录路径 (默认: \"/\")")
	fmt.Println("    -l               长格式显示")
	fmt.Println("    -h               人类可读格式")
	fmt.Println("  示例:")
	fmt.Println("    pikpak-downloader ls")
	fmt.Println("    pikpak-downloader ls -path \"/My Pack\" -l -h")
	fmt.Println("")

	// download 命令详情
	fmt.Println("download - 下载文件或文件夹")
	fmt.Println("  选项:")
	fmt.Println("    -path string     下载路径 (默认: \"/\")")
	fmt.Println("    -output string   输出目录 (默认: \"./downloads\")")
	fmt.Println("    -count int       并发数 (默认: 3)")
	fmt.Println("    -progress        显示进度 (默认: true)")
	fmt.Println("  示例:")
	fmt.Println("    pikpak-downloader download -path \"/My Pack/video.mp4\"")
	fmt.Println("    pikpak-downloader download -path \"/My Pack\" -output \"./my_downloads\"")
	fmt.Println("")

	// quota 命令详情
	fmt.Println("quota - 查看云盘配额")
	fmt.Println("  选项:")
	fmt.Println("    -h               人类可读格式 (默认: true)")
	fmt.Println("  示例:")
	fmt.Println("    pikpak-downloader quota")
	fmt.Println("")

	fmt.Println("配置:")
	fmt.Println("  在 .env 文件中配置 PikPak 认证信息:")
	fmt.Println("    PIKPAK_USERNAME=your_email@example.com")
	fmt.Println("    PIKPAK_PASSWORD=your_password")
	fmt.Println("    PIKPAK_REFRESH_TOKEN=your_refresh_token")
	fmt.Println("")

	return nil
}

func main() {
	if len(os.Args) < 2 {
		cli := NewCLI()
		cli.Run([]string{"help"})
		os.Exit(1)
	}

	cli := NewCLI()
	if err := cli.Run(os.Args[1:]); err != nil {
		log.Fatalf("错误: %v", err)
	}
}
