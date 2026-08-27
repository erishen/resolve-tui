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
