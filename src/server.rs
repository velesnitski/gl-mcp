//! MCP server handler — wires GitLab tools to rmcp.

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
use crate::tools;

/// The MCP server.
#[derive(Clone)]
pub struct GlMcpServer {
    resolver: std::sync::Arc<Resolver>,
    config: std::sync::Arc<Config>,
    tool_router: rmcp::handler::server::tool::ToolRouter<Self>,
}

// ─── Parameter structs ───

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListProjectsParams {
    #[schemars(description = "Search query to filter projects")]
    search: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
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
    #[schemars(description = "Scope: assigned_to_me, created_by_me, all (default: all)")]
    scope: Option<String>,
    #[schemars(description = "Max results (default: 20)")]
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
    #[schemars(description = "Look back N hours (default: 24)")]
    hours: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListGroupProjectsParams {
    #[schemars(description = "Group path (e.g., 'example-org/software')")]
    group_path: String,
    #[schemars(description = "Max results (default: 50)")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

// ─── Helpers ───

fn resolve_client<'a>(resolver: &'a Resolver, instance: &Option<String>, id: &str) -> Result<&'a GitLabClient, McpError> {
    resolver
        .resolve(instance.as_deref().unwrap_or(""), id)
        .map_err(|e| McpError::internal_error(e, None))
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
                timer.finish("error", 0, Some(e.clone()));
                Ok(CallToolResult::error(vec![Content::text(e)]))
            }
        }
    }};
}

// ─── Tool registration ───

#[tool_router]
impl GlMcpServer {
    pub fn new(config: Config) -> Self {
        let resolver = std::sync::Arc::new(Resolver::new(&config));
        Self {
            resolver,
            config: std::sync::Arc::new(config),
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

    #[tool(description = "List merge requests across all projects or within a specific project")]
    async fn list_merge_requests(&self, Parameters(p): Parameters<ListMergeRequestsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_merge_requests",
            tools::merge_requests::list_merge_requests(client, p.project_id.as_deref().unwrap_or(""), p.state.as_deref().unwrap_or("opened"), p.scope.as_deref().unwrap_or("all"), p.per_page.unwrap_or(20)).await
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

    #[tool(description = "Get developer activity: commits, MRs opened/merged/approved for the last N hours")]
    async fn get_user_activity(&self, Parameters(p): Parameters<GetUserActivityParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_user_activity",
            tools::commits::get_user_activity(client, &p.username, p.hours.unwrap_or(24)).await
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
