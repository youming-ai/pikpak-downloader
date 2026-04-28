.PHONY: build run clean test help

help:
	@echo "PikPak Personal Cloud Storage Management Tool (Rust)"
	@echo ""
	@echo "Available commands:"
	@echo "  build  - Build the CLI binary"
	@echo "  run    - Run the CLI binary"
	@echo "  test   - Run all tests"
	@echo "  clean  - Clean build artifacts"
	@echo "  help   - Show this help information"
	@echo ""
	@echo "CLI mode examples:"
	@echo "  make run                          # Show help"
	@echo "  make run ARGS='ls'                # List root directory"
	@echo "  make run ARGS='ls --path \"/My Pack\" -l -h'"
	@echo "  make run ARGS='quota'             # View quota"
	@echo "  make run ARGS='download --path \"/My Pack/video.mp4\"'"
	@echo ""
	@echo "Install binary to PATH:"
	@echo "  cd rust && cargo install --path pikpak-cli"

build:
	cd rust && cargo build --release
	@echo "Binary: rust/target/release/pikpak-cli"

run:
	@if [ -z "$(ARGS)" ]; then \
		cd rust && cargo run --bin pikpak-cli -- help; \
	else \
		cd rust && cargo run --bin pikpak-cli -- $(ARGS); \
	fi

test:
	cd rust && cargo test

clean:
	cd rust && cargo clean
	rm -rf downloads
