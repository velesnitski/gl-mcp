# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.10.0] - 2026-04-07

### Added
- `compare_developers` — side-by-side dev comparison with LOC, files, MR sizes, review matrix
- `generate_team_report` — full HTML team performance report with auto-detected process issues
- `generate_project_report` — full HTML project quality report with grade distribution, commit quality, binary detection
- `analyze_project` — batch file quality analysis with aggregate A–F scores
- `analyze_file` — single file quality metrics: line count, functions, nesting, complexity grade
- `get_project_stats` — repo size, file counts by type, binary file detection
- `validate_project_commits` — bulk commit message validation (conventional commits, ticket refs)
- `validate_mr_changes` — lint full MR unified diff (catches issues in squashed MRs)
- `delete_branch` — delete branch after merge (write tool)
- `get_mr_pipelines` — list all pipelines for a specific MR
- `get_user` — user info lookup by username or ID
- Group-scoped issue search via `group_id` parameter in `search_issues`
- 11 new lint rules: deep nesting, magic numbers, long lines, empty catch, too many params, nested ternary
- Print CSS + "Export PDF" button on all HTML reports
- TTL-based response cache (60s) for user/project lookups
- HTTP 429 rate limit retry with Retry-After header (up to 3 attempts)
- Generic `get_all_pages()` pagination helper
- HTTP/SSE transport for n8n and web clients (`--transport http --port 8000`)
- Docker support: Dockerfile with HTTP default, docker-compose.yml, .dockerignore
- 18 new unit tests (31 total)
- Extracted `params.rs` from `server.rs` for maintainability

### Fixed
- Lint regex pre-compiled once via LazyLock (was recompiling per line per rule)
- UTF-8 safe string truncation (was panicking on Cyrillic)
- Mutex poison recovery instead of panicking
- Concurrent HTTP for project name resolution and team activity
- NaN-safe float sorting in MR turnaround stats
- Token redacted from Debug output on GitLabInstance
- Broader Sentry token scrubbing pattern

## [0.9.0] - 2026-04-04

### Added
- HTTP/SSE transport for n8n and web clients (`--transport http --port 8000`)
- Docker support: Dockerfile with HTTP default, docker-compose.yml, .dockerignore
- `delete_branch` – delete a branch after merge (write tool)
- `get_mr_pipelines` – list all pipelines for a specific MR
- `get_user` – user info lookup by username or ID
- Group-scoped issue search via `group_id` parameter in `search_issues`
- TTL-based response cache (60s) for user/project lookups
- HTTP 429 rate limit retry with Retry-After header (up to 3 attempts)
- Generic `get_all_pages()` pagination helper
- 18 new unit tests (31 total)
- Extracted `params.rs` from `server.rs` (1400 → 717 + 697 lines)

### Fixed
- Lint regex pre-compiled once via LazyLock (was recompiling per line per rule)
- UTF-8 safe string truncation (was panicking on Cyrillic)
- Mutex poison recovery instead of panicking
- Concurrent HTTP for project name resolution and team activity
- NaN-safe float sorting in MR turnaround stats
- Token redacted from Debug output on GitLabInstance
- Broader Sentry token scrubbing pattern

## [0.8.0] - 2026-04-04

### Added
- `create_merge_request` - smart MR creation with auto-title from branch name, auto-description from commits, duplicate detection, reviewer/assignee resolution
- Sentry error tracking (optional, via `SENTRY_DSN` env var) with tool call breadcrumbs and PII scrubbing
- `summary_only` parameter for `list_merge_requests` and `list_commits` (~3-5x smaller responses)
- Global response size warning (>15KB) in `tool_call!` macro — all 54 tools now auto-warn
- GitHub Copilot, Cursor, and Windsurf setup instructions in README
- GitHub Actions release workflow — pre-built binaries for macOS (ARM + Intel) and Linux on tag push
- Pre-built binary install instructions in README

### Changed
- `GitLabClient::new` returns `Result` instead of panicking on invalid tokens
- Reports capped at 50 commits / 10 projects to prevent unbounded responses

### Fixed
- Regex in `create_merge_request` title parser now compiled once via `LazyLock`

