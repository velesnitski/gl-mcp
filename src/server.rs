//! MCP server handler — wires GitLab tools to rmcp.
//!
//! Parameter structs are in `src/params.rs`.

use rmcp::{
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, Content, Implementation, ProtocolVersion,
        ServerCapabilities, ServerInfo,
    },
    tool, tool_handler, tool_router,
    ErrorData as McpError, ServerHandler,
};

use crate::client::GitLabClient;
use crate::config::Config;
use crate::logging::ToolTimer;
use crate::params::*;
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

// ─── Helpers ───

/// Parse human-readable period into hours.
pub(crate) fn parse_period(period: &str) -> u32 {
    let p = period.trim().to_lowercase();
    match p.as_str() {
        "today" => {
            let now = chrono::Utc::now();
            let midnight = now.date_naive().and_time(chrono::NaiveTime::MIN);
            let diff = now.naive_utc() - midnight;
            diff.num_hours().max(1) as u32
        }
        "yesterday" => {
            let now = chrono::Utc::now();
            let yesterday_midnight = (now - chrono::Duration::days(1)).date_naive().and_time(chrono::NaiveTime::MIN);
            let diff = now.naive_utc() - yesterday_midnight;
            diff.num_hours().max(1) as u32
        }
        "week" => {
            let now = chrono::Utc::now();
            use chrono::Datelike;
            let weekday = now.weekday().num_days_from_monday();
            let monday = now - chrono::Duration::days(weekday as i64);
            let monday_midnight = monday.date_naive().and_time(chrono::NaiveTime::MIN);
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

pub(crate) fn strip_markdown(text: &str) -> String {
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

const RESPONSE_SIZE_WARN: usize = 15000;

/// Tool call wrapper: handles compact mode, size warnings, analytics logging.
macro_rules! tool_call {
    ($self:expr, $name:literal, $body:expr) => {{
        let timer = ToolTimer::start($name, None);
        match $body {
            Ok(text) => {
                timer.finish("ok", text.len(), None);
                let mut output = if $self.config.compact { strip_markdown(&text) } else { text };
                if output.len() > RESPONSE_SIZE_WARN {
                    let kb = output.len() / 1024;
                    output = format!(
                        "*Warning: Large response ({kb}KB). Use `summary_only=true` or filter parameters to reduce token usage.*\n\n{output}"
                    );
                }
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

    #[tool(description = "Search GitLab issues across all projects, within a specific project, or within a group")]
    async fn search_issues(&self, Parameters(p): Parameters<SearchIssuesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "search_issues",
            tools::issues::search_issues(client, p.project_id.as_deref().unwrap_or(""), p.group_id.as_deref().unwrap_or(""), p.search.as_deref().unwrap_or(""), p.state.as_deref().unwrap_or("opened"), p.labels.as_deref().unwrap_or(""), p.assignee.as_deref().unwrap_or(""), p.per_page.unwrap_or(20)).await
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

    #[tool(description = "List merge requests. Filter by project, group, state, author, scope, created_after, opened_before.")]
    async fn list_merge_requests(&self, Parameters(p): Parameters<ListMergeRequestsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_merge_requests",
            tools::merge_requests::list_merge_requests(client, p.project_id.as_deref().unwrap_or(""), p.state.as_deref().unwrap_or("opened"), p.author.as_deref().unwrap_or(""), p.scope.as_deref().unwrap_or("all"), p.created_after.as_deref().unwrap_or(""), p.opened_before.as_deref().unwrap_or(""), p.group_id.as_deref().unwrap_or(""), p.per_page.unwrap_or(20), p.summary_only.unwrap_or(false)).await
        )
    }

    #[tool(description = "Create a merge request with smart defaults. Auto-generates title from branch name (e.g., 'feature/PROJ-123-add-auth' → 'PROJ-123: Add auth'), auto-fills description from commit list, validates source branch exists, checks for duplicate MRs. Returns MR URL + diff stats.")]
    async fn create_merge_request(&self, Parameters(p): Parameters<CreateMergeRequestParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "create_merge_request");
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "create_merge_request",
            tools::merge_requests::create_merge_request(
                client,
                &p.project_id,
                &p.source_branch,
                p.target_branch.as_deref().unwrap_or(""),
                p.title.as_deref().unwrap_or(""),
                p.description.as_deref().unwrap_or(""),
                p.labels.as_deref().unwrap_or(""),
                p.assignee.as_deref().unwrap_or(""),
                p.reviewers.as_deref().unwrap_or(""),
                p.squash.unwrap_or(true),
                p.remove_source_branch.unwrap_or(true),
                p.draft.unwrap_or(false),
            ).await
        )
    }

    #[tool(description = "Get full details of a merge request including pipeline status and comments")]
    async fn get_merge_request(&self, Parameters(p): Parameters<GetMergeRequestParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_merge_request",
            tools::merge_requests::get_merge_request(client, &p.project_id, p.mr_iid, p.include_notes.unwrap_or(true)).await
        )
    }

    #[tool(description = "Get MR review turnaround stats: avg/median time to merge, slowest MRs, per-author breakdown.")]
    async fn get_mr_turnaround(&self, Parameters(p): Parameters<GetMrTurnaroundParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_mr_turnaround",
            tools::merge_requests::get_mr_turnaround(client, p.project_id.as_deref().unwrap_or(""), p.group_id.as_deref().unwrap_or(""), p.days.unwrap_or(7)).await
        )
    }

    #[tool(description = "Compact MR dashboard for a group: open count, avg age, reviewer bottlenecks, stale MRs.")]
    async fn get_mr_dashboard(&self, Parameters(p): Parameters<GetMrDashboardParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_mr_dashboard",
            tools::merge_requests::get_mr_dashboard(client, &p.group_id).await
        )
    }

    #[tool(description = "Get MR review depth: how many comments/discussions per MR before merge. Shows zero-review MRs.")]
    async fn get_mr_review_depth(&self, Parameters(p): Parameters<GetMrReviewDepthParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_mr_review_depth",
            tools::merge_requests::get_mr_review_depth(client, p.project_id.as_deref().unwrap_or(""), p.group_id.as_deref().unwrap_or(""), p.days.unwrap_or(7)).await
        )
    }

    #[tool(description = "Cross-group MR dashboard: aggregate open MRs, reviewer load, stale counts across multiple groups.")]
    async fn get_org_mr_dashboard(&self, Parameters(p): Parameters<GetOrgMrDashboardParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let groups_raw: Vec<String> = p.groups.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let groups: Vec<&str> = groups_raw.iter().map(|s| s.as_str()).collect();
        tool_call!(self, "get_org_mr_dashboard",
            tools::merge_requests::get_org_mr_dashboard(client, &groups).await
        )
    }

    #[tool(description = "Classify MRs by category (feature, hotfix, bugfix, chore) based on branch naming conventions.")]
    async fn get_mr_categories(&self, Parameters(p): Parameters<GetMrCategoriesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_mr_categories",
            tools::merge_requests::get_mr_categories(client, p.project_id.as_deref().unwrap_or(""), p.group_id.as_deref().unwrap_or(""), p.state.as_deref().unwrap_or(""), p.days.unwrap_or(7)).await
        )
    }

    #[tool(description = "Decompose MR merge time into queue time (waiting for review) and review time. Shows which MRs sat longest.")]
    async fn get_mr_timeline(&self, Parameters(p): Parameters<GetMrTimelineParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_mr_timeline",
            tools::merge_requests::get_mr_timeline(client, p.project_id.as_deref().unwrap_or(""), p.group_id.as_deref().unwrap_or(""), p.days.unwrap_or(7)).await
        )
    }

    #[tool(description = "Cross-instance MR dashboard: aggregate MR stats across multiple GitLab instances and groups.")]
    async fn get_cross_instance_dashboard(&self, Parameters(p): Parameters<GetCrossInstanceDashboardParams>) -> Result<CallToolResult, McpError> {
        // Parse targets: "instance:group,instance:group" or just "group,group" with default instance
        let default_instance = p.instance.as_deref().unwrap_or("");
        let mut by_instance: std::collections::BTreeMap<String, Vec<String>> = std::collections::BTreeMap::new();

        for target in p.targets.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            if let Some((inst, group)) = target.split_once(':') {
                by_instance.entry(inst.to_string()).or_default().push(group.to_string());
            } else {
                by_instance.entry(default_instance.to_string()).or_default().push(target.to_string());
            }
        }

        let mut all_output: Vec<String> = Vec::new();

        for (inst, groups) in &by_instance {
            let inst_opt = if inst.is_empty() { None } else { Some(inst.as_str()) };
            let client = self.resolver
                .resolve(inst_opt.unwrap_or(""), "")
                .map_err(|e| McpError::internal_error(e.to_string(), None))?;
            let group_refs: Vec<&str> = groups.iter().map(|s| s.as_str()).collect();

            match tools::merge_requests::get_org_mr_dashboard(client, &group_refs).await {
                Ok(text) => {
                    let header = if by_instance.len() > 1 {
                        format!("## Instance: {}\n\n", if inst.is_empty() { "default" } else { inst })
                    } else {
                        String::new()
                    };
                    all_output.push(format!("{header}{text}"));
                }
                Err(e) => {
                    all_output.push(format!("**Error on instance {inst}:** {}", e.short_message()));
                }
            }
        }

        let output = all_output.join("\n\n---\n\n");
        let output = if self.config.compact { strip_markdown(&output) } else { output };
        Ok(CallToolResult::success(vec![Content::text(output)]))
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

    #[tool(description = "List pipelines for a merge request, showing status, ref, SHA, and creation time")]
    async fn get_mr_pipelines(&self, Parameters(p): Parameters<GetMrPipelinesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_mr_pipelines",
            tools::pipelines::get_mr_pipelines(client, &p.project_id, p.mr_iid).await
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

    #[tool(description = "Find stale branches: merged but not deleted, or inactive for N days. Helps with repo hygiene.")]
    async fn get_stale_branches(&self, Parameters(p): Parameters<GetStaleBranchesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_stale_branches",
            tools::projects::get_stale_branches(client, &p.project_id, p.inactive_days.unwrap_or(30)).await
        )
    }

    #[tool(description = "Delete a branch from a project")]
    async fn delete_branch(&self, Parameters(p): Parameters<DeleteBranchParams>) -> Result<CallToolResult, McpError> {
        write_guard!(self, "delete_branch");
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "delete_branch",
            tools::projects::delete_branch(client, &p.project_id, &p.branch).await
        )
    }

    #[tool(description = "Look up a GitLab user by username or numeric ID. Returns profile info, state, and admin status.")]
    async fn get_user(&self, Parameters(p): Parameters<GetUserParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "get_user",
            tools::projects::get_user(client, p.username.as_deref().unwrap_or(""), p.user_id).await
        )
    }

    // ─── Commits & Diffs ───

    #[tool(description = "List commits for a project, optionally filtered by branch, author, and date range")]
    async fn list_commits(&self, Parameters(p): Parameters<ListCommitsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        tool_call!(self, "list_commits",
            tools::commits::list_commits(client, &p.project_id, p.branch.as_deref().unwrap_or(""), p.author.as_deref().unwrap_or(""), p.since.as_deref().unwrap_or(""), p.until.as_deref().unwrap_or(""), p.per_page.unwrap_or(20), p.summary_only.unwrap_or(false)).await
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
            let teams = self.teams.lock().unwrap_or_else(|e| e.into_inner());
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

    #[tool(description = "Get activity for all members of a GitLab group. Auto-discovers members, no config needed.")]
    async fn get_group_activity(&self, Parameters(p): Parameters<GetGroupActivityParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, "")?;
        let hours = parse_period(p.period.as_deref().unwrap_or("24"));
        tool_call!(self, "get_group_activity",
            tools::commits::get_group_activity(client, &p.group_path, hours).await
        )
    }

    #[tool(description = "List configured teams from ~/.gl-mcp/teams.json")]
    async fn list_teams(&self, Parameters(_p): Parameters<ListTeamsParams>) -> Result<CallToolResult, McpError> {
        let teams = self.teams.lock().unwrap_or_else(|e| e.into_inner());
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
        let instances_list: Vec<&str> = p.instances
            .as_deref()
            .unwrap_or("")
            .split(',')
            .map(|s| s.trim())
            .collect();

        let members: Vec<crate::teams::TeamMember> = p.usernames
            .split(',')
            .enumerate()
            .map(|(i, s)| {
                let username = s.trim().to_string();
                let instance = instances_list.get(i).and_then(|s| {
                    if s.is_empty() { None } else { Some(s.to_string()) }
                });
                crate::teams::TeamMember { username, name: String::new(), instance }
            })
            .filter(|m| !m.username.is_empty())
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

        let mut teams = self.teams.lock().unwrap_or_else(|e| e.into_inner());
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

    #[tool(description = "Compare developers side-by-side in a project: MRs opened/merged/reviewed, approvals, avg merge time, comments, commits.")]
    async fn compare_developers(&self, Parameters(p): Parameters<CompareDevelopersParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        let raw_usernames: Vec<String> = p.usernames.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
        let usernames: Vec<&str> = raw_usernames.iter().map(|s| s.as_str()).collect();
        tool_call!(self, "compare_developers",
            tools::commits::compare_developers(client, &p.project_id, &usernames, p.days.unwrap_or(14)).await
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

    #[tool(description = "List project environments with last deployment info (SHA, branch, status, deployer).")]
    async fn list_environments(&self, Parameters(p): Parameters<ListEnvironmentsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "list_environments",
            tools::repository::list_environments(client, &p.project_id, p.per_page.unwrap_or(20)).await
        )
    }

    #[tool(description = "Get all-time contributor stats: commits, additions, deletions per person.")]
    async fn get_contributors(&self, Parameters(p): Parameters<GetContributorsParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_contributors",
            tools::repository::get_contributors(client, &p.project_id).await
        )
    }

    #[tool(description = "Get project-level MR approval rules: who must approve, required count.")]
    async fn get_approval_rules(&self, Parameters(p): Parameters<GetApprovalRulesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_approval_rules",
            tools::repository::get_approval_rules(client, &p.project_id).await
        )
    }

    #[tool(description = "Get deployment frequency (DORA metric): deploys per day, by environment and deployer.")]
    async fn get_deploy_frequency(&self, Parameters(p): Parameters<GetDeployFrequencyParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "get_deploy_frequency",
            tools::repository::get_deploy_frequency(client, &p.project_id, p.environment.as_deref().unwrap_or(""), p.days.unwrap_or(30)).await
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

    #[tool(description = "Validate MR using the full unified diff (not individual commits). Catches issues in squashed MRs where commit diffs are minimal. Checks all added lines against Swift, PHP, Kotlin, Go, TypeScript, and global rules.")]
    async fn validate_mr_changes(&self, Parameters(p): Parameters<ValidateMrChangesParams>) -> Result<CallToolResult, McpError> {
        let client = resolve_client(&self.resolver, &p.instance, &p.project_id)?;
        tool_call!(self, "validate_mr_changes",
            tools::lint::validate_mr_changes(client, &p.project_id, p.mr_iid).await
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_period_hours() {
        assert_eq!(parse_period("24"), 24);
        assert_eq!(parse_period("48"), 48);
    }

    #[test]
    fn test_parse_period_days() {
        assert_eq!(parse_period("3d"), 72);
        assert_eq!(parse_period("7d"), 168);
    }

    #[test]
    fn test_parse_period_hour_suffix() {
        assert_eq!(parse_period("12h"), 12);
    }

    #[test]
    fn test_parse_period_defaults() {
        assert_eq!(parse_period("invalid"), 24);
        assert_eq!(parse_period(""), 24);
    }

    #[test]
    fn test_strip_markdown() {
        assert_eq!(strip_markdown("**bold**"), "bold");
        assert_eq!(strip_markdown("### Header"), "Header");
        assert_eq!(strip_markdown("## H2"), "H2");
        assert_eq!(strip_markdown("# H1"), "H1");
        assert_eq!(strip_markdown("plain text"), "plain text");
    }
}
