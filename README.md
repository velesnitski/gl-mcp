# gl-mcp

[![CI](https://github.com/velesnitski/gl-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/velesnitski/gl-mcp/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/velesnitski/gl-mcp?color=green)](https://github.com/velesnitski/gl-mcp/releases/latest)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![MCP](https://img.shields.io/badge/MCP-compatible-purple)](https://modelcontextprotocol.io)
[![Rust](https://img.shields.io/badge/Rust-1.80+-orange.svg)](https://www.rust-lang.org)

**GitLab MCP server with 66 tools** for projects, issues, merge requests, CI/CD, code review, team analytics, and code quality analysis.

Single Rust binary. Zero runtime dependencies. Works with Claude Code, GitHub Copilot, Cursor, Windsurf, n8n, and any MCP-compatible client.

---

## Highlights

- **66 tools** across 9 categories — from basic CRUD to advanced analytics
- **Code quality analysis** — file-level scoring (A–F), project-wide reports, 41 lint rules for Swift/PHP/Go/Kotlin/TypeScript
- **Team performance reports** — developer comparison, review matrix, MR turnaround, auto-detected process issues
- **HTML reports** — dark-theme reports with Export PDF button for dev activity, team performance, and project quality
- **Token optimization** — `summary_only` mode (~5–10x smaller responses), smart diff filtering, compact mode
- **Multi-instance** — query multiple GitLab instances, auto-resolve by domain
- **Response caching** — 60s TTL cache for repeated lookups, rate limit retry with backoff
- **Docker + n8n** — HTTP/SSE transport, ready-to-use docker-compose

---

## Quick Start

### Install

Download a pre-built binary from [GitHub Releases](https://github.com/velesnitski/gl-mcp/releases/latest):

| Platform | Download |
|----------|----------|
| macOS (Apple Silicon) | [`gl-mcp-aarch64-macos`](https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-aarch64-macos) |
| macOS (Intel) | [`gl-mcp-x86_64-macos`](https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-x86_64-macos) |
| Linux (x86_64) | [`gl-mcp-x86_64-linux`](https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-x86_64-linux) |
| Windows (x86_64) | [`gl-mcp-x86_64-windows.exe`](https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-x86_64-windows.exe) |

Or build from source:

```bash
cargo build --release
```

### Configure

<details>
<summary><strong>Claude Code</strong></summary>

Add to `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "gitlab": {
      "command": "/path/to/gl-mcp",
      "env": {
        "GITLAB_URL": "https://gitlab.example.com",
        "GITLAB_TOKEN": "glpat-your-token-here"
      }
    }
  }
}
```
</details>

<details>
<summary><strong>GitHub Copilot (VS Code)</strong></summary>

Add to `.vscode/mcp.json`:

```json
{
  "servers": {
    "gitlab": {
      "type": "stdio",
      "command": "/path/to/gl-mcp",
      "env": {
        "GITLAB_URL": "https://gitlab.example.com",
        "GITLAB_TOKEN": "glpat-your-token-here"
      }
    }
  }
}
```
</details>

<details>
<summary><strong>Docker / n8n</strong></summary>

```bash
docker compose up -d
```

Or run directly:

```bash
docker build -t gl-mcp .
docker run -p 8000:8000 \
  -e GITLAB_URL=https://gitlab.example.com \
  -e GITLAB_TOKEN=glpat-your-token-here \
  gl-mcp
```

In n8n, add an MCP Client node pointing to `http://localhost:8000/mcp`.

</details>

<details>
<summary><strong>Cursor / Windsurf / Other</strong></summary>

gl-mcp uses stdio transport by default — point your client's MCP config to the binary and set `GITLAB_URL` + `GITLAB_TOKEN`.

For HTTP transport: `gl-mcp --transport http --port 8000`

</details>

---

## Tools (66)

### Projects & Users
| Tool | Description |
|------|-------------|
| `list_projects` | List accessible projects |
| `get_project` | Project details (stars, forks, topics) |
| `get_project_stats` | Repo size, file counts, language breakdown, binary detection |
| `get_user` | User info by username or ID |
| `list_members` | Project members with access levels |
| `list_group_projects` | All projects in a group (with subgroups) |
| `list_branches` | List branches, filtered by name |
| `get_stale_branches` | Find merged-but-not-deleted and inactive branches |
| `delete_branch` | Delete a branch (e.g., after merge) |

### Issues
| Tool | Description |
|------|-------------|
| `search_issues` | Search across projects or groups, filter by state/labels/assignee |
| `get_issue` | Full details with description and comments |
| `create_issue` | Create issue with labels and assignee |
| `update_issue` | Update title, description, state, labels, assignee |
| `add_note` | Add comment to issue or MR |

### Merge Requests
| Tool | Description |
|------|-------------|
| `list_merge_requests` | List MRs with pipeline status, reviewers; filter by group/state/author/date |
| `create_merge_request` | Smart MR creation: auto-title from branch, auto-description from commits |
| `get_merge_request` | Full MR details with pipeline status and comments |
| `get_mr_turnaround` | Avg/median merge time, per-author and per-merger breakdown |
| `get_mr_dashboard` | Compact group dashboard: open count, avg age, reviewer bottlenecks |
| `get_mr_review_depth` | Comments/discussions per MR, zero-review detection |
| `get_mr_categories` | Classify MRs by branch convention (feature/hotfix/bugfix/chore) |
| `get_mr_timeline` | Decompose merge time into queue vs review phases |
| `get_org_mr_dashboard` | Cross-group MR aggregation with reviewer load |
| `get_cross_instance_dashboard` | Aggregate MR stats across multiple GitLab instances |

### CI/CD Pipelines
| Tool | Description |
|------|-------------|
| `list_pipelines` | List pipelines, filter by status/ref |
| `get_pipeline` | Pipeline details with jobs grouped by stage |
| `get_job_log` | Job log output (tail N lines) |
| `get_mr_pipelines` | List all pipelines for a specific MR |
| `retry_pipeline` | Retry a failed pipeline |
| `cancel_pipeline` | Cancel a running pipeline |

### Commits & Code Review
| Tool | Description |
|------|-------------|
| `list_commits` | Commits by branch/author/date, grouped by author |
| `get_commit_diff` | Commit diff with smart filtering and language grouping |
| `get_mr_changes` | MR unified diff with smart filtering |
| `get_file_content` | File content at any branch/tag/SHA |
| `compare_developers` | Side-by-side: LOC, MRs, reviews, merge time, review matrix |
| `get_user_activity` | Developer daily activity across all projects and instances |
| `get_team_activity` | Multiple users in one call (from teams.json or comma-separated) |
| `get_group_activity` | Auto-discover group members and aggregate activity |

### Repository
| Tool | Description |
|------|-------------|
| `search_code` | Search code with regex, returns file paths and snippets |
| `get_languages` | Project language breakdown with visual bars |
| `get_tree` | Repository directory listing (recursive optional) |
| `compare_branches` | Compare two refs with commit and file lists |
| `list_tags` | Tags/releases with commit info |
| `get_mr_approvals` | MR approval status: who approved, how many remaining |
| `get_approval_rules` | Project-level approval rules configuration |
| `get_contributors` | All-time contributor stats (commits, LOC per person) |
| `list_environments` | Environments with last deployment info |
| `get_deploy_frequency` | DORA metric: deploys per day by environment and deployer |
| `update_file` | Create/update file with branch protection and auto-MR |

### Code Quality & Lint
| Tool | Description |
|------|-------------|
| `analyze_file` | File metrics: lines, functions, nesting depth, complexity grade (A–F) |
| `analyze_project` | Batch quality analysis: per-file scores, grade distribution, top issues |
| `validate_commit` | Regex-based commit validation against 41 coding rules |
| `validate_mr` | Validate all commits in an MR |
| `validate_mr_changes` | Validate full MR unified diff (catches squashed MR issues) |
| `validate_project_commits` | Bulk commit message validation: conventional format, ticket refs |
| `list_rules` | Show available rules by language |

### Teams & HTML Reports
| Tool | Description |
|------|-------------|
| `list_teams` | Show configured teams from `~/.gl-mcp/teams.json` |
| `save_team` | Create/update team config |
| `generate_dev_report` | HTML developer report with auto-observations and Export PDF |
| `generate_team_report` | HTML team comparison with review matrix, MR sizes, process issues |
| `generate_project_report` | HTML project quality report with grade distribution, commit quality |

---

## Token Optimization

Diffs can be large. Three levels of compression:

```
# 1. Summary only (~10x smaller) — scan first
get_commit_diff(sha="abc123", summary_only=true)

# 2. Single file — drill into what matters
get_commit_diff(sha="abc123", file="AuthController.php")

# 3. Global compact mode — strip all markdown
GITLAB_COMPACT=1
```

`summary_only` is available on: `list_merge_requests`, `list_commits`, `get_commit_diff`, `get_mr_changes`, `compare_developers`, `get_mr_dashboard`, `get_mr_review_depth`, `get_mr_timeline`, `analyze_project`.

### Smart Filtering

Automatically skips lockfiles and generated code:
- `package-lock.json`, `yarn.lock`, `composer.lock`, `go.sum`, `Cargo.lock`
- `vendor/`, `node_modules/`, `dist/`, `build/`
- `.min.js`, `.min.css`, `.map`, `.pb.go`

Language detection for 25+ languages including PHP, Go, Kotlin, Swift, TypeScript, Rust, Python, and more.

---

## Configuration

### Environment Variables

| Variable | Required | Description |
|----------|----------|-------------|
| `GITLAB_URL` | Yes | GitLab instance URL |
| `GITLAB_TOKEN` | Yes | Personal access token (`api` or `read_api` scope) |
| `GITLAB_COMPACT` | No | Strip markdown formatting (`1`/`true`/`yes`) |
| `GITLAB_READ_ONLY` | No | Disable write tools (`1`/`true`/`yes`) |
| `DISABLED_TOOLS` | No | Comma-separated tools to disable |
| `SENTRY_DSN` | No | Sentry DSN for error tracking |
| `GITLAB_ALLOW_HTTP` | No | Allow non-HTTPS URLs |

### Multi-Instance

```json
{
  "env": {
    "GITLAB_INSTANCES": "main,staging",
    "GITLAB_MAIN_URL": "https://gitlab.example.com",
    "GITLAB_MAIN_TOKEN": "glpat-xxx",
    "GITLAB_STAGING_URL": "https://staging.gitlab.example.com",
    "GITLAB_STAGING_TOKEN": "glpat-yyy"
  }
}
```

Pass `instance="staging"` to any tool. URLs auto-resolve to the correct instance. `get_user_activity` queries all instances when none specified.

### Transport Options

| Flag | Description |
|------|-------------|
| *(default)* | stdio — for Claude Code, Copilot, Cursor |
| `--transport http` | HTTP/SSE — for n8n, web clients, Docker |
| `--port 8000` | Port for HTTP transport (default: 8000) |

### Required Token Scopes

| Scope | Needed for |
|-------|-----------|
| `read_api` | All read tools (minimum) |
| `api` | Write tools (`create_issue`, `create_merge_request`, etc.) |
| `read_user` | `get_user_activity` |
| `read_repository` | `get_file_content`, `get_commit_diff` |

---

## Architecture

- **Single binary** — ~9MB, zero runtime dependencies
- **Frozen config** — env vars parsed once at startup
- **Multi-instance resolver** — domain auto-detection from URLs
- **Response cache** — 60s TTL for user/project lookups, auto-eviction
- **Rate limit retry** — HTTP 429 with Retry-After header, up to 3 attempts
- **Analytics logging** — `~/.gl-mcp/analytics.log` (JSON, safe params only)
- **Sentry integration** — optional error tracking with PII scrubbing

### Stack

| Crate | Purpose |
|-------|---------|
| [rmcp](https://github.com/modelcontextprotocol/rust-sdk) | MCP protocol (stdio + HTTP) |
| [axum](https://github.com/tokio-rs/axum) | HTTP server for n8n/Docker |
| [reqwest](https://crates.io/crates/reqwest) | HTTP client with connection pooling |
| [tokio](https://tokio.rs) | Async runtime |
| [serde](https://serde.rs) / [schemars](https://crates.io/crates/schemars) | JSON + schema generation |

---

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

[MIT](LICENSE)
