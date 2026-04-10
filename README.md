# gl-mcp

[![CI](https://github.com/velesnitski/gl-mcp/actions/workflows/ci.yml/badge.svg)](https://github.com/velesnitski/gl-mcp/actions/workflows/ci.yml)
[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Version](https://img.shields.io/badge/version-0.11.0-green.svg)](CHANGELOG.md)

GitLab MCP server for Claude Code, GitHub Copilot, Cursor, and other MCP clients. Single Rust binary, ~5MB, 58 tools.

## Install

### Pre-built binaries

Download from [GitHub Releases](https://github.com/velesnitski/gl-mcp/releases/latest):

```bash
# macOS (Apple Silicon)
curl -L https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-aarch64-macos -o gl-mcp
chmod +x gl-mcp

# macOS (Intel)
curl -L https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-x86_64-macos -o gl-mcp
chmod +x gl-mcp

# Linux (x86_64)
curl -L https://github.com/velesnitski/gl-mcp/releases/latest/download/gl-mcp-x86_64-linux -o gl-mcp
chmod +x gl-mcp
```

### Build from source

```bash
cargo build --release
# Binary: target/release/gl-mcp
```

## Quick Start

### Claude Code

Add to `.mcp.json` in your project root:

```json
{
  "mcpServers": {
    "gitlab": {
      "command": "/path/to/gl-mcp/target/release/gl-mcp",
      "env": {
        "GITLAB_URL": "https://gitlab.example.com",
        "GITLAB_TOKEN": "glpat-your-token-here"
      }
    }
  }
}
```

### GitHub Copilot (VS Code)

Add to `.vscode/mcp.json` in your project root:

```json
{
  "servers": {
    "gitlab": {
      "type": "stdio",
      "command": "/path/to/gl-mcp/target/release/gl-mcp",
      "env": {
        "GITLAB_URL": "https://gitlab.example.com",
        "GITLAB_TOKEN": "glpat-your-token-here"
      }
    }
  }
}
```

### Cursor / Windsurf / Other MCP Clients

gl-mcp supports stdio transport (default) – it works with any MCP-compatible client. Point your client's MCP configuration to the binary and set the environment variables above.

### Docker

```bash
docker compose up -d
```

Or build and run directly:

```bash
docker build -t gl-mcp .
docker run -p 8000:8000 \
  -e GITLAB_URL=https://gitlab.example.com \
  -e GITLAB_TOKEN=glpat-your-token-here \
  gl-mcp
```

The Docker image runs in HTTP mode by default on port 8000.

### n8n Integration

1. Start gl-mcp in HTTP mode:

```bash
# Via Docker
docker compose up -d

# Or directly
gl-mcp --transport http --port 8000
```

2. In n8n, add an **MCP Client** node
3. Set the MCP server URL to `http://localhost:8000/mcp`
4. All 58 tools will be available as actions in your n8n workflows

### Transport Options

| Flag | Description |
|------|-------------|
| (default) | stdio transport for Claude Code, Copilot, Cursor |
| `--transport http` | HTTP/SSE transport for n8n, web clients |
| `--port 8000` | Port for HTTP transport (default: 8000) |

## Tools (58)

### Projects & Branches
| Tool | Description |
|------|-------------|
| `list_projects` | List accessible projects |
| `get_user` | User info by username or ID |
| `get_project` | Project details (stars, forks, topics) |
| `list_members` | Project members with access levels |
| `list_group_projects` | All projects in a group (with subgroups) |
| `list_branches` | List branches, filtered by name |
| `get_stale_branches` | Find merged-but-not-deleted and inactive branches |
| `delete_branch` | Delete a branch (e.g., after merge) |

### Issues
| Tool | Description |
|------|-------------|
| `search_issues` | Search across projects, filter by state/labels/assignee |
| `get_issue` | Full details with description and comments |
| `create_issue` | Create issue with labels and assignee |
| `update_issue` | Update title, description, state, labels, assignee |
| `add_note` | Add comment to issue or MR |

### Merge Requests
| Tool | Description |
|------|-------------|
| `list_merge_requests` | List MRs with pipeline status, reviewers; filter by group/state/author/date |
| `create_merge_request` | Smart MR creation: auto-title from branch, auto-description from commits, duplicate check |
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
| `retry_pipeline` | Retry a failed pipeline |
| `cancel_pipeline` | Cancel a running pipeline |
| `get_mr_pipelines` | List all pipelines for a specific MR |

### Commits & Code Review
| Tool | Description |
|------|-------------|
| `list_commits` | Commits by branch/author/date, grouped by author |
| `get_commit_diff` | Commit diff with smart filtering and language grouping |
| `get_mr_changes` | MR unified diff with smart filtering |
| `get_file_content` | File content at any branch/tag/SHA |
| `get_user_activity` | Developer daily activity across all projects |
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

### Lint & Quality
| Tool | Description |
|------|-------------|
| `validate_commit` | Regex-based commit validation (zero LLM tokens) |
| `validate_mr` | Validate all commits in an MR against coding rules |
| `list_rules` | Show available rules by language |

### Teams & Reports
| Tool | Description |
|------|-------------|
| `list_teams` | Show configured teams from `~/.gl-mcp/teams.json` |
| `save_team` | Create/update team config |
| `generate_dev_report` | Full HTML daily report with dark theme |

## Token Compression

Diffs can be large. Three levels of compression:

```
# 1. Summary only (~10x smaller) - use first to scan
get_commit_diff(sha="abc123", summary_only=true)

# 2. Single file - drill into what matters
get_commit_diff(sha="abc123", file="AuthController.php")

# 3. Global compact mode - strip all markdown
GITLAB_COMPACT=1
```

| Mode | Response size | Use when |
|------|--------------|----------|
| Full diff | ~4000 chars | Reviewing 1-2 commits |
| `summary_only=true` | ~300 chars | Scanning 10+ commits |
| `file="path"` | ~500 chars | Drilling into specific file |
| `GITLAB_COMPACT=1` | ~40% smaller | Always-on token savings |

### Smart Filtering

Automatically skips lockfiles and generated code:
- `package-lock.json`, `yarn.lock`, `composer.lock`, `go.sum`, `Cargo.lock`
- `vendor/`, `node_modules/`, `dist/`, `build/`
- `.min.js`, `.min.css`, `.map`, `.pb.go`

Language detection for: PHP, Go, Kotlin, Java, Swift, TypeScript, JavaScript, Rust, Python, YAML/Ansible, Shell, SQL, Vue, CSS, Gradle, Docker, CI/CD.

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
| `SENTRY_ENVIRONMENT` | No | Sentry environment (default: `production`) |
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

Pass `instance="staging"` to any tool. URLs in issue/MR identifiers auto-resolve to the correct instance.

### Required Token Scopes

| Scope | Needed for |
|-------|-----------|
| `read_api` | All read tools (minimum) |
| `api` | `create_issue` and other write tools |
| `read_user` | `get_user_activity` |
| `read_repository` | `get_file_content`, `get_commit_diff` |

## Architecture

Mirrors [yt-mcp](https://github.com/velesnitski/yt-mcp) patterns:

- **Frozen config** from env vars, parsed once at startup
- **Multi-instance resolver** with domain auto-detection
- **Analytics logging** to `~/.gl-mcp/analytics.log` (JSON, safe params only)
- **Persistent instance_id** for installation tracking
- **Write tool filtering** via `GITLAB_READ_ONLY` / `DISABLED_TOOLS`
- **Response size warnings** on >15KB responses

### Stack

- [rmcp](https://github.com/modelcontextprotocol/rust-sdk) 0.11 - MCP protocol
- [reqwest](https://crates.io/crates/reqwest) - HTTP client with connection pooling
- [tokio](https://tokio.rs) - async runtime
- [serde](https://serde.rs) / [schemars](https://crates.io/crates/schemars) - JSON + schema generation

## License

MIT
