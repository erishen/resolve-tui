PKG := resolve-tui
BIN_PATTERN := target/.*/resolve-tui

.PHONY: build run tui dev stop fmt clippy test clean

build:
	@CARGO_TERM_QUIET=true cargo build -p $(PKG)

run:
	@CARGO_TERM_QUIET=true cargo run -p $(PKG)

tui:
	@CARGO_TERM_QUIET=true cargo run -p $(PKG) -- --tui

dev:
	-@pkill -f '$(BIN_PATTERN)' 2>/dev/null || true
	@echo ""
	@echo "╔══════════════════════════════════════════════════════════════╗"
	@echo "║                    resolve-tui 启动中...                       ║"
	@echo "╠══════════════════════════════════════════════════════════════╣"
	@echo "║  快捷键:                                                       ║"
	@echo "║    Enter    - 提交输入                                        ║"
	@echo "║    Esc      - 退出 / 中止生成                                 ║"
	@echo "║    /help    - 查看所有命令                                    ║"
	@echo "║    /create  - 新建会话                                        ║"
	@echo "║    /save    - 保存当前会话                                    ║"
	@echo "║    /resume  - 恢复历史会话                                    ║"
	@echo "╠══════════════════════════════════════════════════════════════╣"
	@echo "║  提示: TUI 启动后将接管终端，日志请使用 CLI 模式查看          ║"
	@echo "║        CLI 模式: make run -- \"你的任务\"                       ║"
	@echo "╚══════════════════════════════════════════════════════════════╝"
	@echo ""
	@sleep 1
	@CARGO_TERM_QUIET=true cargo run -p $(PKG) -- --tui

stop:
	-@pkill -f '$(BIN_PATTERN)' 2>/dev/null || true

fmt:
	cargo fmt -p $(PKG)

clippy:
	cargo clippy -p $(PKG) --all-targets -- -D warnings

test:
	cargo test -p $(PKG)

clean:
	cargo clean -p $(PKG)
