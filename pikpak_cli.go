package main

import (
	"flag"
	"fmt"
	"log"
	"os"
)

// Command command structure
type Command struct {
	Name        string
	Description string
	Handler     func(args []string) error
}

// CLI command line interface
type CLI struct {
	client   *PikPakClient
	commands map[string]Command
}

// NewCLI create CLI instance
func NewCLI() *CLI {
	client := NewPikPakClient()

	cli := &CLI{
		client:   client,
		commands: make(map[string]Command),
	}

	// Register commands
	cli.registerCommands()
	return cli
}

// registerCommands register all commands
func (c *CLI) registerCommands() {
	c.commands["ls"] = Command{
		Name:        "ls",
		Description: "List files and directories",
		Handler:     c.handleList,
	}

	c.commands["download"] = Command{
		Name:        "download",
		Description: "Download files or folders",
		Handler:     c.handleDownload,
	}

	c.commands["quota"] = Command{
		Name:        "quota",
		Description: "View cloud storage quota",
		Handler:     c.handleQuota,
	}

	c.commands["help"] = Command{
		Name:        "help",
		Description: "Show help information",
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

	return fmt.Errorf("Unknown command: %s", commandName)
}

// handleList handle list command
func (c *CLI) handleList(args []string) error {
	flags := flag.NewFlagSet("ls", flag.ExitOnError)
	path := flags.String("path", "/", "Directory path")
	longFormat := flags.Bool("l", false, "Long format display")
	humanReadable := flags.Bool("h", false, "Human readable format")

	if err := flags.Parse(args); err != nil {
		return err
	}

	// Check configuration
	if err := c.client.CheckConfig(); err != nil {
		return fmt.Errorf("Configuration check failed: %v", err)
	}

	// List files
	files, err := c.client.ListFiles(*path, *longFormat, *humanReadable)
	if err != nil {
		return err
	}

	// Display files
	c.client.PrintFiles(files, *longFormat, *humanReadable)
	return nil
}

// handleDownload handle download command
func (c *CLI) handleDownload(args []string) error {
	flags := flag.NewFlagSet("download", flag.ExitOnError)
	path := flags.String("path", "/", "Download path")
	outputDir := flags.String("output", "./downloads", "Output directory")
	concurrency := flags.Int("count", 3, "Concurrency count")
	progress := flags.Bool("progress", true, "Show progress")

	if err := flags.Parse(args); err != nil {
		return err
	}

	// Check configuration
	if err := c.client.CheckConfig(); err != nil {
		return fmt.Errorf("Configuration check failed: %v", err)
	}

	fmt.Printf("📥 Starting download: %s\n", *path)
	fmt.Printf("📁 Output directory: %s\n", *outputDir)
	fmt.Printf("⚡ Concurrency: %d\n", *concurrency)

	// Start download
	return c.client.DownloadFile(*path, *outputDir, *concurrency, *progress)
}

// handleQuota handle quota command
func (c *CLI) handleQuota(args []string) error {
	flags := flag.NewFlagSet("quota", flag.ExitOnError)
	humanReadable := flags.Bool("h", true, "Human readable format")

	if err := flags.Parse(args); err != nil {
		return err
	}

	// Check configuration
	if err := c.client.CheckConfig(); err != nil {
		return fmt.Errorf("Configuration check failed: %v", err)
	}

	// Get quota information
	quota, err := c.client.GetQuota()
	if err != nil {
		return err
	}

	// Display quota information
	fmt.Println("📊 Cloud storage quota information:")
	fmt.Printf("Total capacity: %s\n", c.client.formatSize(quota.Total, *humanReadable))
	fmt.Printf("Used: %s\n", c.client.formatSize(quota.Used, *humanReadable))

	if quota.Total > 0 {
		percentage := float64(quota.Used) / float64(quota.Total) * 100
		fmt.Printf("Usage rate: %.1f%%\n", percentage)
	}

	return nil
}

// handleHelp handle help command
func (c *CLI) handleHelp(args []string) error {
	fmt.Println("PikPak Personal Cloud Storage Management Tool")
	fmt.Println("")
	fmt.Println("Usage: pikpak-downloader <command> [parameters]")
	fmt.Println("")
	fmt.Println("Available commands:")

	for name, cmd := range c.commands {
		fmt.Printf("  %-10s %s\n", name, cmd.Description)
	}

	fmt.Println("")
	fmt.Println("Command details:")
	fmt.Println("")

	// ls command details
	fmt.Println("ls - List files and directories")
	fmt.Println("  Options:")
	fmt.Println("    -path string     Directory path (default: \"/\")")
	fmt.Println("    -l               Long format display")
	fmt.Println("    -h               Human readable format")
	fmt.Println("  Examples:")
	fmt.Println("    pikpak-downloader ls")
	fmt.Println("    pikpak-downloader ls -path \"/My Pack\" -l -h")
	fmt.Println("")

	// download command details
	fmt.Println("download - Download files or folders")
	fmt.Println("  Options:")
	fmt.Println("    -path string     Download path (default: \"/\")")
	fmt.Println("    -output string   Output directory (default: \"./downloads\")")
	fmt.Println("    -count int       Concurrency count (default: 3)")
	fmt.Println("    -progress        Show progress (default: true)")
	fmt.Println("  Examples:")
	fmt.Println("    pikpak-downloader download -path \"/My Pack/video.mp4\"")
	fmt.Println("    pikpak-downloader download -path \"/My Pack\" -output \"./my_downloads\"")
	fmt.Println("")

	// quota command details
	fmt.Println("quota - View cloud storage quota")
	fmt.Println("  Options:")
	fmt.Println("    -h               Human readable format (default: true)")
	fmt.Println("  Examples:")
	fmt.Println("    pikpak-downloader quota")
	fmt.Println("")

	fmt.Println("Configuration:")
	fmt.Println("  Configure PikPak authentication in .env file:")
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
		log.Fatalf("Error: %v", err)
	}
}
