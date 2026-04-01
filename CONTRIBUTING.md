# Contributing to gl-mcp

## Development Setup

```bash
# Rust 1.85+ required (edition 2024)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Build
cargo build --release

# Test (single-threaded: env var tests share process state)
cargo test -- --test-threads=1
```

## Branch Workflow

1. Fork the repo
2. Branch from `dev` (not `main`)
3. Make your changes
4. Run `cargo test -- --test-threads=1`
5. Run `cargo clippy` (no warnings)
6. Submit PR to `dev`

`main` is protected and only updated via PR merges from `dev`.

## Adding a New Tool

1. **Implement** the function in `src/tools/<module>.rs`
   - Return `crate::error::Result<String>`
   - Accept `client: &GitLabClient` as first param
   - Use `urlencoding::encode()` for project paths in URLs

2. **Add param struct** in `src/server.rs`
   - `#[derive(Debug, Deserialize, JsonSchema)]`
   - Use `#[schemars(description = "...")]` on each field
   - Optional numbers: `#[serde(default, deserialize_with = "flex::deserialize_opt_u32")]`
   - Always include `instance: Option<String>` for multi-instance support

3. **Wire the tool** in `src/server.rs`
   - Add `#[tool(description = "...")]` method inside `#[tool_router] impl`
   - Use `tool_call!` macro for consistent error handling and analytics
   - Use `resolve_client` for instance resolution

4. **If write tool**: add to `WRITE_TOOLS` in `src/tools/mod.rs` + `write_guard!` in method

5. **Update documentation**:
   - Add to tool table in `README.md`
   - Add entry to `CHANGELOG.md` under `[Unreleased]`

## Code Standards

- **No `unwrap()` in tool functions** - use `?` or `.unwrap_or()`
- **Token efficiency** - always consider response size; add `summary_only` where appropriate
- **Smart filtering** - skip lockfiles and generated code via `should_skip_file()`
- **Compact mode** - `tool_call!` macro handles this automatically
- **No hardcoded instance names** - use generic examples in descriptions

## Testing

- All tests run without network access (unit tests only)
- Test language detection, tool filtering, config parsing
- Use `macro_rules! set_env` / `rm_env` for env var tests (unsafe in edition 2024)

## License

All contributions are licensed under the MIT License.
