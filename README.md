# gl-mcp

GitLab MCP server for Claude Code. Single Rust binary, 4MB, 16 tools.

## Quick Start

```bash
# Build
cargo build --release

# Configure Claude Code (.mcp.json in your project root)
cat > .mcp.json << 'EOF'
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
EOF
```

Restart Claude Code. The `gitlab` MCP server will be available.

## Tools (16)

### Projects
| Tool | Description |
|------|-------------|
| `list_projects` | List accessible projects |
| `get_project` | Project details (stars, forks, topics) |
| `list_members` | Project members with access levels |
| `list_group_projects` | All projects in a group (with subgroups) |

### Issues
| Tool | Description |
|------|-------------|
| `search_issues` | Search across projects, filter by state/labels/assignee |
| `get_issue` | Full details with description and comments |
| `create_issue` | Create issue with labels and assignee |

### Merge Requests
| Tool | Description |
|------|-------------|
| `list_merge_requests` | List MRs, filter by state/scope |
| `get_merge_request` | Full MR details with pipeline status and comments |

### CI/CD Pipelines
| Tool | Description |
|------|-------------|
| `list_pipelines` | List pipelines, filter by status/ref |
| `get_pipeline` | Pipeline details with jobs grouped by stage |

### Commits & Code Review
| Tool | Description |
|------|-------------|
| `list_commits` | Commits by branch/author/date, grouped by author |
| `get_commit_diff` | Commit diff with smart filtering and language grouping |
| `get_mr_changes` | MR unified diff with smart filtering |
| `get_file_content` | File content at any branch/tag/SHA |
| `get_user_activity` | Developer metrics: commits, MRs opened/merged/approved |

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

## Build

```bash
cargo build --release
# Binary: target/release/gl-mcp (~4MB)
```

Cross-compile:
```bash
# Linux
cargo build --release --target x86_64-unknown-linux-gnu

# macOS Intel
cargo build --release --target x86_64-apple-darwin
```

## License

MIT
