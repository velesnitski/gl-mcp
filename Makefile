.PHONY: build test sync-label install clean

## build: compile the release binary (version embedded via CARGO_PKG_VERSION)
build:
	cargo build --release

## test: run the full test suite (env-var tests need a single thread)
test:
	cargo test -- --test-threads=1

## sync-label: rename the gl-mcp key in the MCP config to "gl-mcp v<version>"
## so the /mcp dialog shows the running version (the dialog labels by config
## key, not the server-reported name). Idempotent; keeps a .bak.
sync-label:
	python3 scripts/sync-mcp-label.py

## install: build then sync the /mcp label. Run this after a version bump,
## then restart Claude Code / '/mcp' reconnect. (Kept separate from `build` so
## plain builds never touch your MCP config — sync only when you mean to.)
install: build sync-label

## clean: drop the build cache but keep the runnable release binary, so the
## MCP server still launches without a full rebuild.
clean:
	@cp -f target/release/gl-mcp /tmp/gl-mcp.keep 2>/dev/null || true
	cargo clean
	@mkdir -p target/release && mv -f /tmp/gl-mcp.keep target/release/gl-mcp 2>/dev/null || true
	@echo "build cache cleared; release binary preserved"
