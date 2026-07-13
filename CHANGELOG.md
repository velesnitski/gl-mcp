# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [1.1.1] - 2026-07-10

### Fixed
- **`get_merge_request` reported a bogus `mergeable` for merge requests in archived projects.** GitLab keeps returning a normal `detailed_merge_status` (e.g. `mergeable`) even when the project is **archived** and therefore read-only — any merge, push, or comment is rejected. Acting on that status silently wastes work (we chased four "ready to merge" MRs, two of which were in archived repos and could never land). The MR API never exposes the project's archived flag, so `get_merge_request` now looks the project up — **only while the MR is open** (where the lie is harmful) and **cached** (60s, so repeated calls on one project don't refetch) — and replaces the status with an explicit `⚠️ BLOCKED — project is ARCHIVED` line that still shows what GitLab claimed.
- `get_project` now surfaces **archived** state with a prominent warning. It was previously invisible, despite invalidating every write operation.

See ADR 033.

## [1.1.0] - 2026-07-09

### Added
- `audit_readmes` — scan a group (with subgroups) and classify each project's README as **missing**, **small/stub** (`< small_bytes`, default 300), or **Russian/Cyrillic** (alphabetic chars ≥ `cyrillic_pct`% Cyrillic, default 20), else ok. Fetches and classifies **server-side** (two calls per repo: root tree + raw README), so the caller gets a compact table + counts instead of N READMEs — the same shape as `get_ai_adoption`. Archived projects skipped; `include_ok` optionally lists healthy repos. First minor release under the 1.0 stability contract (additive). See ADR 032. 100 tools total.

## [1.0.1] - 2026-07-09

### Security (hardening — not a vulnerability fix)
- Reviewed gl-mcp against yt-mcp advisory **GHSA-99mq-fjjc-6v9j** (path traversal in a `file_path` tool, CWE-22/73). gl-mcp is **not affected**: no tool reads a caller-controlled *local* file (`update_file` takes content directly; `get_file_content`/`analyze_file` operate on the *remote* GitLab repo via URL-encoded API paths), so the read-and-exfiltrate class does not exist here. The only local write from caller input — spec-audit snapshot filenames (`~/.gl-mcp/spec_maps/…`) — was already traversal-safe (separators were replaced). Hardened it anyway from a denylist to an **allowlist by construction** (`[A-Za-z0-9._-]`, leading dots stripped), so a snapshot can never escape `spec_maps/` regardless of `project`/`ref`/`key` input. Pinned by `snapshot_path_is_traversal_proof` + `safe_component_allowlist` tests. Note: the filename scheme changed slightly, so the first spec audit after upgrade re-baselines its snapshot. See ADR 031.

## [1.0.0] - 2026-07-03

**Stability milestone.** The tool surface (99 tools: names, parameters, output shapes) is now a semver contract: additive changes bump minor, breaking changes bump major with a deprecation note. This release caps the v0.28→v0.38 hardening arc — architecture pass (one retry core, typed error classification, shared user resolution), fence-safe compact mode, Sentry hygiene, honest adoption metrics, project/member lifecycle, CI debugging, and toolset profiles — with all known defects fixed and the task backlog empty.

### Added
- `GITLAB_TOOLSET` — toolset profiles for schema-token savings. `full` (default), `core` (~33 everyday dev tools: navigate/read/search, issues, MRs, commits, CI, basic writes — selected from usage analytics), or an explicit comma-separated tool list. Pruning happens **at router construction**, so filtered tools are absent from `tools/list` itself: the measured ~77 KB (~20k-token) full schema payload drops ~70% under `core`. Matters for MCP clients that load all schemas up front (Claude Code defers schemas, so it was already unaffected). Unknown names in a custom list are warned about at startup.

### Changed
- `DISABLED_TOOLS` and `GITLAB_READ_ONLY` now also prune the affected tools from `tools/list` (previously they were listed but rejected at call time). The model no longer sees tools it cannot call, and their schemas cost nothing. Call-time guards remain as defense in depth.

See ADR 030. 99 tools total (`full`); `core` exposes 33.

## [0.37.0] - 2026-07-03

