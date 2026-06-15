# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- Build profiles tuned for a frequently-rebuilt tool: release uses `lto = "thin"` (faster link, binary 9.8→10 MB) and dev/test uses `debug = "line-tables-only"` (readable backtraces, ~24% smaller `target/debug`). No runtime behavior change.

## [0.25.0] - 2026-06-15

### Changed
- `audit_spec_drift` / `generate_spec_audit_report` / `sweep_spec_audit` / `generate_sweep_report` `routes_file` now accepts a single file, a comma-separated list, **or a directory** (expanded recursively to every code file under it — e.g. `routes/api` audits a Laravel backend's whole route surface in one pass). Non-source files are skipped, fetches run concurrently (capped at 60), and each undocumented endpoint still links to its own source file.

98 tools total.

## [0.24.0] - 2026-06-15

### Added
- `generate_sweep_report` — clickable HTML cross-team spec-drift report: summary cards (teams, stale versions, drift, undocumented, secrets), a By-Team table linking to per-team detail, a needs-attention list, and collapsible per-team sections (version, drift, stale-doc, undocumented endpoints with GitLab links, masked secrets). Audits all teams concurrently; failures become an error row.

98 tools total.

## [0.23.0] - 2026-06-15

### Fixed
- `audit_spec_drift` / `sweep_spec_audit` reverse-drift search-harvest now seeds single-quote route literals (`'/login'`) and call-style forms (`Route::get('/...')`), not just double-quote — so PHP/Laravel, Ruby, and Python repos are no longer silently missed when no `routes_file` is given.

97 tools total.

## [0.22.0] - 2026-06-15

### Added
- `sweep_spec_audit` — audit several specs against their repos concurrently (e.g. iOS/Android/Windows/Mac app-spec articles) and roll up into one cross-platform table: per-platform version/cleanup-debt/drift/stale-doc/undocumented/secrets, a needs-attention list, and totals. Platforms run in parallel (capped); a per-platform failure yields an error row instead of sinking the sweep. Search-harvested reverse-drift is marked `~` (namespace-gated lower bound) vs precise routes_file counts.

### Fixed
- Metadata-map snapshots are keyed by an optional discriminator so multiple specs audited against the same project+ref (e.g. Windows and macOS specs both targeting one desktop repo in a sweep) no longer clobber each other's "changes since last audit" history.

97 tools total.

## [0.21.0] - 2026-06-15

### Added
- `audit_spec_drift` — cross-reference a documented spec (e.g. a knowledge-base app-spec article, passed as markdown) against a project's code. Route drift: cleanup-debt (flagged for removal but still in code), stale-doc (flagged and already gone), drift (listed active but missing), needs-review (path too generic to match). Version drift: spec version vs latest git tag. gl-mcp stays GitLab-only — the caller supplies the spec text.
- `audit_spec_drift` security check — detects secret-shaped strings in the spec (base64 keys/tokens, UUIDs, credential emails) and code-searches each to flag doc-only leaks vs secrets also hardcoded in the repo. Reports masked previews only, never the raw value.
- `audit_spec_drift` local metadata map — persists a snapshot of each run to `~/.gl-mcp/spec_maps/` and shows a "Changes since last audit" diff (routes drifted/fixed, version verdict moved, secrets appeared/resolved, shadow endpoints appeared/resolved) on the next run for the same project+ref.
- `generate_spec_audit_report` — clickable HTML spec-drift report (dark theme, Export PDF): summary cards linking to sections, version banner, route-drift sections, undocumented-endpoints table with GitLab file links, masked security findings, and changes since the last audit. Shares the scan with `audit_spec_drift` via `compute_audit`.
- `audit_spec_drift` reverse drift — undocumented endpoints (in code, not in the spec). Pass `routes_file` (the file that defines the routes) for a precise inventory incl. new namespaces; otherwise harvested by search within documented namespaces. Matched by last-two-segments key, tolerant of prefix differences. Handles fragment-assembled routes: paths split across `+`-joined literals (`"/v3" + "/user"`) are stitched, and interpolated middle segments (`"/users/\(id)/posts"`) normalize cleanly.

### Fixed
- `audit_spec_drift` version compare now parses prefixed tags (`release-4.9.10`, `app-v2.3.0`) and compares numerically — previously only a leading `v` was stripped, so prefixed tags returned "could not compare".
- `audit_spec_drift` security check no longer mistakes a long slash-delimited route path for a base64 secret (the base64 class includes `/`); a candidate now needs `=`/`+` or no `/` to count.

96 tools total.

## [0.20.0] - 2026-06-12

### Added
- AI adoption reports name the people behind the usage — "Who" column (commit authors + AI tool parsed from the Co-Authored-By trailer) in Invisible usage, linked sample commit as evidence, top author named in usage-w/o-config and squash-hidden flags, author subline in the Adopting table. Zero extra API calls: extracted from commit data the scan already fetches.
- One-shot CLI mode: `gl-mcp --adoption-report GROUP [--days N] [--gl-instance NAME]` prints the AI-adoption HTML report to stdout and exits. Same engine as the `generate_ai_adoption_report` MCP tool — lets cron/CI consumers (e.g. a scheduled email-report workflow) reuse the exact scan without speaking MCP or duplicating the heuristics.

## [0.19.0] - 2026-06-11

### Added
- Fully clickable HTML adoption report — every repo, branch, and config marker links to its GitLab page (CLAUDE.md blob, `.claude` trees, ADR directory, branch trees, commit history, merged-MR filter)
- Aggregate numbers link to their evidence: summary cards, funnel counts, and By-Team cells jump to section anchors; `+N task commits` links to path-scoped commit history; anchors auto-expand collapsed sections via hashchange handler

94 tools total.

## [0.18.0] - 2026-06-11

### Added
- `generate_ai_adoption_report` — HTML adoption scorecard: summary cards, level funnel, per-team trajectories, in-flight pipeline, quality flags, collapsible methodology, Export PDF
- "Invisible usage" section in both report formats — repos with AI-trailed commits but zero config, sorted heaviest first with squash-hidden attribution status
- Dormant repo visibility: per-team Dormant column, collapsible archive-candidates list (oldest first), `dormant_days` parameter (default 180)

### Fixed
- Skills in the directory format (`skills/<name>/SKILL.md`) were counted as 0 — each subdirectory now counts as one skill

94 tools total.

## [0.17.0] - 2026-06-10

### Added
- `get_ai_adoption` — per-team Claude Code adoption scorecard: levels L0–L3, trajectory (↑→↓), quality flags (stale config, setup unused, no attribution, squash-hidden usage), in-flight branch radar, MR-description usage detection (survives squash), ADR cadence, config staleness, `.tasks`/`.claude` activity tracking
- Version shown in MCP server name — `/mcp` displays "gl-mcp v0.17.0"
- Mutation-resistant test suite for `mr_project_path` (11 input shapes)

### Fixed
- `list_branches` returned 400 on GitLab 17+ (invalid `sort`/`order_by` params); now uses `sort=updated_desc`
- Adoption branch radar sorts branches by recent activity — alphabetical default buried real hits below the 100-branch cutoff
- Browser user-agent / agency branch names no longer false-positive as AI work

93 tools total.

## [0.16.0] - 2026-05-19

### Added
- `list_commits` now accepts `all_branches: bool` parameter — maps to GitLab's `?all=true` query. Useful for catching feature-branch work that hasn't merged to default branch. Mutually exclusive with `branch` (branch wins if both set).
- `list_merge_requests` now accepts `include_descriptions: bool` parameter — when true with `summary_only`, includes MR description bodies (useful for parsing ticket IDs).

## [0.15.0] - 2026-05-12

### Added
- Cargo features `sentry` and `http` (both default on) — slim builds via `--no-default-features` are 7.1 MB vs 9.8 MB default
- `simple_tool!` macro collapses 86 of 92 tool body boilerplate
- `mr_project_path` helper using `url::Url` parsing — replaces fragile `references["full"]` split

### Changed
- `save_team` now properly respects `GITLAB_READ_ONLY` mode
- `analyze_project` reuses `compute_file_metrics` (removed 120 lines of duplication)
- `get_code_hotspots` and `get_group_activity` now fetch concurrently (10× latency improvement)
- Sentry events filtered: 4xx user errors (404, 401, 403) skipped, only 5xx and network errors captured

### Fixed
- Critical: `save_team` was missing `write_guard!` — read-only mode is now enforced
- `get_code_hotspots` no longer times out on busy repos (was sequential N HTTP calls)
- Commit message validation extracted to shared helper — no more drift between lint.rs and reports.rs

## [0.14.0] - 2026-05-04

### Added
- `create_project` — create new GitLab project with namespace/visibility/branch options (write)
- `set_ci_variable` / `update_ci_variable` / `delete_ci_variable` — full CRUD for CI/CD variables (write)
- `create_deploy_token` — create deploy token with custom scopes (write)
- `list_deploy_tokens` — list existing deploy tokens (metadata only)
- `summary_only` parameter on 7 more analytics tools: get_mr_discussions, get_project_events, get_reviewer_velocity, get_review_load, get_mr_size_trend, get_team_timezone, get_code_hotspots
- Sentry noise filter: skip 4xx user errors (404, 401, 403), keep 5xx and network errors
- 2 new unit tests for `is_actionable_error` filter

92 tools total.

## [0.13.0] - 2026-04-25

### Added
- `check_branch_protection` — view protected branch settings
- `update_branch_protection` — create/update protected branch rules (write)
- `get_commit_refs` — list branches/tags containing a commit
- `revert_commit` — create revert commit on target branch (write)
- `get_reviewer_velocity` — per-reviewer first-response time, sorted fastest first
- `get_review_load` — review distribution, bus factor warning when top reviewer >70%
- `get_mr_size_trend` — weekly MR size trends (files, LOC) with verdict
- `get_team_timezone` — UTC peak hour analysis with timezone heuristic and weekend %

86 tools total.

## [0.12.0] - 2026-04-17

### Added
- `search_users` — find users by name, username, or email
- `get_group_members` — all group members including inherited
- `merge_mr` — merge MR with squash/remove_branch options (write)
- `rebase_mr` — trigger MR rebase (write)
- `close_mr` — close a merge request (write)
- `get_mr_discussions` — threaded MR discussions with resolved status
- `get_project_events` — project activity feed with action filter
- `list_labels` — project labels with colors and issue counts
- `create_label` — create label with color/description (write)
- `get_milestones` — project milestones with state filter
- `get_ci_variables` — CI/CD variable keys and metadata (never values)
- `get_code_hotspots` — most frequently changed files across recent commits
- Windows binary in release workflow
- Redesigned README with badges, collapsible setup, highlights

### Fixed
- `data_dir()` falls back to USERPROFILE on Windows

## [0.11.0] - 2026-04-10

### Added
- Auto-observations in dev reports: self-merging, branch typos, test coverage %, weekend/off-hours work, ticket reference rate, no-reviewer flag
- `compare_developers` cross-project support (comma-separated project_ids)
- `generate_dev_report` multi-instance auto-discovery
- `summary_only` for compare_developers, get_mr_dashboard, get_mr_review_depth, get_mr_timeline, analyze_project
- `get_user_activity` auto-discovers all configured instances

### Fixed
- `generate_dev_report` pagination: fetches all commits via get_all_pages (was first 20 only)

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
