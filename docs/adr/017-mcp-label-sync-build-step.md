# ADR 017: /mcp label sync build step

## Status

Accepted (2026-06-15)

## Context

Claude Code's `/mcp` dialog labels a server by its **config key**, not by the
`serverInfo.name` the server reports at initialize. gl-mcp already sets
`serverInfo.name = "gl-mcp v{version}"`, but the dialog showed "gitlab" (the
config key in `~/Downloads/.mcp.json`). Hand-editing the key to a version works
once, then goes stale at the next bump. The slk-mcp repo solved the same problem
with a `Makefile` + `scripts/sync-mcp-label.py` that re-keys to
`"slack v<version>"` from the binary's own reported version (fleet pattern).

## Decision

Port the pattern to gl-mcp:

- **`--version` / `-V` flag** in `main.rs` — prints the bare version and exits
  before any config load or server start (previously `--version` just started
  the server). Lets the sync script read the running binary's version.
- **`scripts/sync-mcp-label.py`** — finds the gl-mcp entry by its *binary path*
  (robust to a key that already carries a version), asks the binary
  `--version`, and renames the key to `"gl-mcp v<version>"`. Idempotent, atomic
  per-file write, keeps a `.bak`. Searches `~/.claude.json` (root + per-project)
  and `~/Downloads/.mcp.json` (where gl-mcp is actually registered), plus
  `$GL_MCP_CONFIG` if set.
- **`Makefile`** — `build`, `test`, `sync-label`, `install: build sync-label`,
  and a `clean` that drops the build cache but preserves the runnable binary.
  `build` deliberately never touches the MCP config; `install` is the
  post-version-bump command.

Also corrected the tool count in `Cargo.toml`/`CLAUDE.md` to the actual **93**
(`#[tool]` methods in `server.rs`); recent docs had drifted to "98".

## Consequences

- After a version bump: `make install` then restart Claude Code / `/mcp`
  reconnect → the dialog shows the new `gl-mcp v<version>`. No more stale or
  hand-edited keys.
- Re-keying changes the tool namespace each release (`mcp__gl-mcp_v0_26_0__*`),
  so permission allowlists referencing the old namespace need re-granting — the
  accepted cost of a version-truthful label, same as slk-mcp.
- `build` and `sync-label` are separate on purpose: routine compiles never
  mutate the user's MCP config.