### Changed (internal refactor — no tool-surface changes)
- **One retry core.** All six HTTP verbs (`get`, `get_text`, `get_cached`, `post`, `put`, `delete`) now funnel through a single `send_with_retry`; the 429/Retry-After loop previously hand-copied into each verb lives in exactly one place. (This duplication is what originally broke `get_job_log` — adding a verb meant re-copying the loop.)
- **Typed user-error classification.** New `Error::UserInput` variant + `Error::is_user_error()` (UserInput/NotFound/GitLab-4xx-except-408/429). `tool_call!` reports such failures with status `user_error`, which Sentry never captures — the ADR 026 string heuristic is demoted to a fallback for string-only error paths. Analytics now distinguishes `user_error` from `error`.
- **One user-resolution implementation.** New `tools/users.rs` with `resolve_user_id` (hard — errors on unknown user; member grants) and `lookup_user_id` (soft — `Ok(None)`; assignee/reviewer enrichment), plus access-level parsing moved from `projects.rs`. Replaces 5 scattered inline implementations across issues/merge_requests/projects.

See ADR 029. Behavior-preserving: 175 tests pass; tool surface unchanged (99 tools).

## [0.36.0] - 2026-07-03

### Changed
- `generate_ai_adoption_report` (HTML) mirrors the v0.35.0 active/configured split: a new **AI-Active** summary card (config markers OR usage evidence — trailed commits on any branch incl. squash-hidden, `.tasks/` activity, AI-marked MRs) alongside the marker-based card (renamed **Configured (L1+)**), and an **AI-Active** column in the By-Team table (highlighted green when it exceeds Configured — real adoption the config number misses). Methodology section documents both axes. Leadership slides built from the report no longer inherit the undercount.

### Added
- `get_group_activity` gains `include_commit_messages` (default false): lists head-commit titles per push (`branch: title`, cap 10/member) under each member, straight from the events payload — no extra API calls. Enables text correlation (e.g. issue IDs in commit messages).

See ADR 028. 99 tools total.

## [0.35.0] - 2026-07-03

### Changed
- `get_ai_adoption` no longer undercounts real usage in the per-team roll-up. The team table now has an **Active** column (config markers **OR** any usage evidence — AI-trailed commits on any branch incl. squash-hidden feature branches, `.tasks/` commits, or AI-marked MRs) next to the marker-based **Configured** column. Previously a team could read "0/6 adopting" while a repo had 100% AI-trailed commits, because usage-without-config landed only in a side "Invisible usage" section and was excluded from the headline. The `summary_only` line is now `active/repos · configured (best L…)`. "Best level" stays marker-based (config quality is a separate axis). See ADR 027.

## [0.34.1] - 2026-07-02

### Fixed
- Expected user-input errors no longer create Sentry issues (false alerts). The Sentry filter matched **exact** error strings, so variably-worded validation messages slipped through — e.g. `add_member`'s "User '@x' not found" didn't match the literal `"User not found"` in the allowlist, and lowercase "has no id" didn't match "User has no ID". `is_actionable_error` now matches user-error **phrases case-insensitively** (`not found`, `not accessible`, `unknown access level`, `has no id`, `refusing to delete`, `nothing to update`, …), so validation errors from the member/project lifecycle tools stay out of Sentry while real errors (5xx, HTTP, JSON) still page. See ADR 026.

## [0.34.0] - 2026-07-01

### Added
- `add_group_member` — add a member to a **group** (`POST /groups/:id/members`), granting access to **all projects in the group** (vs `add_member` which is project-scoped). `group_id` accepts a full path (e.g. `my-org/devops`) or numeric id; `user` (username or id) and `access_level` (role name or number) are resolved the same way as `add_member`, plus optional `expires_at`. Write-guarded.

See ADR 025. 99 tools total.

## [0.33.0] - 2026-07-01

### Added
- `add_member` — add a member to a project (`POST /projects/:id/members`). `user` accepts a **username** (leading `@` optional) or numeric id — resolved automatically, so no user-id lookup — and `access_level` accepts a **role name** (guest/reporter/developer/maintainer/owner) or number. Optional `expires_at` (YYYY-MM-DD). Write-guarded.

See ADR 024. 98 tools total.

## [0.32.0] - 2026-07-01

