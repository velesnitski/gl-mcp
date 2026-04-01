# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

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
