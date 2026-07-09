//! Tool registration and filtering.
//!
//! Mirrors yt-mcp: WRITE_TOOLS frozenset, read-only mode, disabled tools.

pub mod projects;
pub mod issues;
pub mod merge_requests;
pub mod pipelines;
pub mod commits;
pub mod reports;
pub mod repository;
pub mod lint;
pub mod adoption;
pub mod spec;
pub mod users;
pub mod docs;

/// Tools that modify data — blocked in read-only mode.
pub const WRITE_TOOLS: &[&str] = &[
    "create_issue",
    "update_issue",
    "add_note",
    "create_merge_request",
    "merge_mr",
    "rebase_mr",
    "close_mr",
    "update_merge_request",
    "create_branch",
    "create_label",
    "retry_pipeline",
    "cancel_pipeline",
    "save_team",
    "update_file",
    "delete_branch",
    "update_branch_protection",
    "revert_commit",
    "create_project",
    "transfer_project",
    "delete_project",
    "add_member",
    "add_group_member",
    "set_ci_variable",
    "update_ci_variable",
    "delete_ci_variable",
    "create_deploy_token",
];

/// The "core" toolset: the ~30 everyday dev-workflow tools (navigate, read,
/// search, issues, MRs, commits, CI, basic writes). Selected from real usage
/// analytics; excludes org analytics, HTML reports, adoption/spec auditing,
/// and admin (CI variables, deploy tokens, branch protection, project
/// lifecycle, membership). Exposed via GITLAB_TOOLSET=core to cut the
/// tools/list schema payload ~70% for clients that load all schemas up front.
pub const CORE_TOOLS: &[&str] = &[
    // Navigate & read
    "list_projects",
    "get_project",
    "list_group_projects",
    "get_tree",
    "get_file_content",
    "search_code",
    "list_branches",
    "list_members",
    // Issues
    "search_issues",
    "get_issue",
    "create_issue",
    "update_issue",
    "add_note",
    // Merge requests
    "list_merge_requests",
    "get_merge_request",
    "get_mr_changes",
    "get_mr_discussions",
    "create_merge_request",
    "update_merge_request",
    "merge_mr",
    "close_mr",
    // Commits & diffs
    "list_commits",
    "get_commit_diff",
    "compare_branches",
    // CI
    "list_pipelines",
    "get_pipeline",
    "get_job_log",
    "retry_pipeline",
    // Repo writes
    "update_file",
    "create_branch",
    "delete_branch",
    // Users
    "get_user",
    "search_users",
];

/// Resolve a GITLAB_TOOLSET value to an allowlist. `None` = expose everything.
/// "full" (or empty) → None; "core" → CORE_TOOLS; anything else is treated as
/// an explicit comma-separated tool list.
pub fn toolset_allowlist(toolset: &str) -> Option<Vec<String>> {
    match toolset.trim().to_lowercase().as_str() {
        "" | "full" => None,
        "core" => Some(CORE_TOOLS.iter().map(|s| s.to_string()).collect()),
        custom => Some(
            custom
                .split(',')
                .map(|s| s.trim().to_lowercase().replace('-', "_"))
                .filter(|s| !s.is_empty())
                .collect(),
        ),
    }
}

/// Check if a tool should be available given config.
pub fn is_tool_enabled(name: &str, read_only: bool, disabled: &[String]) -> bool {
    if read_only && WRITE_TOOLS.contains(&name) {
        return false;
    }
    if disabled.iter().any(|d| d == name) {
        return false;
    }
    true
}
