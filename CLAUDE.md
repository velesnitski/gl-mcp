# gl-mcp

GitLab MCP server for Claude Code. Rust, single binary.

## Build & Test

```bash
cargo build --release          # binary: target/release/gl-mcp (~4MB)
cargo test -- --test-threads=1 # env var tests must run single-threaded
```

## Architecture

```
src/
├── main.rs          # MCP stdio entry point
├── lib.rs           # library crate (for tests + future gl-report)
├── server.rs        # rmcp tool registration, tool_call! + write_guard! macros
├── config.rs        # env var config (GITLAB_URL, GITLAB_TOKEN, etc.)
├── client.rs        # GitLab API HTTP client (reqwest, connection pooling)
├── resolver.rs      # multi-instance resolution (domain auto-detect from URLs)
├── logging.rs       # analytics (ToolTimer → ~/.gl-mcp/analytics.log), instance_id
└── tools/
    ├── mod.rs           # WRITE_TOOLS list, is_tool_enabled()
    ├── projects.rs      # list_projects, get_project, list_members, list_branches
    ├── issues.rs        # search/get/create/update_issue, add_note
    ├── merge_requests.rs # list/get merge requests
    ├── pipelines.rs     # list/get pipelines, get_job_log, retry/cancel
    └── commits.rs       # list_commits, get_commit_diff, get_mr_changes,
                         # get_file_content, get_user_activity, list_group_projects
```

## Key Patterns

- **`tool_call!` macro** — wraps every tool with analytics logging + compact mode. One line per tool.
- **`write_guard!` macro** — blocks write tools in read-only mode. Added at top of each write tool method.
- **Smart diff filtering** — skips lockfiles/generated code, groups by language, truncates large diffs.
- **`summary_only` param** — ~10x token savings on diffs. Use first, then `file=` to drill in.
- **Client-side author filter** — GitLab API exact-matches author names, fails on Cyrillic. We fetch all and filter locally with case-insensitive contains on name + email.

## Env Vars

| Var | Required | Notes |
|-----|----------|-------|
| `GITLAB_URL` | yes | Instance URL |
| `GITLAB_TOKEN` | yes | PAT with `read_api` minimum |
| `GITLAB_COMPACT` | no | `1` strips markdown from all responses |
| `GITLAB_READ_ONLY` | no | `1` blocks write tools |
| `DISABLED_TOOLS` | no | Comma-separated tool names |
| `GITLAB_INSTANCES` | no | Multi-instance: comma-separated names |

## Adding a New Tool

1. Add function in `src/tools/<module>.rs` — returns `Result<String, String>`
2. Add param struct in `src/server.rs` with `#[derive(Debug, Deserialize, JsonSchema)]`
3. Add `#[tool]` method in `server.rs` using `tool_call!` macro
4. If write tool: add to `WRITE_TOOLS` in `mod.rs` + add `write_guard!` in method
5. Run `cargo test -- --test-threads=1`

## Branch Workflow

- `main` — protected, requires PR review
- `dev` — active development, push freely