### Added
- `transfer_project` — move a project to a different namespace/group (`PUT /projects/:id/transfer`). `namespace` accepts a **full group path** (e.g. `my-org/devops`) or a numeric id; it's resolved and validated before the move, so you never need to look up a subgroup's numeric id. Non-destructive.
- `delete_project` — delete a project (`DELETE /projects/:id`), **guarded** by a required `confirm_full_path` that must exactly match the project's `path_with_namespace`, so a mistyped id can't delete the wrong project.
- `create_project` now accepts a `namespace` param (full path or id), resolved/validated — preventing the "landed one namespace too high" mistake that a bare numeric `namespace_id` invites. `namespace_id` is still honored for back-compat.

Both new tools are write-guarded. See ADR 023. 97 tools total.

## [0.31.0] - 2026-06-30

### Added
- `get_pipeline` now shows each job's **numeric id** (`(job <id>)`) and the GitLab **`failure_reason`** for failed jobs, so you can go straight from a pipeline to `get_job_log` without scraping the id out-of-band.

### Changed
- `get_job_log` strips **ANSI escape codes** and carriage returns from the trace before tailing it — readable, token-efficient logs instead of `\x1b[0K\x1b[32;1m…` noise. The job header also surfaces `failure_reason`.

See ADR 022. 95 tools total.

## [0.30.0] - 2026-06-29

### Fixed
- `get_job_log` was completely broken — it fetched the CI job **trace** with `client.get::<String>()`, which tries to JSON-parse the body, but the trace endpoint returns **plain text**, so every call failed with `JSON parse error: expected value at line 1 column 1`. Added `client.get_text()` (raw body, 429-retry) and switched the trace fetch to it; empty traces now return a clear note.
- Compact mode (`GITLAB_COMPACT=1`) no longer corrupts file contents. `strip_markdown` is now **code-fence-aware**: content inside ```` ``` ````/`~~~` fences (file reads via `get_file_content`, job logs, diffs) is passed through byte-for-byte, while prose in reports/dashboards is still stripped for token savings. Previously it removed `#`/`**` from *all* output, e.g. turning a commented `# RUN apt-get …` Dockerfile line into an apparent active instruction and flattening markdown headings.

See ADR 021. 95 tools total.

## [0.29.0] - 2026-06-29

### Added
- `update_merge_request` — edit an open MR's title, description, labels, or target branch (`PUT /merge_requests/:iid`). Only the fields you pass are changed; a no-op (all fields empty) is refused rather than issuing an empty update. Closes the MR lifecycle gap: create → **update** → merge/close.
- `create_branch` — create a branch from any source ref (branch/tag/SHA, default `main`). Makes branch lifecycle symmetric with `delete_branch`, and pairs with the now-stackable `update_file` for scripted multi-file branches.

Both are write-guarded (blocked in read-only mode). See ADR 020.

95 tools total.

## [0.28.0] - 2026-06-29

### Fixed
- `update_file` can now stack multiple files onto one feature branch. It only sends `start_branch` when creating the branch (GitLab rejected a second commit to an existing branch with "branch already exists"), and checks create-vs-update against the target branch rather than the source — so a README rewrite plus several ADR files land on one branch for a single MR.

93 tools total.

## [0.27.0] - 2026-06-26

### Added
- `--version` / `-V` flag — prints the bare version and exits without starting the server or requiring config.
- `Makefile` + `scripts/sync-mcp-label.py` — `make install` (build + sync) re-keys the gl-mcp entry in the MCP config to `gl-mcp v<version>` so Claude Code's `/mcp` dialog shows the running version (the dialog labels by config key, not the reported server name). Idempotent, keeps a `.bak`. Mirrors slk-mcp's fleet pattern.

### Fixed
- Corrected the advertised tool count to the actual **93** (`Cargo.toml`/`CLAUDE.md` had drifted to "98").
- `list_group_projects` now paginates (via `get_all_pages`) instead of a single request — groups with more than 100 projects were silently truncated (GitLab caps `per_page` at 100) despite "list all projects". `per_page` is honored as a max-results cap (default raised 50 → 200, hard ceiling 2000); the header flags truncation.

93 tools total.

## [0.26.0] - 2026-06-15

### Changed
- Build profiles tuned for a frequently-rebuilt tool: release uses `lto = "thin"` (faster link, binary 9.8→10 MB) and dev/test uses `debug = "line-tables-only"` (readable backtraces, ~24% smaller `target/debug`). No runtime behavior change.

98 tools total.

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