### Removed
- Dead code: `extract_safe_params`, `hash_params`, `SAFE_PARAMS`, `deserialize_opt_u64`, `default_client`, `language_to_rule_file`
- `sha2` dependency (no longer needed)

## [0.7.0] - 2026-04-01

### Added
- `get_mr_categories` - classify MRs by branch convention (feature/hotfix/bugfix/chore/docs/test/ci)
- `get_mr_timeline` - decompose merge time into queue vs review phases
- `get_cross_instance_dashboard` - aggregate MR stats across multiple GitLab instances

## [0.6.0] - 2026-04-01

### Added
- `get_mr_review_depth` - comments/discussions per MR, zero-review detection
- `get_org_mr_dashboard` - cross-group MR aggregation with reviewer load
- `get_deploy_frequency` - DORA deployment frequency metric by environment and deployer
- `get_stale_branches` - merged-but-not-deleted and inactive branch detection
- `merged_by` tracking in `get_mr_turnaround` - shows who merges MRs

## [0.5.0] - 2026-04-01

### Added
- `get_mr_turnaround` - avg/median merge time, per-author breakdown, slowest MRs
- `get_mr_dashboard` - compact group overview with reviewer bottlenecks and stale MRs
- `list_environments` - deployment tracking with last deploy SHA/branch/status
- `get_contributors` - all-time contributor stats per person (commits, LOC)
- `get_approval_rules` - project-level MR approval configuration

## [0.4.0] - 2026-04-01

### Added
- `get_group_activity` - auto-discover group members and aggregate activity
- Pipeline status (`[CI: success/failed]`) in MR list output
- Reviewers shown in MR list output
- `opened_before` param for finding stale MRs
- `group_id` param for cross-group MR queries
- Cross-instance team support (`instance` field on TeamMember)
- `save_team` / `list_teams` - team config stored in `~/.gl-mcp/teams.json`
- `get_team_activity` - multiple users in one call

### Changed
- Lint engine: reduced noise with file skip list and violation cap (3 per rule per file)

## [0.3.0] - 2026-03-31

### Added
- Rule-based commit validation - regex matching on diffs, zero LLM tokens
- `validate_commit`, `validate_mr`, `list_rules` tools
- Rules for PHP (14), Kotlin (9), Swift (13), Go (8), TypeScript (8), Ansible (7), global (5)
- Commented-out code detection rules
- `update_file` - create/update files with branch protection and auto-MR

## [0.2.0] - 2026-03-31

### Added
- `search_code` - search code across project with regex
- `get_languages` - project language breakdown with visual bars
- `get_tree` - repository directory listing
- `compare_branches` - compare two refs with commit and file lists
- `list_tags` - tags/releases with commit info
- `get_mr_approvals` - MR approval status
- `get_user_activity` - developer daily activity across all projects
- `get_commit_diff` `summary_only` param (~10x token savings)
- `GITLAB_COMPACT` mode - strip markdown from all responses
- Smart diff filtering: skip lockfiles, group by language, truncate
- Language detection for 25+ languages/frameworks
- Client-side Cyrillic author name matching
- Merge commit detection (>20 commits in single push)

### Changed
- All compiler warnings fixed

## [0.1.0] - 2026-03-31

### Added
- Initial release: 16 core tools
- Projects: `list_projects`, `get_project`, `list_members`, `list_branches`
- Issues: `search_issues`, `get_issue`, `create_issue`, `update_issue`, `add_note`
- Merge requests: `list_merge_requests`, `get_merge_request`
- Pipelines: `list_pipelines`, `get_pipeline`, `get_job_log`, `retry_pipeline`, `cancel_pipeline`
- Commits: `list_commits`, `get_commit_diff`, `get_mr_changes`, `get_file_content`
- Multi-instance support with domain auto-detection
- Read-only mode (`GITLAB_READ_ONLY`)
- Tool filtering (`DISABLED_TOOLS`)
- Analytics logging to `~/.gl-mcp/analytics.log`
- CI/CD workflow for Linux and macOS builds
- Docker multi-stage build
