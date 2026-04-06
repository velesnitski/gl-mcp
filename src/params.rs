//! MCP tool parameter structs.
//!
//! Each struct maps to one tool's input schema (JSON → Rust via serde + schemars).

// Flexible number deserializer: accepts both "20" and 20 from JSON.
// MCP clients sometimes send numbers as strings.
mod flex {
    use serde::{self, Deserialize, Deserializer};
    use std::str::FromStr;

    fn deserialize_opt_num<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
    where
        D: Deserializer<'de>,
        T: FromStr + Deserialize<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNum<T> {
            Str(String),
            Num(T),
        }
        let v = Option::<StringOrNum<T>>::deserialize(deserializer)?;
        Ok(match v {
            Some(StringOrNum::Num(n)) => Some(n),
            Some(StringOrNum::Str(s)) => s.parse().ok(),
            None => None,
        })
    }

    pub fn deserialize_opt_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
    where D: Deserializer<'de> {
        deserialize_opt_num(deserializer)
    }

    pub fn deserialize_opt_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
    where D: Deserializer<'de> {
        deserialize_opt_num(deserializer)
    }
}

use serde::Deserialize;
use schemars::JsonSchema;

// ─── Projects ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListProjectsParams {
    #[schemars(description = "Search query to filter projects")]
    pub search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProjectParams {
    #[schemars(description = "Project ID or URL-encoded path (e.g., 'my-group/my-project')")]
    pub project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMembersParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListGroupProjectsParams {
    #[schemars(description = "Group path (e.g., 'example-org/software')")]
    pub group_path: String,
    #[schemars(description = "Max results (default: 50)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListBranchesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Search branch name")]
    pub search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetStaleBranchesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Days of inactivity to consider stale (default: 30)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub inactive_days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeleteBranchParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Branch name to delete")]
    pub branch: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetUserParams {
    #[schemars(description = "GitLab username (e.g., 'john.doe')")]
    pub username: Option<String>,
    #[schemars(description = "GitLab user ID (numeric)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub user_id: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Issues ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchIssuesParams {
    #[schemars(description = "Project ID or path (empty = search all projects)")]
    pub project_id: Option<String>,
    #[schemars(description = "Group ID or path to search issues within a group (e.g., 'my-org/backend')")]
    pub group_id: Option<String>,
    #[schemars(description = "Search text in title/description")]
    pub search: Option<String>,
    #[schemars(description = "Filter by state: opened, closed, all (default: opened)")]
    pub state: Option<String>,
    #[schemars(description = "Comma-separated label names")]
    pub labels: Option<String>,
    #[schemars(description = "Filter by assignee username")]
    pub assignee: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetIssueParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Issue IID (the # number)")]
    pub issue_iid: u64,
    #[schemars(description = "Include comments (default: true)")]
    pub include_notes: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateIssueParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Issue title")]
    pub title: String,
    #[schemars(description = "Issue description (markdown)")]
    pub description: Option<String>,
    #[schemars(description = "Comma-separated label names")]
    pub labels: Option<String>,
    #[schemars(description = "Assignee username")]
    pub assignee: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateIssueParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Issue IID")]
    pub issue_iid: u64,
    #[schemars(description = "New title (empty = keep)")]
    pub title: Option<String>,
    #[schemars(description = "New description (empty = keep)")]
    pub description: Option<String>,
    #[schemars(description = "New state: close or reopen")]
    pub state_event: Option<String>,
    #[schemars(description = "New labels (comma-separated, replaces existing)")]
    pub labels: Option<String>,
    #[schemars(description = "Assignee username (empty = unassign)")]
    pub assignee: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddNoteParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Issue or MR IID")]
    pub iid: u64,
    #[schemars(description = "Note type: 'issue' or 'mr' (default: 'issue')")]
    pub note_type: Option<String>,
    #[schemars(description = "Comment text (markdown)")]
    pub body: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Merge Requests ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMergeRequestsParams {
    #[schemars(description = "Project ID or path (empty = all projects)")]
    pub project_id: Option<String>,
    #[schemars(description = "Group path to list MRs across all group projects (e.g., 'my-org/backend')")]
    pub group_id: Option<String>,
    #[schemars(description = "Filter by state: opened, closed, merged, all (default: opened)")]
    pub state: Option<String>,
    #[schemars(description = "Filter by author username")]
    pub author: Option<String>,
    #[schemars(description = "Scope: assigned_to_me, created_by_me, all (default: all)")]
    pub scope: Option<String>,
    #[schemars(description = "Only MRs created after this date (ISO, e.g., '2026-03-01')")]
    pub created_after: Option<String>,
    #[schemars(description = "Only MRs created before this date (ISO). Use to find stale/old MRs.")]
    pub opened_before: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "Return compact one-line-per-MR summary (~5x smaller). Use first to scan, then get_merge_request to drill in.")]
    pub summary_only: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateMergeRequestParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Source branch (your feature branch)")]
    pub source_branch: String,
    #[schemars(description = "Target branch (default: project default branch)")]
    pub target_branch: Option<String>,
    #[schemars(description = "MR title (default: auto-generated from branch name, e.g., 'feature/PROJ-123-add-auth' → 'PROJ-123: Add auth')")]
    pub title: Option<String>,
    #[schemars(description = "MR description in markdown (default: auto-generated commit list)")]
    pub description: Option<String>,
    #[schemars(description = "Comma-separated labels")]
    pub labels: Option<String>,
    #[schemars(description = "Assignee username")]
    pub assignee: Option<String>,
    #[schemars(description = "Comma-separated reviewer usernames")]
    pub reviewers: Option<String>,
    #[schemars(description = "Squash commits on merge (default: true)")]
    pub squash: Option<bool>,
    #[schemars(description = "Delete source branch after merge (default: true)")]
    pub remove_source_branch: Option<bool>,
    #[schemars(description = "Create as draft MR (default: false)")]
    pub draft: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMergeRequestParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Merge request IID (the ! number)")]
    pub mr_iid: u64,
    #[schemars(description = "Include comments (default: true)")]
    pub include_notes: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrTurnaroundParams {
    #[schemars(description = "Project ID or path (optional if group_id set)")]
    pub project_id: Option<String>,
    #[schemars(description = "Group path for cross-project stats (e.g., 'my-org/backend')")]
    pub group_id: Option<String>,
    #[schemars(description = "Number of days to analyze (default: 7)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrDashboardParams {
    #[schemars(description = "Group path (e.g., 'my-org/backend')")]
    pub group_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrReviewDepthParams {
    #[schemars(description = "Project ID or path (optional if group_id set)")]
    pub project_id: Option<String>,
    #[schemars(description = "Group path for cross-project stats")]
    pub group_id: Option<String>,
    #[schemars(description = "Number of days to analyze (default: 7)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetOrgMrDashboardParams {
    #[schemars(description = "Comma-separated group paths (e.g., 'my-org/backend,my-org/frontend,my-org/infrastructure')")]
    pub groups: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrCategoriesParams {
    #[schemars(description = "Project ID or path (optional if group_id set)")]
    pub project_id: Option<String>,
    #[schemars(description = "Group path for cross-project stats")]
    pub group_id: Option<String>,
    #[schemars(description = "Filter by state: opened, merged, closed, all (default: all)")]
    pub state: Option<String>,
    #[schemars(description = "Number of days to analyze (default: 7)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrTimelineParams {
    #[schemars(description = "Project ID or path (optional if group_id set)")]
    pub project_id: Option<String>,
    #[schemars(description = "Group path for cross-project stats")]
    pub group_id: Option<String>,
    #[schemars(description = "Number of days to analyze (default: 7)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCrossInstanceDashboardParams {
    #[schemars(description = "Comma-separated 'instance:group' pairs (e.g., 'staging:my-org/backend,production:my-org/frontend'). Groups within same instance are batched.")]
    pub targets: String,
    #[schemars(description = "Default instance if not specified per-group")]
    pub instance: Option<String>,
}

// ─── Pipelines ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPipelinesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Filter by status: running, pending, success, failed, canceled")]
    pub status: Option<String>,
    #[schemars(description = "Filter by git ref (branch/tag)")]
    pub ref_name: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPipelineParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Pipeline ID")]
    pub pipeline_id: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetJobLogParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Job ID")]
    pub job_id: u64,
    #[schemars(description = "Max lines from end of log (default: 100)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_usize")]
    pub tail: Option<usize>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RetryPipelineParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Pipeline ID")]
    pub pipeline_id: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrPipelinesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Merge request IID (the ! number)")]
    pub mr_iid: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelPipelineParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Pipeline ID")]
    pub pipeline_id: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Commits & Diffs ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListCommitsParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Branch or tag name (empty = default branch)")]
    pub branch: Option<String>,
    #[schemars(description = "Filter by author name or email")]
    pub author: Option<String>,
    #[schemars(description = "ISO date: commits after this date (e.g., '2026-03-24T00:00:00Z')")]
    pub since: Option<String>,
    #[schemars(description = "ISO date: commits before this date")]
    pub until: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "Return compact one-line-per-commit summary (~3x smaller). Use first to scan, then get_commit_diff to drill in.")]
    pub summary_only: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCommitDiffParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Commit SHA (full or short)")]
    pub sha: String,
    #[schemars(description = "Max diff lines per file (default: 200)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_usize")]
    pub max_lines_per_file: Option<usize>,
    #[schemars(description = "Skip lockfiles and generated code (default: true)")]
    pub skip_generated: Option<bool>,
    #[schemars(description = "Return only file list + stats, no diff content. ~10x smaller response.")]
    pub summary_only: Option<bool>,
    #[schemars(description = "Only show diff for files matching this path substring")]
    pub file: Option<String>,
    #[schemars(description = "Strip markdown formatting for smaller responses")]
    pub compact: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrChangesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Merge request IID")]
    pub mr_iid: u64,
    #[schemars(description = "Max diff lines per file (default: 200)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_usize")]
    pub max_lines_per_file: Option<usize>,
    #[schemars(description = "Skip lockfiles and generated code (default: true)")]
    pub skip_generated: Option<bool>,
    #[schemars(description = "Return only file list + stats, no diff content")]
    pub summary_only: Option<bool>,
    #[schemars(description = "Only show diff for files matching this path substring")]
    pub file: Option<String>,
    #[schemars(description = "Strip markdown formatting for smaller responses")]
    pub compact: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetFileContentParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "File path within the repository")]
    pub file_path: String,
    #[schemars(description = "Branch, tag, or commit SHA (default: default branch)")]
    pub ref_name: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetUserActivityParams {
    #[schemars(description = "GitLab username")]
    pub username: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week' (since Monday), '3d', or hours as number (default: 24)")]
    pub period: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTeamActivityParams {
    #[schemars(description = "Team key from teams.json (e.g., 'devops', 'backend') OR comma-separated usernames")]
    pub team: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week', '3d', or hours (default: 24)")]
    pub period: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetGroupActivityParams {
    #[schemars(description = "Group path (e.g., 'my-org/backend')")]
    pub group_path: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week', '3d', or hours (default: 24)")]
    pub period: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Teams ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTeamsParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveTeamParams {
    #[schemars(description = "Team key (e.g., 'devops')")]
    pub key: String,
    #[schemars(description = "Team display name")]
    pub name: String,
    #[schemars(description = "Comma-separated GitLab usernames")]
    pub usernames: String,
    #[schemars(description = "Comma-separated project paths (optional)")]
    pub projects: Option<String>,
    #[schemars(description = "Comma-separated instance names per user (optional, matches usernames order)")]
    pub instances: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GenerateDevReportParams {
    #[schemars(description = "GitLab username")]
    pub username: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week', '3d', or hours (default: today)")]
    pub period: Option<String>,
    #[schemars(description = "Filter to specific project path substring (optional)")]
    pub project: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Repository ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Search query (regex supported)")]
    pub query: String,
    #[schemars(description = "Branch/tag to search in (optional)")]
    pub ref_name: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetLanguagesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTreeParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Directory path (empty = root)")]
    pub path: Option<String>,
    #[schemars(description = "Branch/tag/SHA (optional)")]
    pub ref_name: Option<String>,
    #[schemars(description = "Include subdirectories recursively (default: false)")]
    pub recursive: Option<bool>,
    #[schemars(description = "Max results (default: 100)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareBranchesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Base branch/tag/SHA")]
    pub from: String,
    #[schemars(description = "Head branch/tag/SHA")]
    pub to: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTagsParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Search tag name")]
    pub search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrApprovalsParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Merge request IID")]
    pub mr_iid: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateFileParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "File path in the repository (e.g., 'README.md')")]
    pub file_path: String,
    #[schemars(description = "File content")]
    pub content: String,
    #[schemars(description = "Branch name to commit to (must NOT be main/master/develop)")]
    pub branch: String,
    #[schemars(description = "Commit message")]
    pub commit_message: String,
    #[schemars(description = "Source branch to create from (default: main)")]
    pub source_branch: Option<String>,
    #[schemars(description = "Create a merge request after commit (default: true)")]
    pub create_mr: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListEnvironmentsParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetContributorsParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetApprovalRulesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetDeployFrequencyParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Filter by environment name (e.g., 'production')")]
    pub environment: Option<String>,
    #[schemars(description = "Number of days to analyze (default: 30)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Developer Comparison ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareDevelopersParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Comma-separated GitLab usernames to compare")]
    pub usernames: String,
    #[schemars(description = "Number of days to analyze (default: 14)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Team Report ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GenerateTeamReportParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Comma-separated GitLab usernames")]
    pub usernames: String,
    #[schemars(description = "Number of days to analyze (default: 14)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    pub days: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

// ─── Lint ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateCommitParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Commit SHA")]
    pub sha: String,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateMrParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Merge request IID")]
    pub mr_iid: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateMrChangesParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "Merge request IID")]
    pub mr_iid: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AnalyzeFileParams {
    #[schemars(description = "Project ID or path")]
    pub project_id: String,
    #[schemars(description = "File path in the repository (e.g., 'app/Services/UserService.php')")]
    pub file_path: String,
    #[schemars(description = "Branch, tag, or SHA (default: HEAD)")]
    pub ref_name: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    pub instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRulesParams {
    #[schemars(description = "Language filter: PHP, Kotlin, TypeScript, Ansible (empty = all)")]
    pub language: Option<String>,
}
