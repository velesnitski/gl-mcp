//! MCP server handler — wires GitLab tools to rmcp.

// Flexible number deserializer: accepts both "20" and 20 from JSON.
// MCP clients sometimes send numbers as strings.
mod flex {
    use serde::{self, Deserialize, Deserializer};

    pub fn deserialize_opt_u32<'de, D>(deserializer: D) -> Result<Option<u32>, D::Error>
    where D: Deserializer<'de> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNum {
            Str(String),
            Num(u32),
        }
        let v = Option::<StringOrNum>::deserialize(deserializer)?;
        Ok(match v {
            Some(StringOrNum::Num(n)) => Some(n),
            Some(StringOrNum::Str(s)) => s.parse().ok(),
            None => None,
        })
    }

    #[allow(dead_code)]
    pub fn deserialize_opt_u64<'de, D>(deserializer: D) -> Result<Option<u64>, D::Error>
    where D: Deserializer<'de> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNum {
            Str(String),
            Num(u64),
        }
        let v = Option::<StringOrNum>::deserialize(deserializer)?;
        Ok(match v {
            Some(StringOrNum::Num(n)) => Some(n),
            Some(StringOrNum::Str(s)) => s.parse().ok(),
            None => None,
        })
    }

    pub fn deserialize_opt_usize<'de, D>(deserializer: D) -> Result<Option<usize>, D::Error>
    where D: Deserializer<'de> {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum StringOrNum {
            Str(String),
            Num(usize),
        }
        let v = Option::<StringOrNum>::deserialize(deserializer)?;
        Ok(match v {
            Some(StringOrNum::Num(n)) => Some(n),
            Some(StringOrNum::Str(s)) => s.parse().ok(),
            None => None,
        })
    }
}

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};
use serde::Deserialize;
use schemars::JsonSchema;

use crate::client::GitLabClient;
use crate::config::Config;
use crate::logging::ToolTimer;
use crate::resolver::Resolver;
use crate::teams::Teams;
use crate::tools;

/// The MCP server.
#[derive(Clone)]
pub struct GlMcpServer {
    resolver: std::sync::Arc<Resolver>,
    config: std::sync::Arc<Config>,
    teams: std::sync::Arc<std::sync::Mutex<Teams>>,
    tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
}

