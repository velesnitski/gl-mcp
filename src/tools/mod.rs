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

/// Tools that modify data — blocked in read-only mode.
pub const WRITE_TOOLS: &[&str] = &[
    "create_issue",
    "update_issue",
    "add_note",
    "create_merge_request",
    "retry_pipeline",
    "cancel_pipeline",
    "save_team",
    "update_file",
];

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
