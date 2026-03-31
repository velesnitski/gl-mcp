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
// use crate::logging::ToolTimer;
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
    #[schemars(description = "Return only file list + stats, no diff content. ~10x smaller response. Use for initial review, then drill into specific files.")]
    summary_only: Option<bool>,
    #[schemars(description = "Only show diff for files matching this path substring (e.g., 'AuthController.php')")]
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
    #[schemars(description = "Group path (e.g., 'freevpnplanet/software')")]
    group_path: String,
    #[schemars(description = "Max results (default: 50)")]
    per_page: Option<u32>,
    #[schemars(description = "GitLab instance name (optional)")]
    instance: Option<String>,
}

// ─── Helper ───

fn resolve_client<'a>(resolver: &'a Resolver, instance: &Option<String>, id: &str) -> Result<&'a GitLabClient, McpError> {
    resolver
        .resolve(instance.as_deref().unwrap_or(""), id)
        .map_err(|e| McpError::internal_error(e, None))
}

fn tool_ok(text: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::success(vec![Content::text(text)]))
}

fn tool_err(e: String) -> Result<CallToolResult, McpError> {
    Ok(CallToolResult::error(vec![Content::text(e)]))
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

    #[tool(description = "List GitLab projects accessible to the authenticated user")]
    async fn list_projects(&self, Parameters(p): Parameters<ListProjectsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::projects::list_projects(client, p.search.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get detailed info about a GitLab project")]
    async fn get_project(&self, Parameters(p): Parameters<GetProjectParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        match tools::projects::get_project(client, &p.project_id).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "List members of a GitLab project")]
    async fn list_members(&self, Parameters(p): Parameters<ListMembersParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::projects::list_members(client, &p.project_id).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Search GitLab issues. Can search across all projects or within a specific project")]
    async fn search_issues(&self, Parameters(p): Parameters<SearchIssuesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::issues::search_issues(
            client,
            p.project_id.as_deref().unwrap_or(""),
            p.search.as_deref().unwrap_or(""),
            p.state.as_deref().unwrap_or("opened"),
            p.labels.as_deref().unwrap_or(""),
            p.assignee.as_deref().unwrap_or(""),
            p.per_page.unwrap_or(20),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get full details of a GitLab issue including description and comments")]
    async fn get_issue(&self, Parameters(p): Parameters<GetIssueParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::issues::get_issue(client, &p.project_id, p.issue_iid, p.include_notes.unwrap_or(true)).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Create a new issue in a GitLab project")]
    async fn create_issue(&self, Parameters(p): Parameters<CreateIssueParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::issues::create_issue(
            client, &p.project_id, &p.title,
            p.description.as_deref().unwrap_or(""),
            p.labels.as_deref().unwrap_or(""),
            p.assignee.as_deref().unwrap_or(""),
            None,
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "List merge requests. Can search across all projects or within a specific project")]
    async fn list_merge_requests(&self, Parameters(p): Parameters<ListMergeRequestsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::merge_requests::list_merge_requests(
            client,
            p.project_id.as_deref().unwrap_or(""),
            p.state.as_deref().unwrap_or("opened"),
            p.scope.as_deref().unwrap_or("all"),
            p.per_page.unwrap_or(20),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get full details of a merge request including pipeline status and comments")]
    async fn get_merge_request(&self, Parameters(p): Parameters<GetMergeRequestParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::merge_requests::get_merge_request(client, &p.project_id, p.mr_iid, p.include_notes.unwrap_or(true)).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "List CI/CD pipelines for a project")]
    async fn list_pipelines(&self, Parameters(p): Parameters<ListPipelinesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::pipelines::list_pipelines(client, &p.project_id, p.status.as_deref().unwrap_or(""), p.ref_name.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get pipeline details including all jobs grouped by stage")]
    async fn get_pipeline(&self, Parameters(p): Parameters<GetPipelineParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::pipelines::get_pipeline(client, &p.project_id, p.pipeline_id).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    // ─── Commits & Diffs ───

    #[tool(description = "List commits for a project, optionally filtered by branch, author, and date range. Groups by author for overview.")]
    async fn list_commits(&self, Parameters(p): Parameters<ListCommitsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::commits::list_commits(
            client, &p.project_id,
            p.branch.as_deref().unwrap_or(""),
            p.author.as_deref().unwrap_or(""),
            p.since.as_deref().unwrap_or(""),
            p.until.as_deref().unwrap_or(""),
            p.per_page.unwrap_or(20),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get the diff of a commit with smart filtering. Use summary_only=true first for overview (~10x smaller), then drill into specific files with file= parameter.")]
    async fn get_commit_diff(&self, Parameters(p): Parameters<GetCommitDiffParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::commits::get_commit_diff(
            client, &p.project_id, &p.sha,
            p.max_lines_per_file.unwrap_or(200),
            p.skip_generated.unwrap_or(true),
            p.summary_only.unwrap_or(false),
            p.file.as_deref().unwrap_or(""),
            p.compact.unwrap_or(false),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get all changes in a merge request. Use summary_only=true first, then file= for specific files. Grouped by language.")]
    async fn get_mr_changes(&self, Parameters(p): Parameters<GetMrChangesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::commits::get_mr_changes(
            client, &p.project_id, p.mr_iid,
            p.max_lines_per_file.unwrap_or(200),
            p.skip_generated.unwrap_or(true),
            p.summary_only.unwrap_or(false),
            p.file.as_deref().unwrap_or(""),
            p.compact.unwrap_or(false),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get file content at a specific branch, tag, or commit SHA. Use for additional context during code review.")]
    async fn get_file_content(&self, Parameters(p): Parameters<GetFileContentParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::commits::get_file_content(
            client, &p.project_id, &p.file_path,
            p.ref_name.as_deref().unwrap_or("HEAD"),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "Get developer activity: commits, MRs opened/merged/approved for the last N hours. Mirrors youtrack-reports dev metrics.")]
    async fn get_user_activity(&self, Parameters(p): Parameters<GetUserActivityParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::commits::get_user_activity(
            client, &p.username, p.hours.unwrap_or(24),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
    }

    #[tool(description = "List all projects in a GitLab group (including subgroups). Use to expand team paths like 'backend/*'.")]
    async fn list_group_projects(&self, Parameters(p): Parameters<ListGroupProjectsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        match tools::commits::list_group_projects(
            client, &p.group_path, p.per_page.unwrap_or(50),
        ).await {
            Ok(text) => tool_ok(text),
            Err(e) => tool_err(e),
        }
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
                "GitLab MCP server — manage projects, issues, merge requests, and CI/CD pipelines.".to_string(),
            ),
        }
    }
}