// ─── Parameter structs ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListProjectsParams {
    #[schemars(description = "Search query to filter projects")]
    search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetProjectParams {
    #[schemars(description = "Project ID or URL-encoded path (e.g., 'my-group/my-project')")]
    project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMembersParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchIssuesParams {
    #[schemars(description = "Project ID or path (empty = search all projects)")]
    project_id: Option<String>,
    #[schemars(description = "Search text in title/description")]
    search: Option<String>,
    #[schemars(description = "Filter by state: opened, closed, all (default: opened)")]
    state: Option<String>,
    #[schemars(description = "Comma-separated label names")]
    labels: Option<String>,
    #[schemars(description = "Filter by assignee username")]
    assignee: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetIssueParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Issue IID (the # number)")]
    issue_iid: u64,
    #[schemars(description = "Include comments (default: true)")]
    include_notes: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CreateIssueParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Issue title")]
    title: String,
    #[schemars(description = "Issue description (markdown)")]
    description: Option<String>,
    #[schemars(description = "Comma-separated label names")]
    labels: Option<String>,
    #[schemars(description = "Assignee username")]
    assignee: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateIssueParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Issue IID")]
    issue_iid: u64,
    #[schemars(description = "New title (empty = keep)")]
    title: Option<String>,
    #[schemars(description = "New description (empty = keep)")]
    description: Option<String>,
    #[schemars(description = "New state: close or reopen")]
    state_event: Option<String>,
    #[schemars(description = "New labels (comma-separated, replaces existing)")]
    labels: Option<String>,
    #[schemars(description = "Assignee username (empty = unassign)")]
    assignee: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AddNoteParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Issue or MR IID")]
    iid: u64,
    #[schemars(description = "Note type: 'issue' or 'mr' (default: 'issue')")]
    note_type: Option<String>,
    #[schemars(description = "Comment text (markdown)")]
    body: String,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMergeRequestsParams {
    #[schemars(description = "Project ID or path (empty = all projects)")]
    project_id: Option<String>,
    #[schemars(description = "Filter by state: opened, closed, merged, all (default: opened)")]
    state: Option<String>,
    #[schemars(description = "Filter by author username")]
    author: Option<String>,
    #[schemars(description = "Scope: assigned_to_me, created_by_me, all (default: all)")]
    scope: Option<String>,
    #[schemars(description = "Only MRs created after this date (ISO, e.g., '2026-03-01')")]
    created_after: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMergeRequestParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Merge request IID (the ! number)")]
    mr_iid: u64,
    #[schemars(description = "Include comments (default: true)")]
    include_notes: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListPipelinesParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Filter by status: running, pending, success, failed, canceled")]
    status: Option<String>,
    #[schemars(description = "Filter by git ref (branch/tag)")]
    ref_name: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetPipelineParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Pipeline ID")]
    pipeline_id: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetJobLogParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Job ID")]
    job_id: u64,
    #[schemars(description = "Max lines from end of log (default: 100)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_usize")]
    tail: Option<usize>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RetryPipelineParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Pipeline ID")]
    pipeline_id: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CancelPipelineParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Pipeline ID")]
    pipeline_id: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListBranchesParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Search branch name")]
    search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListCommitsParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Branch or tag name (empty = default branch)")]
    branch: Option<String>,
    #[schemars(description = "Filter by author name or email")]
    author: Option<String>,
    #[schemars(description = "ISO date: commits after this date (e.g., '2026-03-24T00:00:00Z')")]
    since: Option<String>,
    #[schemars(description = "ISO date: commits before this date")]
    until: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetCommitDiffParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Commit SHA (full or short)")]
    sha: String,
    #[schemars(description = "Max diff lines per file (default: 200)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_usize")]
    max_lines_per_file: Option<usize>,
    #[schemars(description = "Skip lockfiles and generated code (default: true)")]
    skip_generated: Option<bool>,
    #[schemars(description = "Return only file list + stats, no diff content. ~10x smaller response.")]
    summary_only: Option<bool>,
    #[schemars(description = "Only show diff for files matching this path substring")]
    file: Option<String>,
    #[schemars(description = "Strip markdown formatting for smaller responses")]
    compact: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrChangesParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Merge request IID")]
    mr_iid: u64,
    #[schemars(description = "Max diff lines per file (default: 200)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_usize")]
    max_lines_per_file: Option<usize>,
    #[schemars(description = "Skip lockfiles and generated code (default: true)")]
    skip_generated: Option<bool>,
    #[schemars(description = "Return only file list + stats, no diff content")]
    summary_only: Option<bool>,
    #[schemars(description = "Only show diff for files matching this path substring")]
    file: Option<String>,
    #[schemars(description = "Strip markdown formatting for smaller responses")]
    compact: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetFileContentParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "File path within the repository")]
    file_path: String,
    #[schemars(description = "Branch, tag, or commit SHA (default: default branch)")]
    ref_name: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetUserActivityParams {
    #[schemars(description = "GitLab username")]
    username: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week' (since Monday), '3d', or hours as number (default: 24)")]
    period: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTeamActivityParams {
    #[schemars(description = "Team key from teams.json (e.g., 'devops', 'backend') OR comma-separated usernames")]
    team: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week', '3d', or hours (default: 24)")]
    period: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTeamsParams {}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SaveTeamParams {
    #[schemars(description = "Team key (e.g., 'devops')")]
    key: String,
    #[schemars(description = "Team display name")]
    name: String,
    #[schemars(description = "Comma-separated GitLab usernames")]
    usernames: String,
    #[schemars(description = "Comma-separated project paths (optional)")]
    projects: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GenerateDevReportParams {
    #[schemars(description = "GitLab username")]
    username: String,
    #[schemars(description = "Period: 'today', 'yesterday', 'week', '3d', or hours (default: today)")]
    period: Option<String>,
    #[schemars(description = "Filter to specific project path substring (optional)")]
    project: Option<String>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListGroupProjectsParams {
    #[schemars(description = "Group path (e.g., 'freevpnplanet/software')")]
    group_path: String,
    #[schemars(description = "Max results (default: 50)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

// ─── Repository params ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchCodeParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Search query (regex supported)")]
    query: String,
    #[schemars(description = "Branch/tag to search in (optional)")]
    ref_name: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetLanguagesParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetTreeParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Directory path (empty = root)")]
    path: Option<String>,
    #[schemars(description = "Branch/tag/SHA (optional)")]
    ref_name: Option<String>,
    #[schemars(description = "Include subdirectories recursively (default: false)")]
    recursive: Option<bool>,
    #[schemars(description = "Max results (default: 100)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CompareBranchesParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Base branch/tag/SHA")]
    from: String,
    #[schemars(description = "Head branch/tag/SHA")]
    to: String,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListTagsParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Search tag name")]
    search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
    #[serde(default, deserialize_with = "flex::deserialize_opt_u32")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetMrApprovalsParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Merge request IID")]
    mr_iid: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateFileParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "File path in the repository (e.g., 'README.md')")]
    file_path: String,
    #[schemars(description = "File content")]
    content: String,
    #[schemars(description = "Branch name to commit to (must NOT be main/master/develop)")]
    branch: String,
    #[schemars(description = "Commit message")]
    commit_message: String,
    #[schemars(description = "Source branch to create from (default: main)")]
    source_branch: Option<String>,
    #[schemars(description = "Create a merge request after commit (default: true)")]
    create_mr: Option<bool>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

// ─── Lint params ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateCommitParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Commit SHA")]
    sha: String,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ValidateMrParams {
    #[schemars(description = "Project ID or path")]
    project_id: String,
    #[schemars(description = "Merge request IID")]
    mr_iid: u64,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListRulesParams {
    #[schemars(description = "Language filter: PHP, Kotlin, TypeScript, Ansible (empty = all)")]
    language: Option<String>,
}

// ─── Helpers ───

/// Parse human-readable period into hours.
fn parse_period(period: &str) -> u32 {
    let p = period.trim().to_lowercase();
    match p.as_str() {
        "today" => {
            let now = chrono::Utc::now();
            let midnight = now.date_naive().and_hms_opt(0, 0, 0).unwrap();
            let diff = now.naive_utc() - midnight;
            diff.num_hours().max(1) as u32
        }
        "yesterday" => {
            let now = chrono::Utc::now();
            let yesterday_midnight = (now - chrono::Duration::days(1)).date_naive().and_hms_opt(0, 0, 0).unwrap();
            let diff = now.naive_utc() - yesterday_midnight;
            diff.num_hours().max(1) as u32
        }
        "week" => {
            let now = chrono::Utc::now();
            use chrono::Datelike;
            let weekday = now.weekday().num_days_from_monday();
            let monday = now - chrono::Duration::days(weekday as i64);
            let monday_midnight = monday.date_naive().and_hms_opt(0, 0, 0).unwrap();
            let diff = now.naive_utc() - monday_midnight;
            diff.num_hours().max(1) as u32
        }
        _ if p.ends_with('d') => {
            p.trim_end_matches('d').parse::<u32>().unwrap_or(1) * 24
        }
        _ if p.ends_with('h') => {
            p.trim_end_matches('h').parse::<u32>().unwrap_or(24)
        }
        _ => p.parse::<u32>().unwrap_or(24),
    }
}

fn resolve_client<'a>(resolver: &'a Resolver, instance: &Option<String>, id: &str) -> std::result::Result<&'a GitLabClient, McpError> {
    resolver
        .resolve(instance.as_deref().unwrap_or(""), id)
        .map_err(|e| McpError::internal_error(e.to_string(), None))
}

fn strip_markdown(text: &str) -> String {
    let mut out = text.replace("**", "").replace("__", "");
    out = out.lines().map(|line| {
        if line.starts_with("### ") { &line[4..] }
        else if line.starts_with("## ") { &line[3..] }
        else if line.starts_with("# ") { &line[2..] }
        else { line }
    }).collect::<Vec<_>>().join("\n");
    while out.contains("\n\n\n") {
        out = out.replace("\n\n\n", "\n\n");
    }
    out
}

/// Guard: block write tools in read-only mode.
macro_rules! write_guard {
    ($self:expr, $name:literal) => {
        if $self.config.read_only && crate::tools::WRITE_TOOLS.contains(&$name) {
            return Ok(CallToolResult::error(vec![Content::text(
                format!("Tool '{}' is disabled in read-only mode (GITLAB_READ_ONLY=1)", $name)
            )]));
        }
        if !crate::tools::is_tool_enabled($name, false, &$self.config.disabled_tools) {
            return Ok(CallToolResult::error(vec![Content::text(
                format!("Tool '{}' is disabled via DISABLED_TOOLS", $name)
            )]));
        }
    };
}

/// Tool call wrapper: handles compact mode + analytics logging.
macro_rules! tool_call {
    ($self:expr, $name:literal, $body:expr) => {{
        let timer = ToolTimer::start($name, None);
        match $body {
            Ok(text) => {
                timer.finish("ok", text.len(), None);
                let output = if $self.config.compact { strip_markdown(&text) } else { text };
                Ok(CallToolResult::success(vec![Content::text(output)]))
            }
            Err(e) => {
                let msg = e.short_message();
                timer.finish("error", 0, Some(msg.clone()));
                Ok(CallToolResult::error(vec![Content::text(msg)]))
            }
        }
    }};
}

// ─── Tool registration ───

#[tool_router]
impl GlMcpServer {
    pub fn new(config: Config) -> Self {
        let resolver = std::sync::Arc::new(Resolver::new(&config));
        let teams = Teams::load();
        let team_count = teams.list().len();
        if team_count > 0 {
            eprintln!("Loaded {} teams from teams.json", team_count);
        }
        Self {
            resolver,
            config: std::sync::Arc::new(config),
            teams: std::sync::Arc::new(std::sync::Mutex::new(teams)),
            tool_router: Self::tool_router(),
        }
    }

    // ─── Projects ───

    #[tool(description = "List GitLab projects accessible to the authenticated user")]
    async fn list_projects(&self, Parameters(p): Parameters<ListProjectsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_projects",
            tools::projects::list_projects(client, p.search.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get detailed info about a GitLab project")]
    async fn get_project(&self, Parameters(p): Parameters<GetProjectParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_project",
            tools::projects::get_project(client, &p.project_id).await
        )
    }

    #[tool(description = "List members of a GitLab project")]
    async fn list_members(&self, Parameters(p): Parameters<ListMembersParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_members",
            tools::projects::list_members(client, &p.project_id).await
        )
    }

    #[tool(description = "List all projects in a GitLab group (including subgroups)")]
    async fn list_group_projects(&self, Parameters(p): Parameters<ListGroupProjectsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_group_projects",
            tools::commits::list_group_projects(client, &p.group_path, p.per_page.unwrap_or(50)).await
        )
    }

    // ─── Issues ───

    #[tool(description = "Search GitLab issues across all projects or within a specific project")]
    async fn search_issues(&self, Parameters(p): Parameters<SearchIssuesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "search_issues",
            tools::issues::search_issues(client, p.project_id.as_deref().unwrap_or(""), p.search.as_deref().unwrap_or(""), p.state.as_deref().unwrap_or("opened"), p.labels.as_deref().unwrap_or(""), p.assignee.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get full details of a GitLab issue including description and comments")]
    async fn get_issue(&self, Parameters(p): Parameters<GetIssueParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_issue",
            tools::issues::get_issue(client, &p.project_id, p.issue_iid, p.include_notes.unwrap_or(true)).await
        )
    }

    #[tool(description = "Create a new issue in a GitLab project")]
    async fn create_issue(&self, Parameters(p): Parameters<CreateIssueParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "create_issue");
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "create_issue",
            tools::issues::create_issue(client, &p.project_id, &p.title, p.description.as_deref().unwrap_or(""), p.labels.as_deref().unwrap_or(""), p.assignee.as_deref().unwrap_or(""), None).await
        )
    }

    #[tool(description = "Update a GitLab issue: title, description, state, labels, assignee")]
    async fn update_issue(&self, Parameters(p): Parameters<UpdateIssueParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "update_issue");
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "update_issue",
            tools::issues::update_issue(client, &p.project_id, p.issue_iid, p.title.as_deref(), p.description.as_deref(), p.state_event.as_deref(), p.labels.as_deref(), p.assignee.as_deref()).await
        )
    }

    #[tool(description = "Add a comment (note) to an issue or merge request")]
    async fn add_note(&self, Parameters(p): Parameters<AddNoteParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "add_note");
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let note_type = p.note_type.as_deref().unwrap_or("issue");
        tool_call!(self, "add_note",
            tools::issues::add_note(client, &p.project_id, p.iid, note_type, &p.body).await
        )
    }

    // ─── Merge Requests ───

    #[tool(description = "List merge requests. Filter by project, state, author, scope, created_after.")]
    async fn list_merge_requests(&self, Parameters(p): Parameters<ListMergeRequestsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_merge_requests",
            tools::merge_requests::list_merge_requests(client, p.project_id.as_deref().unwrap_or(""), p.state.as_deref().unwrap_or("opened"), p.author.as_deref().unwrap_or(""), p.scope.as_deref().unwrap_or("all"), p.created_after.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get full details of a merge request including pipeline status and comments")]
    async fn get_merge_request(&self, Parameters(p): Parameters<GetMergeRequestParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_merge_request",
            tools::merge_requests::get_merge_request(client, &p.project_id, p.mr_iid, p.include_notes.unwrap_or(true)).await
        )
    }

    // ─── Pipelines ───

    #[tool(description = "List CI/CD pipelines for a project")]
    async fn list_pipelines(&self, Parameters(p): Parameters<ListPipelinesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_pipelines",
            tools::pipelines::list_pipelines(client, &p.project_id, p.status.as_deref().unwrap_or(""), p.ref_name.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get pipeline details including all jobs grouped by stage")]
    async fn get_pipeline(&self, Parameters(p): Parameters<GetPipelineParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_pipeline",
            tools::pipelines::get_pipeline(client, &p.project_id, p.pipeline_id).await
        )
    }

    #[tool(description = "Get CI job log output. Returns last N lines (tail). Critical for debugging failed jobs.")]
    async fn get_job_log(&self, Parameters(p): Parameters<GetJobLogParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_job_log",
            tools::pipelines::get_job_log(client, &p.project_id, p.job_id, p.tail.unwrap_or(100)).await
        )
    }

    #[tool(description = "Retry a failed pipeline")]
    async fn retry_pipeline(&self, Parameters(p): Parameters<RetryPipelineParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "retry_pipeline");
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "retry_pipeline",
            tools::pipelines::retry_pipeline(client, &p.project_id, p.pipeline_id).await
        )
    }

    #[tool(description = "Cancel a running pipeline")]
    async fn cancel_pipeline(&self, Parameters(p): Parameters<CancelPipelineParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "cancel_pipeline");
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "cancel_pipeline",
            tools::pipelines::cancel_pipeline(client, &p.project_id, p.pipeline_id).await
        )
    }

    // ─── Branches ───

    #[tool(description = "List branches for a project, optionally filtered by name")]
    async fn list_branches(&self, Parameters(p): Parameters<ListBranchesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_branches",
            tools::projects::list_branches(client, &p.project_id, p.search.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    // ─── Commits & Diffs ───

    #[tool(description = "List commits for a project, optionally filtered by branch, author, and date range")]
    async fn list_commits(&self, Parameters(p): Parameters<ListCommitsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_commits",
            tools::commits::list_commits(client, &p.project_id, p.branch.as_deref().unwrap_or(""), p.author.as_deref().unwrap_or(""), p.since.as_deref().unwrap_or(""), p.until.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get commit diff with smart filtering. Use summary_only=true first, then file= to drill in.")]
    async fn get_commit_diff(&self, Parameters(p): Parameters<GetCommitDiffParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let compact = p.compact.unwrap_or(self.config.compact);
        tool_call!(self, "get_commit_diff",
            tools::commits::get_commit_diff(client, &p.project_id, &p.sha, p.max_lines_per_file.unwrap_or(200), p.skip_generated.unwrap_or(true), p.summary_only.unwrap_or(false), p.file.as_deref().unwrap_or(""), compact).await
        )
    }

    #[tool(description = "Get all MR changes as unified diff. Use summary_only=true first, then file= for specific files.")]
    async fn get_mr_changes(&self, Parameters(p): Parameters<GetMrChangesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let compact = p.compact.unwrap_or(self.config.compact);
        tool_call!(self, "get_mr_changes",
            tools::commits::get_mr_changes(client, &p.project_id, p.mr_iid, p.max_lines_per_file.unwrap_or(200), p.skip_generated.unwrap_or(true), p.summary_only.unwrap_or(false), p.file.as_deref().unwrap_or(""), compact).await
        )
    }

    #[tool(description = "Get file content at a specific branch, tag, or commit SHA")]
    async fn get_file_content(&self, Parameters(p): Parameters<GetFileContentParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_file_content",
            tools::commits::get_file_content(client, &p.project_id, &p.file_path, p.ref_name.as_deref().unwrap_or("HEAD")).await
        )
    }

    #[tool(description = "Get developer daily activity across all projects: commits, MRs, grouped by day and project. Use period='today', 'yesterday', 'week', '3d', or hours.")]
    async fn get_user_activity(&self, Parameters(p): Parameters<GetUserActivityParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let hours = parse_period(p.period.as_deref().unwrap_or("24"));
        tool_call!(self, "get_user_activity",
            tools::commits::get_user_activity(client, &p.username, hours).await
        )
    }

    #[tool(description = "Get team activity. Pass team key from teams.json (e.g., 'devops') or comma-separated usernames.")]
    async fn get_team_activity(&self, Parameters(p): Parameters<GetTeamActivityParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let hours = parse_period(p.period.as_deref().unwrap_or("24"));

        // Resolve: team key from teams.json OR raw usernames
        let raw_usernames: Vec<String> = {
            let teams = self.teams.lock().unwrap();
            if let Some(team) = teams.get(&p.team) {
                team.members.iter().map(|m| m.username.clone()).collect()
            } else {
                p.team.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect()
            }
        };
        let usernames: Vec<&str> = raw_usernames.iter().map(|s| s.as_str()).collect();

        tool_call!(self, "get_team_activity",
            tools::commits::get_team_activity(client, &usernames, hours).await
        )
    }

    #[tool(description = "List configured teams from ~/.gl-mcp/teams.json")]
    async fn list_teams(&self, Parameters(_p): Parameters<ListTeamsParams>) -> Result<CallToolResult, McpError> {
        let teams = self.teams.lock().unwrap();
        let list = teams.list();
        if list.is_empty() {
            return Ok(CallToolResult::success(vec![Content::text(
                "No teams configured. Use save_team to add teams, or create ~/.gl-mcp/teams.json manually."
            )]));
        }
        let mut lines = vec![format!("**Teams: {}**\n", list.len())];
        for (key, team) in &list {
            let members: Vec<&str> = team.members.iter().map(|m| m.username.as_str()).collect();
            let projects: String = if team.projects.is_empty() {
                "–".into()
            } else {
                team.projects.join(", ")
            };
            lines.push(format!(
                "- **{}** ({}): {} | projects: {}",
                key, team.name, members.join(", "), projects
            ));
        }
        Ok(CallToolResult::success(vec![Content::text(lines.join("\n"))]))
    }

    #[tool(description = "Save a team to ~/.gl-mcp/teams.json (not committed to repo)")]
    async fn save_team(&self, Parameters(p): Parameters<SaveTeamParams>) -> Result<CallToolResult, McpError> {
        let members: Vec<crate::teams::TeamMember> = p.usernames
            .split(',')
            .map(|s| s.trim())
            .filter(|s| !s.is_empty())
            .map(|u| crate::teams::TeamMember { username: u.to_string(), name: String::new() })
            .collect();

        let projects: Vec<String> = p.projects
            .unwrap_or_default()
            .split(',')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect();

        let team = crate::teams::Team {
            name: p.name.clone(),
            members,
            projects,
        };

        let mut teams = self.teams.lock().unwrap();
        teams.set(p.key.clone(), team);
        teams.save().map_err(|e| McpError::internal_error(format!("Failed to save teams.json: {e}"), None))?;

        let count = teams.list().len();
        Ok(CallToolResult::success(vec![Content::text(
            format!("Saved team '{}' ({}). Total teams: {count}. File: ~/.gl-mcp/teams.json", p.key, p.name)
        )]))
    }

    #[tool(description = "Generate a complete HTML daily report for a developer. Returns full HTML with dark theme, commits, diffs, open MRs, and quality notes. Save to file and open in browser.")]
    async fn generate_dev_report(&self, Parameters(p): Parameters<GenerateDevReportParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let hours = parse_period(p.period.as_deref().unwrap_or("today"));
        tool_call!(self, "generate_dev_report",
            tools::reports::generate_dev_report(client, &p.username, hours, p.project.as_deref().unwrap_or("")).await
        )
    }

    // ─── Repository ───

    #[tool(description = "Search code in a project. Returns matching file paths, line numbers, and code snippets.")]
    async fn search_code(&self, Parameters(p): Parameters<SearchCodeParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "search_code",
            tools::repository::search_code(client, &p.project_id, &p.query, p.ref_name.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get project language breakdown (e.g., PHP 80%, Go 15%).")]
    async fn get_languages(&self, Parameters(p): Parameters<GetLanguagesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_languages",
            tools::repository::get_languages(client, &p.project_id).await
        )
    }

    #[tool(description = "Get repository directory listing. Use recursive=true for full tree.")]
    async fn get_tree(&self, Parameters(p): Parameters<GetTreeParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_tree",
            tools::repository::get_tree(client, &p.project_id, p.path.as_deref().unwrap_or(""), p.ref_name.as_deref().unwrap_or(""), p.recursive.unwrap_or(false), p.per_page.unwrap_or(100)).await
        )
    }

    #[tool(description = "Compare two branches/tags/SHAs. Shows commits and changed files between them.")]
    async fn compare_branches(&self, Parameters(p): Parameters<CompareBranchesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "compare_branches",
            tools::repository::compare_branches(client, &p.project_id, &p.from, &p.to).await
        )
    }

    #[tool(description = "List tags (releases) for a project.")]
    async fn list_tags(&self, Parameters(p): Parameters<ListTagsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "list_tags",
            tools::repository::list_tags(client, &p.project_id, p.search.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get merge request approval status: who approved, how many remaining.")]
    async fn get_mr_approvals(&self, Parameters(p): Parameters<GetMrApprovalsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_mr_approvals",
            tools::repository::get_mr_approvals(client, &p.project_id, p.mr_iid).await
        )
    }

    #[tool(description = "Create or update a file in a GitLab repo. Always commits to a new branch (never main), optionally creates MR. Use for README fixes, translations, config updates.")]
    async fn update_file(&self, Parameters(p): Parameters<UpdateFileParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "update_file");
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "update_file",
            tools::repository::update_file(
                client, &p.project_id, &p.file_path, &p.content, &p.branch,
                &p.commit_message, p.source_branch.as_deref().unwrap_or(""),
                p.create_mr.unwrap_or(true),
            ).await
        )
    }

    // ─── Lint ───

    #[tool(description = "Validate a commit against coding rules (regex-based, zero LLM tokens). Returns only violations grouped by severity.")]
    async fn validate_commit(&self, Parameters(p): Parameters<ValidateCommitParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "validate_commit",
            tools::lint::validate_commit(client, &p.project_id, &p.sha).await
        )
    }

    #[tool(description = "Validate all commits in a merge request against coding rules. Returns only violations.")]
    async fn validate_mr(&self, Parameters(p): Parameters<ValidateMrParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "validate_mr",
            tools::lint::validate_mr(client, &p.project_id, p.mr_iid).await
        )
    }

    #[tool(description = "List available coding rules, optionally filtered by language.")]
    async fn list_rules(&self, Parameters(p): Parameters<ListRulesParams>) -> Result<CallToolResult, McpError> {
        tool_call!(self, "list_rules",
            Ok::<String, crate::error::Error>(tools::lint::list_rules(p.language.as_deref().unwrap_or("")))
        )
    }
}

// ─── ServerHandler ───

#[tool_handler]
impl ServerHandler for GlMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo {
            protocol_version: ProtocolVersion::V_2025_06_18,
            capabilities: ServerCapabilities::builder()
                .enable_tools()
                .build(),
            server_info: Implementation::from_build_env(),
            instructions: Some(
                "GitLab MCP server — projects, issues, merge requests, CI/CD pipelines, commits, and code review.".to_string(),
            ),
        }
    }
}
