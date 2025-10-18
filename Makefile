.PHONY: build run clean deps help build-cli run-cli

# 默认目标
help:
	@echo "PikPak 个人云盘管理工具"
	@echo ""
	@echo "可用命令:"
	@echo "  deps      - 安装依赖"
	@echo "  build-cli - 编译CLI程序"
	@echo "  run-cli   - 运行CLI程序"
	@echo "  clean     - 清理构建文件"
	@echo "  help      - 显示此帮助信息"
	@echo ""
	@echo "CLI模式示例:"
	@echo "  make run-cli ls                   # 列出根目录文件"
	@echo "  make run-cli quota                # 查看配额"
	@echo "  make run-cli download -path '/My Pack'"

# 安装依赖
deps:
	@echo "📦 安装依赖..."
	go mod tidy
	@echo "📦 安装 pikpakcli..."
	go install github.com/52funny/pikpakcli@latest

# 编译CLI程序
build-cli: deps
	@echo "🔨 编译CLI程序..."
	go build -o pikpak-cli pikpak_cli.go pikpak_client.go config_manager.go
	@echo "✅ 编译完成: ./pikpak-cli"

# 运行CLI程序
run-cli:
	@echo "🚀 启动CLI程序..."
	@if [ -z "$(ARGS)" ]; then \
		./pikpak-cli help; \
	else \
		./pikpak-cli $(ARGS); \
	fi

# 清理构建文件
clean:
	@echo "🧹 清理文件..."
	rm -f pikpak-cli
	rm -rf downloads temp_cli_downloads
	go clean -cache
