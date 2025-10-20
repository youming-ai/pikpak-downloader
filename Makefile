.PHONY: build run clean deps help build-cli run-cli

# Default target
help:
	@echo "PikPak Personal Cloud Storage Management Tool"
	@echo ""
	@echo "Available commands:"
	@echo "  deps      - Install dependencies"
	@echo "  build-cli - Build CLI program"
	@echo "  run-cli   - Run CLI program"
	@echo "  clean     - Clean build files"
	@echo "  help      - Show this help information"
	@echo ""
	@echo "CLI mode examples:"
	@echo "  make run-cli ls                   # List root directory files"
	@echo "  make run-cli quota                # View quota"
	@echo "  make run-cli download -path '/My Pack'"

# Install dependencies
deps:
	@echo "📦 Installing dependencies..."
	go mod tidy
	@echo "📦 Installing pikpakcli..."
	go install github.com/52funny/pikpakcli@latest

# Build CLI program
build-cli: deps
	@echo "🔨 Building CLI program..."
	go build -o pikpak-cli pikpak_cli.go pikpak_client.go config_manager.go
	@echo "✅ Build completed: ./pikpak-cli"

# Run CLI program
run-cli:
	@echo "🚀 Starting CLI program..."
	@if [ -z "$(ARGS)" ]; then \
		./pikpak-cli help; \
	else \
		./pikpak-cli $(ARGS); \
	fi

# Clean build files
clean:
	@echo "🧹 Cleaning files..."
	rm -f pikpak-cli
	rm -rf downloads temp_cli_downloads
	go clean -cache
