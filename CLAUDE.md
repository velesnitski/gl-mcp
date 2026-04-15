# gl-mcp

GitLab MCP server. Rust, single binary, 66 tools.

## Build & Test

```bash
cargo build --release          # binary: target/release/gl-mcp (~9MB)
cargo test -- --test-threads=1 # env var tests must run single-threaded
```

## Architecture

```
src/
├── main.rs          # Entry point: stdio (default) or HTTP transport
├── lib.rs           # Library crate (for tests)
├── server.rs        # rmcp tool registration, tool_call! + write_guard! macros
├── params.rs        # All 66 parameter structs (serde + schemars)
├── config.rs        # Env var config (GITLAB_URL, GITLAB_TOKEN, etc.)
├── client.rs        # GitLab API HTTP client (reqwest, caching, retry, pagination)
├── resolver.rs      # Multi-instance resolution (domain auto-detect from URLs)
├── logging.rs       # Analytics + Sentry integration
├── error.rs         # Error types
├── teams.rs         # teams.json management
└── tools/
    ├── mod.rs           # WRITE_TOOLS list, is_tool_enabled()
    ├── projects.rs      # list_projects, get_project, list_members, list_branches, get_user, delete_branch
    ├── issues.rs        # search/get/create/update_issue, add_note (group-scoped search)
    ├── merge_requests.rs # list/get MRs, turnaround, dashboard, review depth, timeline, categories
    ├── pipelines.rs     # list/get pipelines, get_job_log, retry/cancel, get_mr_pipelines
    ├── commits.rs       # list_commits, diffs, file_content, user/team/group activity, compare_developers
    ├── repository.rs    # search_code, tree, languages, tags, branches, environments, deploy_frequency, project_stats
    ├── reports.rs       # HTML reports: dev, team, project (with auto-observations, print CSS)
    └── lint.rs          # validate_commit/mr/mr_changes, analyze_file/project, validate_project_commits
```

## Key Patterns

- **`tool_call!` macro** — wraps every tool with analytics logging + compact mode + size warnings
- **`write_guard!` macro** — blocks write tools in read-only mode
- **`summary_only` param** — compact 1-3 line responses for token efficiency
- **Response cache** — 60s TTL for user/project lookups, auto-eviction at 500 entries
- **Rate limit retry** — HTTP 429 retry with Retry-After header, up to 3 attempts
- **`get_all_pages`** — generic pagination helper for multi-page GitLab API responses
- **Multi-instance** — auto-discovers all configured instances for user activity queries

## Env Vars

| Var | Required | Notes |
|-----|----------|-------|
| `GITLAB_URL` | yes | Instance URL |
| `GITLAB_TOKEN` | yes | PAT with `read_api` minimum |
| `GITLAB_COMPACT` | no | `1` strips markdown from all responses |
| `GITLAB_READ_ONLY` | no | `1` blocks write tools |
| `DISABLED_TOOLS` | no | Comma-separated tool names |
| `GITLAB_INSTANCES` | no | Multi-instance: comma-separated names |
| `SENTRY_DSN` | no | Sentry error tracking |

## Adding a New Tool

1. Add function in `src/tools/<module>.rs` — returns `Result<String>`
2. Add param struct in `src/params.rs` with `#[derive(Debug, Deserialize, JsonSchema)]`
3. Add `#[tool]` method in `server.rs` using `tool_call!` macro
4. If write tool: add to `WRITE_TOOLS` in `mod.rs` + add `write_guard!` in method
5. Run `cargo test -- --test-threads=1`

## Branch Workflow

- `main` — protected, releases tagged here
- `dev` — active development
