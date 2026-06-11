//! AI-assisted development adoption scanner.
//!
//! Scans a GitLab group's repos for AI tooling markers (CLAUDE.md, .claude/agents,
//! skills, MCP configs, ADR practice, AI co-authored commits) and produces a
//! per-team adoption scorecard with levels L0-L3 and quality flags.

use crate::client::GitLabClient;
use crate::error::Result;
use futures::future::join_all;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

/// Default for `dormant_days`: repos with no activity in this many days are
/// skipped as dormant.
pub(crate) const DORMANT_DAYS: u32 = 180;

/// Branch names that signal in-flight AI work (feature branches created by/for agents).
static AI_BRANCH_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)(claude|copilot|llm|agentic|agent|ai[-_])").unwrap());

/// "agent" matches that are NOT about AI: browser user agents, agency, etc.
static AI_BRANCH_EXCLUDE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)(user[-_ ]?agents?|agency|agenda)").unwrap());

/// True when a branch name looks AI-related.
pub(crate) fn is_ai_branch(name: &str) -> bool {
    // Strip known false-positive phrases first so the remainder is judged on its own.
    let cleaned = AI_BRANCH_EXCLUDE_RE.replace_all(name, "");
    AI_BRANCH_RE.is_match(&cleaned)
}

/// Markers detected for a single repository.
#[derive(Debug, Default)]
pub(crate) struct RepoMarkers {
    pub claude_md: bool,
    pub claude_md_size: u64,
    pub agents_count: usize,
    pub skills_count: usize,
    pub commands: bool,
    pub shared_settings: bool,
    pub hooks: bool,
    pub mcp_json: bool,
    pub agents_md: bool,
    pub cursor: bool,
    pub tasks_dir: bool,
    pub adr_count: usize,
    /// AI-trailed commits across ALL branches in the scan window.
    pub ai_commits: usize,
    /// All commits across ALL branches in the scan window.
    pub total_commits: usize,
    /// AI-trailed commits on the default branch only (squash/merge can strip trailers).
    pub ai_commits_default: usize,
    /// Commits touching `.tasks/` in the scan window — agent activity even without attribution.
    pub tasks_recent_commits: usize,
    /// Commits touching `.claude/` in the scan window.
    pub claude_recent_commits: usize,
    /// AI-related branch names found (capped at 3).
    pub branch_hits: Vec<String>,
    /// MRs in the window whose description carries AI markers (survives squash).
    pub ai_mr_count: usize,
    pub total_mr_count: usize,
    /// Commits touching `docs/adr` in the scan window.
    pub adr_recent_commits: usize,
    /// ISO date of the last commit touching CLAUDE.md (any time, not window-bound).
    pub claude_md_last_touch: Option<String>,
    /// CLAUDE.md last touched before the scan window started AND >= 30 commits since (lower bound).
    pub claude_md_stale: bool,
}

impl RepoMarkers {
    pub(crate) fn ai_pct(&self) -> f64 {
        if self.total_commits == 0 {
            0.0
        } else {
            self.ai_commits as f64 / self.total_commits as f64 * 100.0
        }
    }

    fn has_any_marker(&self) -> bool {
        self.claude_md
            || self.agents_md
            || self.cursor
            || self.mcp_json
            || self.agents_count > 0
            || self.skills_count > 0
            || self.commands
            || self.shared_settings
            || self.hooks
            || self.tasks_dir
    }
}

/// Active usage = measurable AI commit share OR recent commits touching `.tasks/`
/// OR MRs with AI markers in the description. Teams often disable Co-Authored-By
/// attribution (or squash strips it), so task-state files and MR descriptions are
/// first-class usage evidence.
pub(crate) fn has_active_usage(m: &RepoMarkers) -> bool {
    m.ai_pct() >= 10.0 || m.tasks_recent_commits > 0 || m.ai_mr_count > 0
}

/// Adoption trajectory for a repo:
/// - "↑" actively building: AI feature branches in flight, or markers with recent
///   `.claude/` or `docs/adr` maintenance
/// - "↓" decaying: markers present but no usage and no maintenance
/// - "→" steady: markers present, in use
/// - ""  no signals at all
pub(crate) fn trajectory(m: &RepoMarkers) -> &'static str {
    let has_markers = m.has_any_marker();
    if !has_markers && m.branch_hits.is_empty() {
        return "";
    }
    if !m.branch_hits.is_empty()
        || (has_markers && (m.claude_recent_commits > 0 || m.adr_recent_commits > 0))
    {
        return "↑";
    }
    if has_markers
        && !has_active_usage(m)
        && m.claude_recent_commits == 0
        && m.tasks_recent_commits == 0
    {
        return "↓";
    }
    "→"
}

/// Compute the adoption level (0-3) for a repo from its markers.
///
/// - L0 None: no markers at all
/// - L1 Exploring: any config marker (CLAUDE.md / AGENTS.md / cursorrules / mcp.json)
/// - L2 Practicing: CLAUDE.md + one of commands/settings/mcp/adr/hooks — OR agents with no AI commits
/// - L3 Scaling: agents + active usage (ai_pct >= 10 OR recent .tasks commits) — agentic workflow in use
pub(crate) fn adoption_level(m: &RepoMarkers) -> u8 {
    if m.agents_count > 0 && has_active_usage(m) {
        return 3;
    }
    let practicing = m.claude_md
        && (m.commands || m.shared_settings || m.mcp_json || m.adr_count > 0 || m.hooks);
    if practicing || (m.agents_count > 0 && m.ai_commits == 0) {
        return 2;
    }
    if m.has_any_marker() {
        return 1;
    }
    0
}

/// Quality flags for a repo: anti-patterns and easy wins.
pub(crate) fn quality_flags(m: &RepoMarkers) -> Vec<String> {
    let mut flags: Vec<String> = Vec::new();
    if m.claude_md && m.claude_md_size < 200 {
        flags.push("stub CLAUDE.md".into());
    }
    if m.claude_md && m.claude_md_size > 15000 {
        flags.push("bloated CLAUDE.md".into());
    }
    if m.agents_count > 0
        && m.ai_commits == 0
        && m.tasks_recent_commits == 0
        && m.claude_recent_commits == 0
        && m.ai_mr_count == 0
    {
        flags.push("setup unused".into());
    }
    if (m.tasks_recent_commits > 0 || m.claude_recent_commits > 0) && m.ai_commits == 0 {
        // Workflow is active but commits carry no Co-Authored-By trailer.
        flags.push("no attribution".into());
    }
    if m.ai_pct() > 10.0 && !m.claude_md {
        flags.push("usage w/o config".into());
    }
    if m.ai_commits > 0 && m.ai_commits_default == 0 {
        // Trailers exist on feature branches but merge/squash strips them from default.
        flags.push("squash-hidden usage".into());
    }
    if !m.has_any_marker() && !m.branch_hits.is_empty() {
        // No config landed yet, but AI work is happening on feature branches.
        flags.push(format!("in-flight (branch: {})", m.branch_hits[0]));
    }
    if m.claude_md && m.claude_md_stale {
        flags.push("stale config (30+ commits behind)".into());
    }
    flags
}

/// True if `flags` contains `flag` (exact match).
fn has_flag(flags: &[String], flag: &str) -> bool {
    flags.iter().any(|f| f == flag)
}

/// Scan result for one repository.
pub(crate) struct RepoResult {
    pub path: String,
    pub team: String,
    /// ISO date of last project activity (for the In-flight section).
    pub last_activity: String,
    /// Browser URL of the project (from the listing; empty when missing).
    pub web_url: String,
    /// Default branch (from the listing; "main" fallback).
    pub default_branch: String,
    pub markers: RepoMarkers,
}

impl RepoResult {
    pub(crate) fn level(&self) -> u8 {
        adoption_level(&self.markers)
    }

    /// Compact marker list for the table, e.g. "CLAUDE.md, agents(6), skills(9), tasks".
    pub(crate) fn marker_list(&self) -> String {
        let m = &self.markers;
        let mut parts: Vec<String> = Vec::new();
        if m.claude_md {
            parts.push("CLAUDE.md".into());
        }
        if m.agents_md {
            parts.push("AGENTS.md".into());
        }
        if m.agents_count > 0 {
            parts.push(format!("agents({})", m.agents_count));
        }
        if m.skills_count > 0 {
            parts.push(format!("skills({})", m.skills_count));
        }
        if m.commands {
            parts.push("commands".into());
        }
        if m.shared_settings {
            parts.push("settings".into());
        }
        if m.hooks {
            parts.push("hooks".into());
        }
        if m.mcp_json {
            parts.push(".mcp.json".into());
        }
        if m.cursor {
            parts.push("cursor".into());
        }
        if m.tasks_dir {
            parts.push("tasks".into());
        }
        if m.adr_count > 0 {
            if m.adr_recent_commits > 0 {
                parts.push(format!("ADR active({})", m.adr_recent_commits));
            } else {
                parts.push("ADR stale".into());
            }
        }
        if parts.is_empty() {
            "–".into()
        } else {
            parts.join(", ")
        }
    }
}

/// Count `.md` blobs in a repository tree path. Returns 0 on any error (404 is normal).
async fn count_md_files(client: &GitLabClient, project_id: u64, path: &str, per_page: &str) -> usize {
    let entries: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/tree"),
            &[("path", path), ("per_page", per_page)],
        )
        .await
        .unwrap_or_default();
    entries
        .iter()
        .filter(|e| {
            e["type"].as_str() == Some("blob")
                && e["name"].as_str().is_some_and(|n| n.ends_with(".md"))
        })
        .count()
}

/// Count skills under `.claude/skills`. Supports both layouts: flat `<name>.md`
/// files and the directory format `<name>/SKILL.md` (each subdirectory = one skill).
async fn count_skills(client: &GitLabClient, project_id: u64) -> usize {
    let entries: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/tree"),
            &[("path", ".claude/skills"), ("per_page", "100")],
        )
        .await
        .unwrap_or_default();
    entries
        .iter()
        .filter(|e| match e["type"].as_str() {
            Some("blob") => e["name"].as_str().is_some_and(|n| n.ends_with(".md")),
            Some("tree") => true, // directory-format skill
            _ => false,
        })
        .count()
}

/// Detect all AI adoption markers for a single repo. Never fails — a broken
/// repo just yields default (empty) markers so the group scan continues.
async fn scan_repo(
    client: &GitLabClient,
    project_id: u64,
    default_branch: &str,
    since: &str,
) -> RepoMarkers {
    let mut m = RepoMarkers::default();

    // 1. Root tree
    let root: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/tree"),
            &[("per_page", "100")],
        )
        .await
        .unwrap_or_default();

    let mut has_claude_dir = false;
    let mut need_docs_check = false;

    for entry in &root {
        let name = entry["name"].as_str().unwrap_or("");
        let is_tree = entry["type"].as_str() == Some("tree");
        match (name, is_tree) {
            ("CLAUDE.md", false) => m.claude_md = true,
            ("AGENTS.md", false) => m.agents_md = true,
            (".claude", true) => has_claude_dir = true,
            (".mcp.json", false) => m.mcp_json = true,
            (".cursorrules", false) | (".cursor", true) => m.cursor = true,
            (".windsurfrules", false) => m.cursor = true, // other AI assistant config
            (".tasks", true) => m.tasks_dir = true,
            ("docs", true) => need_docs_check = true,
            _ => {}
        }
    }

    // 2. .claude directory contents
    if has_claude_dir {
        let claude_tree: Vec<Value> = client
            .get(
                &format!("/projects/{project_id}/repository/tree"),
                &[("path", ".claude"), ("per_page", "100")],
            )
            .await
            .unwrap_or_default();

        let mut has_agents = false;
        let mut has_skills = false;
        for entry in &claude_tree {
            let name = entry["name"].as_str().unwrap_or("");
            let is_tree = entry["type"].as_str() == Some("tree");
            match (name, is_tree) {
                ("agents", true) => has_agents = true,
                ("skills", true) => has_skills = true,
                ("commands", true) => m.commands = true,
                ("hooks", true) => m.hooks = true,
                ("settings.json", false) => m.shared_settings = true,
                _ => {}
            }
        }

        if has_agents {
            m.agents_count = count_md_files(client, project_id, ".claude/agents", "100").await;
        }
        if has_skills {
            m.skills_count = count_skills(client, project_id).await;
        }
    }

    // 3. ADR practice — 404 is normal (no docs/adr dir), treated as 0
    if need_docs_check {
        m.adr_count = count_md_files(client, project_id, "docs/adr", "20").await;
    }

    // 4. CLAUDE.md size (quality check)
    if m.claude_md {
        let ref_name = if default_branch.is_empty() { "HEAD" } else { default_branch };
        let file: Option<Value> = client
            .get(
                &format!("/projects/{project_id}/repository/files/CLAUDE.md"),
                &[("ref", ref_name)],
            )
            .await
            .ok();
        m.claude_md_size = file
            .as_ref()
            .and_then(|f| f["size"].as_u64())
            .unwrap_or(0);
    }

    // 5. AI commit usage across ALL branches (one page is enough). Feature branches
    // carry trailers that squash-merge strips from the default branch.
    let commits: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/commits"),
            &[("since", since), ("per_page", "100"), ("all", "true")],
        )
        .await
        .unwrap_or_default();

    m.total_commits = commits.len();
    m.ai_commits = count_ai_commits(&commits);

    // 5b. Default-branch-only recount — detects squash-hidden usage. Only worth a
    // call when all-branch trailers exist at all.
    if m.ai_commits > 0 {
        let default_commits: Vec<Value> = client
            .get(
                &format!("/projects/{project_id}/repository/commits"),
                &[("since", since), ("per_page", "100")],
            )
            .await
            .unwrap_or_default();
        m.ai_commits_default = count_ai_commits(&default_commits);
    }

    // 6. Agent activity without attribution: recent commits touching .tasks / .claude.
    // Only queried for repos that have the marker — no extra calls for unmarked repos.
    if m.tasks_dir {
        m.tasks_recent_commits = count_path_commits(client, project_id, ".tasks", since).await;
    }
    if has_claude_dir {
        m.claude_recent_commits = count_path_commits(client, project_id, ".claude", since).await;
    }

    // 7. Branch radar: AI-named feature branches = in-flight work, even before any
    // config lands on the default branch. sort=updated_desc so recently active
    // branches land in the first page — alphabetical default buried real hits
    // below the per_page cutoff on branch-heavy repos.
    let branches: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/branches"),
            &[("per_page", "100"), ("sort", "updated_desc")],
        )
        .await
        .unwrap_or_default();
    m.branch_hits = branches
        .iter()
        .filter_map(|b| b["name"].as_str())
        .filter(|n| is_ai_branch(n))
        .take(3)
        .map(String::from)
        .collect();

    // 8. MR description scan — the most reliable usage signal: GitLab squash drops
    // commit trailers, but MR descriptions survive. Only for repos with any signal.
    if m.has_any_marker() || !m.branch_hits.is_empty() {
        let mrs: Vec<Value> = client
            .get(
                &format!("/projects/{project_id}/merge_requests"),
                &[("updated_after", since), ("state", "all"), ("per_page", "50")],
            )
            .await
            .unwrap_or_default();
        m.total_mr_count = mrs.len();
        m.ai_mr_count = mrs
            .iter()
            .filter(|mr| {
                let desc = mr["description"].as_str().unwrap_or("");
                let lower = desc.to_lowercase();
                lower.contains("generated with claude")
                    || lower.contains("co-authored-by")
                    || desc.contains("🤖")
            })
            .count();
    }

    // 9. ADR cadence — is the decision log alive or a one-time import?
    if m.adr_count > 0 {
        m.adr_recent_commits = count_path_commits(client, project_id, "docs/adr", since).await;
    }

    // 10. Config staleness — last commit ever touching CLAUDE.md (no since filter).
    if m.claude_md {
        let touches: Vec<Value> = client
            .get(
                &format!("/projects/{project_id}/repository/commits"),
                &[("path", "CLAUDE.md"), ("per_page", "1")],
            )
            .await
            .unwrap_or_default();
        m.claude_md_last_touch = touches
            .first()
            .and_then(|c| c["created_at"].as_str())
            .map(String::from);
        // Stale = last touch predates the scan window AND >= 30 commits in the window
        // (window count is only a lower bound on commits since the touch, hence "30+").
        let touched_before_window = m
            .claude_md_last_touch
            .as_deref()
            .and_then(|t| chrono::DateTime::parse_from_rfc3339(t).ok())
            .zip(chrono::DateTime::parse_from_rfc3339(since).ok())
            .map(|(touch, start)| touch < start)
            .unwrap_or(false);
        m.claude_md_stale = touched_before_window && m.total_commits >= 30;
    }

    m
}

/// Count commits whose message carries an AI co-author trailer.
fn count_ai_commits(commits: &[Value]) -> usize {
    commits
        .iter()
        .filter(|c| {
            let msg = c["message"].as_str().unwrap_or("");
            let lower = msg.to_lowercase();
            lower.contains("co-authored-by:")
                && (lower.contains("claude")
                    || lower.contains("copilot")
                    || lower.contains("cursor")
                    || msg.contains("AI"))
        })
        .count()
}

/// Count commits touching `path` since the window start. Returns 0 on any error.
async fn count_path_commits(
    client: &GitLabClient,
    project_id: u64,
    path: &str,
    since: &str,
) -> usize {
    let commits: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/commits"),
            &[("path", path), ("since", since), ("per_page", "20")],
        )
        .await
        .unwrap_or_default();
    commits.len()
}

/// Team = second path segment: `group/team/repo` → `team`; `group/repo` → `(root)`.
fn team_of(path_with_namespace: &str) -> String {
    let segments: Vec<&str> = path_with_namespace.split('/').collect();
    if segments.len() >= 3 {
        segments[1].to_string()
    } else {
        "(root)".to_string()
    }
}

/// A repo skipped as dormant — kept for archive-candidate visibility.
/// All fields come from the project listing; zero extra API calls.
#[derive(Debug, Clone)]
pub(crate) struct DormantRepo {
    /// path_with_namespace
    pub path: String,
    /// Same `team_of()` mapping as active repos.
    pub team: String,
    /// ISO date of last project activity (truncated to 10 chars for display).
    pub last_activity: String,
    /// Browser URL of the project (from the listing; empty when missing).
    pub web_url: String,
}

/// Dormant repos sorted oldest-first by last activity (unknown dates last).
pub(crate) fn sorted_dormant(dormant: &[DormantRepo]) -> Vec<&DormantRepo> {
    let mut sorted: Vec<&DormantRepo> = dormant.iter().collect();
    sorted.sort_by(|a, b| {
        (a.last_activity.is_empty(), &a.last_activity)
            .cmp(&(b.last_activity.is_empty(), &b.last_activity))
    });
    sorted
}

/// Per-team dormant repo counts.
pub(crate) fn dormant_by_team(dormant: &[DormantRepo]) -> BTreeMap<String, usize> {
    let mut map: BTreeMap<String, usize> = BTreeMap::new();
    for d in dormant {
        *map.entry(d.team.clone()).or_insert(0) += 1;
    }
    map
}

/// Result of a full group adoption scan — shared by the markdown scorecard
/// (`get_ai_adoption`) and the HTML report (`generate_ai_adoption_report`).
pub(crate) struct AdoptionScan {
    pub group: String,
    pub days: u32,
    /// Inactivity threshold (days) used for this scan.
    pub dormant_days: u32,
    /// Scanned repos with markers populated (preserves last_activity_at desc order).
    pub active: Vec<RepoResult>,
    /// Repos skipped as dormant (archive candidates).
    pub dormant: Vec<DormantRepo>,
}

impl AdoptionScan {
    pub(crate) fn dormant_count(&self) -> usize {
        self.dormant.len()
    }
}

/// Scan a GitLab group: list projects, skip dormant repos, detect AI adoption
/// markers per repo (10× concurrent). Pure data — no formatting.
pub(crate) async fn scan_group(
    client: &GitLabClient,
    group_path: &str,
    days: u32,
    dormant_days: u32,
) -> Result<AdoptionScan> {
    let encoded = urlencoding::encode(group_path);

    // Step 1: list group projects (max 3 pages = 300 repos)
    let projects: Vec<Value> = client
        .get_all_pages(
            &format!("/groups/{encoded}/projects"),
            &[
                ("include_subgroups", "true"),
                ("archived", "false"),
                ("order_by", "last_activity_at"),
                ("sort", "desc"),
            ],
            3,
        )
        .await?;

    if projects.is_empty() {
        return Ok(AdoptionScan {
            group: group_path.to_string(),
            days,
            dormant_days,
            active: Vec::new(),
            dormant: Vec::new(),
        });
    }

    let dormant_cutoff = chrono::Utc::now() - chrono::Duration::days(dormant_days as i64);
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    // Extract metadata, skip dormant repos (saves ~5 API calls per repo)
    struct ProjectMeta {
        id: u64,
        path: String,
        default_branch: String,
        last_activity: String,
        web_url: String,
    }

    let mut active: Vec<ProjectMeta> = Vec::new();
    let mut dormant: Vec<DormantRepo> = Vec::new();

    for p in &projects {
        let id = match p["id"].as_u64() {
            Some(id) => id,
            None => continue,
        };
        let path = p["path_with_namespace"].as_str().unwrap_or("?").to_string();
        let last_activity = p["last_activity_at"]
            .as_str()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok());
        let is_dormant = last_activity
            .map(|dt| dt < dormant_cutoff)
            .unwrap_or(false);
        let last_activity_date: String = p["last_activity_at"]
            .as_str()
            .map(|s| s.chars().take(10).collect())
            .unwrap_or_default();
        let web_url = p["web_url"].as_str().unwrap_or("").to_string();
        if is_dormant {
            dormant.push(DormantRepo {
                team: team_of(&path),
                path,
                last_activity: last_activity_date,
                web_url,
            });
            continue;
        }
        active.push(ProjectMeta {
            id,
            path,
            default_branch: p["default_branch"].as_str().unwrap_or("main").to_string(),
            last_activity: last_activity_date,
            web_url,
        });
    }

    // Step 2: per-repo marker detection, batched 10× concurrent
    let mut results: Vec<RepoResult> = Vec::new();
    for chunk in active.chunks(10) {
        let futs: Vec<_> = chunk
            .iter()
            .map(|meta| {
                let client = client.clone();
                let id = meta.id;
                let path = meta.path.clone();
                let default_branch = meta.default_branch.clone();
                let last_activity = meta.last_activity.clone();
                let web_url = meta.web_url.clone();
                let since = since.clone();
                async move {
                    let markers = scan_repo(&client, id, &default_branch, &since).await;
                    RepoResult {
                        team: team_of(&path),
                        path,
                        last_activity,
                        web_url,
                        default_branch,
                        markers,
                    }
                }
            })
            .collect();
        results.extend(join_all(futs).await);
    }

    Ok(AdoptionScan {
        group: group_path.to_string(),
        days,
        dormant_days,
        active: results,
        dormant,
    })
}

/// Scan a GitLab group for AI-assisted development adoption markers and
/// return a per-team scorecard.
pub async fn get_ai_adoption(
    client: &GitLabClient,
    group_path: &str,
    days: u32,
    dormant_days: u32,
    summary_only: bool,
) -> Result<String> {
    let scan = scan_group(client, group_path, days, dormant_days).await?;

    if scan.active.is_empty() {
        if scan.dormant_count() == 0 {
            return Ok(format!("No projects found in group '{group_path}'."));
        }
        return Ok(format!(
            "Group '{group_path}': all {} repos dormant (no activity in {dormant_days}d). Nothing to scan.",
            scan.dormant_count()
        ));
    }

    let dormant_repos = scan.dormant;
    let results = scan.active;
    let dormant = dormant_repos.len();
    let active_count = results.len();

    // In-flight = ONLY signal is AI-named branches (no config markers yet)
    let in_flight: Vec<&RepoResult> = results
        .iter()
        .filter(|r| !r.markers.has_any_marker() && !r.markers.branch_hits.is_empty())
        .collect();

    // Invisible usage = AI-trailed commits but ZERO config markers. Devs adopted
    // Claude on their own; the repo gives it no context. Sorted heaviest first.
    let mut invisible: Vec<&RepoResult> = results
        .iter()
        .filter(|r| !r.markers.has_any_marker() && r.markers.ai_commits > 0)
        .collect();
    invisible.sort_by(|a, b| {
        b.markers
            .ai_pct()
            .partial_cmp(&a.markers.ai_pct())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    // Step 4: aggregate per team
    struct TeamStats {
        repos: usize,
        with_markers: usize,
        best_level: u8,
        adopting_ai_pcts: Vec<f64>,
        dormant: usize,
    }

    let mut teams: BTreeMap<String, TeamStats> = BTreeMap::new();
    for r in &results {
        let level = r.level();
        let stats = teams.entry(r.team.clone()).or_insert(TeamStats {
            repos: 0,
            with_markers: 0,
            best_level: 0,
            adopting_ai_pcts: Vec::new(),
            dormant: 0,
        });
        stats.repos += 1;
        if level >= 1 {
            stats.with_markers += 1;
            stats.adopting_ai_pcts.push(r.markers.ai_pct());
        }
        if level > stats.best_level {
            stats.best_level = level;
        }
    }

    // Step 5: output
    if summary_only {
        let team_parts: Vec<String> = teams
            .iter()
            .map(|(name, s)| {
                format!("{name}: {}/{} adopting (best L{})", s.with_markers, s.repos, s.best_level)
            })
            .collect();
        let in_flight_part = if in_flight.is_empty() {
            String::new()
        } else {
            format!(" | {} in-flight", in_flight.len())
        };
        return Ok(format!(
            "{group_path}: {active_count} repos scanned, {dormant} dormant. {}{in_flight_part}",
            team_parts.join(" | ")
        ));
    }

    // Fold dormant repos into the team table (after summary_only, which only
    // reports active teams). Teams with ONLY dormant repos still get a row.
    for (team, count) in dormant_by_team(&dormant_repos) {
        teams
            .entry(team)
            .or_insert(TeamStats {
                repos: 0,
                with_markers: 0,
                best_level: 0,
                adopting_ai_pcts: Vec::new(),
                dormant: 0,
            })
            .dormant = count;
    }

    let mut out = vec![
        format!(
            "## AI Adoption: {group_path} (last {days}d, {active_count} active repos, {dormant} dormant skipped)"
        ),
        String::new(),
        "### By Team".to_string(),
        String::new(),
        "| Team | Repos | With markers | Best level | AI commits % (avg of adopting) | Dormant |".to_string(),
        "|------|-------|--------------|-----------|--------------------------------|---------|".to_string(),
    ];

    for (name, s) in &teams {
        let avg_pct = if s.adopting_ai_pcts.is_empty() {
            "–".to_string()
        } else {
            format!(
                "{:.0}%",
                s.adopting_ai_pcts.iter().sum::<f64>() / s.adopting_ai_pcts.len() as f64
            )
        };
        out.push(format!(
            "| {name} | {} | {} | L{} | {avg_pct} | {} |",
            s.repos, s.with_markers, s.best_level, s.dormant
        ));
    }

    // Adopting repos table: level desc, then ai_pct desc, top 25
    let mut adopting: Vec<&RepoResult> = results.iter().filter(|r| r.level() >= 1).collect();
    adopting.sort_by(|a, b| {
        b.level()
            .cmp(&a.level())
            .then(b.markers.ai_pct().partial_cmp(&a.markers.ai_pct()).unwrap_or(std::cmp::Ordering::Equal))
    });

    out.push(String::new());
    out.push("### Adopting Repos".to_string());
    out.push(String::new());

    if adopting.is_empty() {
        out.push("No repos with AI adoption markers found.".to_string());
    } else {
        out.push("| Repo | Level | Traj | Markers | AI commits | Flags |".to_string());
        out.push("|------|-------|------|---------|-----------|-------|".to_string());

        let shown = adopting.len().min(25);
        for r in adopting.iter().take(25) {
            let m = &r.markers;
            let mut ai_str = if m.total_commits == 0 {
                "0%".to_string()
            } else {
                format!("{:.0}% ({}/{})", m.ai_pct(), m.ai_commits, m.total_commits)
            };
            if m.ai_mr_count > 0 {
                // Squash-proof signal: AI markers in MR descriptions
                ai_str.push_str(&format!(" +{} MRs", m.ai_mr_count));
            }
            if m.tasks_recent_commits > 0 {
                // Agent activity visible even when attribution is missing
                ai_str.push_str(&format!(" +{} task commits", m.tasks_recent_commits));
            }
            let flags = quality_flags(m);
            let flags_str = if flags.is_empty() {
                "–".to_string()
            } else {
                flags.join(", ")
            };
            // Drop the group prefix for readability
            let short_path = r
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&r.path);
            out.push(format!(
                "| {short_path} | L{} | {} | {} | {ai_str} | {flags_str} |",
                r.level(),
                trajectory(m),
                r.marker_list()
            ));
        }
        if adopting.len() > shown {
            out.push(format!(
                "\n*Showing top {shown} of {} adopting repos.*",
                adopting.len()
            ));
        }
    }

    // In-flight section: AI work on feature branches before any config lands
    if !in_flight.is_empty() {
        out.push(String::new());
        out.push("### In-flight (branch signals only)".to_string());
        out.push(String::new());
        out.push("| Repo | Branch | Last activity |".to_string());
        out.push("|------|--------|---------------|".to_string());
        for r in &in_flight {
            let short_path = r
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&r.path);
            let branch = r.markers.branch_hits.first().map(String::as_str).unwrap_or("?");
            let last = if r.last_activity.is_empty() { "–" } else { &r.last_activity };
            out.push(format!("| {short_path} | {branch} | {last} |"));
        }
    }

    // Invisible usage section: heavy AI users with zero config
    if !invisible.is_empty() {
        out.push(String::new());
        out.push("### Invisible usage (no config)".to_string());
        out.push(String::new());
        out.push("Devs adopted Claude on their own — the repo gives it no context. Cheapest win: add a CLAUDE.md.".to_string());
        out.push(String::new());
        out.push("| Repo | AI commits | Attribution |".to_string());
        out.push("|------|-----------|-------------|".to_string());
        for r in &invisible {
            let short_path = r
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&r.path);
            let m = &r.markers;
            let attribution = if m.ai_commits_default == 0 {
                "squash-hidden (branches only)"
            } else {
                "visible on default"
            };
            out.push(format!(
                "| {short_path} | {:.0}% ({}/{}) | {attribution} |",
                m.ai_pct(),
                m.ai_commits,
                m.total_commits
            ));
        }
    }

    // Quality flags section
    let mut flag_lines: Vec<String> = Vec::new();
    for r in &results {
        let m = &r.markers;
        for flag in quality_flags(m) {
            match flag.as_str() {
                "bloated CLAUDE.md" => flag_lines.push(format!(
                    "- {}: bloated CLAUDE.md ({}KB) — trim to essentials",
                    r.path,
                    m.claude_md_size / 1024
                )),
                "stub CLAUDE.md" => flag_lines.push(format!(
                    "- {}: stub CLAUDE.md ({}B) — add real project context",
                    r.path, m.claude_md_size
                )),
                "setup unused" => flag_lines.push(format!(
                    "- {}: setup unused — .claude/agents present, 0 AI commits in {days}d",
                    r.path
                )),
                "no attribution" => flag_lines.push(format!(
                    "- {}: no attribution — {} .tasks / {} .claude commits in {days}d but 0 AI-trailed commits — enable Co-Authored-By attribution for measurable adoption",
                    r.path, m.tasks_recent_commits, m.claude_recent_commits
                )),
                "usage w/o config" => flag_lines.push(format!(
                    "- {}: usage w/o config — {:.0}% AI commits but no CLAUDE.md (add one)",
                    r.path,
                    m.ai_pct()
                )),
                "squash-hidden usage" => flag_lines.push(format!(
                    "- {}: squash-hidden usage — {} AI-trailed commits on feature branches, 0 on default — squash strips attribution at merge",
                    r.path, m.ai_commits
                )),
                "stale config (30+ commits behind)" => flag_lines.push(format!(
                    "- {}: stale config — CLAUDE.md last touched {} but {}+ commits since — refresh it",
                    r.path,
                    m.claude_md_last_touch
                        .as_deref()
                        .map(|t| &t[..t.len().min(10)])
                        .unwrap_or("?"),
                    m.total_commits
                )),
                f if f.starts_with("in-flight") => flag_lines.push(format!(
                    "- {}: {f} — AI work on feature branches, no config on default yet",
                    r.path
                )),
                _ => {}
            }
        }
    }
    if !flag_lines.is_empty() {
        out.push(String::new());
        out.push("### Quality Flags".to_string());
        out.extend(flag_lines);
    }

    // Recommendations
    let mut recs: Vec<String> = Vec::new();
    for (name, s) in &teams {
        if s.with_markers == 0 && s.repos > 0 {
            // Pilot candidate: most recently active repo of this team
            // (results preserve the API's last_activity_at desc ordering)
            let pilot = results
                .iter()
                .find(|r| &r.team == name)
                .map(|r| r.path.as_str())
                .unwrap_or("?");
            recs.push(format!(
                "- {name} team: 0 adoption across {} active repos — pilot candidate: {pilot}",
                s.repos
            ));
        }
    }
    let no_config_count = results
        .iter()
        .filter(|r| has_flag(&quality_flags(&r.markers), "usage w/o config"))
        .count();
    if no_config_count > 0 {
        recs.push(format!(
            "- {no_config_count} repos have AI commits but no CLAUDE.md — quick win: add one"
        ));
    }
    let no_attribution_count = results
        .iter()
        .filter(|r| has_flag(&quality_flags(&r.markers), "no attribution"))
        .count();
    if no_attribution_count > 0 {
        recs.push(format!(
            "- {no_attribution_count} repos show agent activity without commit attribution — standardize Co-Authored-By trailers to measure adoption"
        ));
    }
    let squash_hidden_count = results
        .iter()
        .filter(|r| has_flag(&quality_flags(&r.markers), "squash-hidden usage"))
        .count();
    if squash_hidden_count > 0 {
        recs.push(format!(
            "- {squash_hidden_count} repos lose attribution at merge — check squash settings or rely on MR descriptions"
        ));
    }
    if !in_flight.is_empty() {
        recs.push(format!(
            "- {} repos have AI work on feature branches — adoption pipeline",
            in_flight.len()
        ));
    }
    if !recs.is_empty() {
        out.push(String::new());
        out.push("### Recommendations".to_string());
        out.extend(recs);
    }

    // Dormant repos: archive candidates, oldest first
    if !dormant_repos.is_empty() {
        let sorted = sorted_dormant(&dormant_repos);
        out.push(String::new());
        out.push("### Dormant repos (archive candidates)".to_string());
        out.push(String::new());
        out.push(format!(
            "Inactive {dormant_days}+ days and not archived — consider archiving to reduce noise."
        ));
        out.push(String::new());
        out.push("| Repo | Team | Last activity |".to_string());
        out.push("|------|------|---------------|".to_string());
        let shown = sorted.len().min(20);
        for d in sorted.iter().take(20) {
            let short_path = d
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&d.path);
            let last = if d.last_activity.is_empty() { "–" } else { &d.last_activity };
            out.push(format!("| {short_path} | {} | {last} |", d.team));
        }
        if sorted.len() > shown {
            out.push(format!("\n*+{} more dormant repos.*", sorted.len() - shown));
        }
    }

    Ok(out.join("\n"))
}

/// Attribution rate: among adopting repos that show active usage, the share
/// whose usage is trailer-visible (`ai_commits > 0`). `None` when no adopting
/// repo has usage evidence at all — there is nothing to attribute.
pub(crate) fn attribution_rate(adopting: &[&RepoMarkers]) -> Option<f64> {
    let with_usage: Vec<&&RepoMarkers> =
        adopting.iter().filter(|m| has_active_usage(m)).collect();
    if with_usage.is_empty() {
        return None;
    }
    let visible = with_usage.iter().filter(|m| m.ai_commits > 0).count();
    Some(visible as f64 / with_usage.len() as f64 * 100.0)
}

/// Wrap `text` in an anchor when `url` is non-empty. `text` is pre-escaped by
/// callers; the URL is escaped here. HTML report only — markdown stays link-free.
fn link(url: &str, text: &str) -> String {
    if url.is_empty() {
        return text.to_string();
    }
    format!(
        "<a href=\"{}\">{}</a>",
        crate::tools::reports::htmlescape(url),
        text
    )
}

/// In-document anchor link (`#fragment` targets). Same contract as `link`:
/// `text` is pre-escaped by callers. HTML report only.
fn anchor(href: &str, text: &str) -> String {
    link(href, text)
}

/// `{web_url}{suffix}`, or empty when the project has no web_url (→ no link).
fn sub_url(web_url: &str, suffix: &str) -> String {
    if web_url.is_empty() {
        String::new()
    } else {
        format!("{web_url}{suffix}")
    }
}

/// Linked HTML version of `RepoResult::marker_list()` — each marker points at
/// the file/directory it was detected from. Same order and labels as the
/// plain-text list used by the markdown scorecard.
fn markers_html(m: &RepoMarkers, web_url: &str, default_branch: &str) -> String {
    let blob = |path: &str| sub_url(web_url, &format!("/-/blob/{default_branch}/{path}"));
    let tree = |path: &str| sub_url(web_url, &format!("/-/tree/{default_branch}/{path}"));
    let mut parts: Vec<String> = Vec::new();
    if m.claude_md {
        parts.push(link(&blob("CLAUDE.md"), "CLAUDE.md"));
    }
    if m.agents_md {
        parts.push(link(&blob("AGENTS.md"), "AGENTS.md"));
    }
    if m.agents_count > 0 {
        parts.push(link(&tree(".claude/agents"), &format!("agents({})", m.agents_count)));
    }
    if m.skills_count > 0 {
        parts.push(link(&tree(".claude/skills"), &format!("skills({})", m.skills_count)));
    }
    if m.commands {
        parts.push(link(&tree(".claude/commands"), "commands"));
    }
    if m.shared_settings {
        parts.push(link(&blob(".claude/settings.json"), "settings"));
    }
    if m.hooks {
        parts.push(link(&tree(".claude/hooks"), "hooks"));
    }
    if m.mcp_json {
        parts.push(link(&blob(".mcp.json"), ".mcp.json"));
    }
    if m.cursor {
        // .cursorrules / .cursor / .windsurfrules — source ambiguous, no link
        parts.push("cursor".to_string());
    }
    if m.tasks_dir {
        parts.push(link(&tree(".tasks"), "tasks"));
    }
    if m.adr_count > 0 {
        let label = if m.adr_recent_commits > 0 {
            format!("ADR active({})", m.adr_recent_commits)
        } else {
            "ADR stale".to_string()
        };
        parts.push(link(&tree("docs/adr"), &label));
    }
    if parts.is_empty() {
        "&ndash;".to_string()
    } else {
        parts.join(", ")
    }
}

/// Generate a townhall-ready HTML AI adoption report for a GitLab group:
/// level funnel, per-team scorecard, trajectories, in-flight pipeline,
/// quality flags, and recommendations. Dark theme, print/PDF-friendly.
pub async fn generate_ai_adoption_report(
    client: &GitLabClient,
    group_path: &str,
    days: u32,
    dormant_days: u32,
) -> Result<String> {
    use crate::tools::reports::{htmlescape as esc, EXPORT_BUTTON, PRINT_CSS};

    let scan = scan_group(client, group_path, days, dormant_days).await?;

    if scan.active.is_empty() {
        if scan.dormant_count() == 0 {
            return Ok(format!("No projects found in group '{group_path}'."));
        }
        return Ok(format!(
            "Group '{group_path}': all {} repos dormant (no activity in {dormant_days}d). Nothing to scan.",
            scan.dormant_count()
        ));
    }

    // The scan carries its own identity — single source of truth from here on.
    let group_path = scan.group.as_str();
    let days = scan.days;
    let dormant_days = scan.dormant_days;
    let results = &scan.active;
    let active_count = results.len();
    let dormant = scan.dormant_count();
    let date_str = chrono::Utc::now().format("%A, %d %B %Y").to_string();

    // ── Aggregates ──

    let mut level_counts = [0usize; 4];
    for r in results {
        level_counts[r.level() as usize] += 1;
    }
    let in_flight: Vec<&RepoResult> = results
        .iter()
        .filter(|r| !r.markers.has_any_marker() && !r.markers.branch_hits.is_empty())
        .collect();
    // In-flight repos are L0 by level — split them out of the L0 bucket so the
    // funnel rows are disjoint.
    let l0_plain = level_counts[0] - in_flight.len();
    let adopting_count = level_counts[1] + level_counts[2] + level_counts[3];

    // Invisible usage = AI-trailed commits but zero config markers, heaviest first.
    let mut invisible: Vec<&RepoResult> = results
        .iter()
        .filter(|r| !r.markers.has_any_marker() && r.markers.ai_commits > 0)
        .collect();
    invisible.sort_by(|a, b| {
        b.markers
            .ai_pct()
            .partial_cmp(&a.markers.ai_pct())
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let adopting_markers: Vec<&RepoMarkers> = results
        .iter()
        .filter(|r| r.level() >= 1)
        .map(|r| &r.markers)
        .collect();
    let attr_rate = attribution_rate(&adopting_markers);
    let attr_str = attr_rate
        .map(|p| format!("{p:.0}%"))
        .unwrap_or_else(|| "&ndash;".to_string());

    // Per-team aggregation
    #[derive(Default)]
    struct TeamStats {
        repos: usize,
        adopting: usize,
        best_level: u8,
        up: usize,
        steady: usize,
        down: usize,
        adopting_ai_pcts: Vec<f64>,
        dormant: usize,
    }
    let mut teams: BTreeMap<String, TeamStats> = BTreeMap::new();
    for r in results {
        let stats = teams.entry(r.team.clone()).or_default();
        stats.repos += 1;
        let level = r.level();
        if level >= 1 {
            stats.adopting += 1;
            stats.adopting_ai_pcts.push(r.markers.ai_pct());
        }
        if level > stats.best_level {
            stats.best_level = level;
        }
        match trajectory(&r.markers) {
            "↑" => stats.up += 1,
            "→" => stats.steady += 1,
            "↓" => stats.down += 1,
            _ => {}
        }
    }
    // Teams with ONLY dormant repos still get a row (0 active, N dormant).
    for (team, count) in dormant_by_team(&scan.dormant) {
        teams.entry(team).or_default().dormant = count;
    }

    // ── HTML head + summary cards ──

    let group_esc = esc(group_path);
    let adopting_pct = adopting_count as f64 / active_count as f64 * 100.0;

    // "N dormant skipped" links to the dormant details only when that section
    // renders (non-empty); same string is reused in the Active Repos card.
    let dormant_skipped = if dormant > 0 {
        anchor("#dormant", &format!("{dormant} dormant skipped"))
    } else {
        format!("{dormant} dormant skipped")
    };
    // The In-flight section only renders when non-empty — don't link to a
    // missing anchor. The other card targets (by-team, adopting, methodology)
    // always render.
    let in_flight_card = if in_flight.is_empty() {
        in_flight.len().to_string()
    } else {
        anchor("#in-flight", &in_flight.len().to_string())
    };

    let mut html = format!(
        r##"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>AI Adoption Report — {group_esc} — {date_str}</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#0d1117;color:#c9d1d9;padding:32px;line-height:1.6}}
h1{{color:#58a6ff;margin-bottom:8px;font-size:24px}}
h2{{color:#58a6ff;margin:36px 0 16px;font-size:18px;border-bottom:1px solid #21262d;padding-bottom:8px}}
.sub{{color:#8b949e;margin-bottom:24px;font-size:14px}}
.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:14px;margin:16px 0}}
.card{{background:#161b22;border:1px solid #21262d;border-radius:8px;padding:18px}}
.card-t{{color:#8b949e;font-size:11px;text-transform:uppercase;letter-spacing:1px;margin-bottom:6px}}
.card-v{{font-size:28px;font-weight:700}}
.card-s{{color:#8b949e;font-size:12px;margin-top:4px}}
.g{{color:#3fb950}}.r{{color:#f85149}}.y{{color:#d29922}}.b{{color:#58a6ff}}.gr{{color:#8b949e}}
table{{width:100%;border-collapse:collapse;margin:12px 0}}
th{{background:#161b22;color:#8b949e;text-align:left;padding:10px 14px;font-size:11px;text-transform:uppercase;letter-spacing:.5px;border-bottom:2px solid #21262d}}
td{{padding:10px 14px;border-bottom:1px solid #21262d;font-size:14px}}
.issue{{background:#161b22;border:1px solid #21262d;border-radius:6px;padding:14px 18px;margin:8px 0}}
.issue b{{font-weight:600}}.issue .m{{color:#8b949e;font-size:13px;margin-top:4px}}
.risk{{border-left:3px solid #f85149}}.warn{{border-left:3px solid #d29922}}.ok{{border-left:3px solid #3fb950}}
.bar{{height:10px;border-radius:4px;display:inline-block;vertical-align:middle;min-width:4px}}
code{{background:#21262d;padding:1px 6px;border-radius:4px;font-size:13px}}
a{{color:inherit;text-decoration:none;border-bottom:1px dotted #58a6ff}}
a:hover{{color:#58a6ff}}
details{{margin:24px 0;color:#8b949e;font-size:13px}}
details summary{{cursor:pointer;color:#58a6ff}}
details p{{margin-top:8px;max-width:900px}}
footer{{margin-top:48px;padding-top:16px;border-top:1px solid #21262d;color:#484f58;font-size:12px}}
{PRINT_CSS}
@media print{{a{{border-bottom:none !important;color:inherit !important}}}}
</style>
</head>
<body>
{EXPORT_BUTTON}
<script>
function openTarget(){{var el=document.getElementById(location.hash.slice(1));while(el){{if(el.tagName==='DETAILS'){{el.open=true;break}}el=el.parentElement}}}}
window.addEventListener('hashchange',openTarget);openTarget();
</script>

<h1>AI Adoption Report — {group_esc}</h1>
<div class="sub">Last {days} days &middot; {date_str} &middot; {active_count} active repos scanned, {dormant_skipped}</div>

<!-- Summary Cards -->
<div class="grid">
  <div class="card"><div class="card-t">Active Repos</div><div class="card-v b"><a href="#by-team">{active_count}</a></div><div class="card-s">{dormant_skipped}</div></div>
  <div class="card"><div class="card-t">Adopting (L1+)</div><div class="card-v g"><a href="#adopting">{adopting_count}</a></div><div class="card-s">{adopting_pct:.0}% of active</div></div>
  <div class="card"><div class="card-t">In-flight</div><div class="card-v y">{in_flight_card}</div><div class="card-s">branch signals only</div></div>
  <div class="card"><div class="card-t">Scaling (L3)</div><div class="card-v g"><a href="#adopting">{l3}</a></div><div class="card-s">agents + active usage</div></div>
  <div class="card"><div class="card-t">Attribution Rate</div><div class="card-v{attr_class}"><a href="#methodology">{attr_str}</a></div><div class="card-s">usage visible via commit trailers</div></div>
</div>
"##,
        l3 = level_counts[3],
        attr_class = match attr_rate {
            Some(p) if p < 50.0 => " y",
            Some(_) => " g",
            None => "",
        },
    );

    // ── Level Funnel ──

    html.push_str("<h2>Adoption Levels</h2>\n");
    let funnel = [
        ("L3 Scaling", level_counts[3], "#3fb950", "#adopting"),
        ("L2 Practicing", level_counts[2], "#58a6ff", "#adopting"),
        ("L1 Exploring", level_counts[1], "#d29922", "#adopting"),
        ("In-flight", in_flight.len(), "#8b949e", "#in-flight"),
        ("L0 None", l0_plain, "#f85149", "#by-team"),
    ];
    let max_count = funnel.iter().map(|(_, c, _, _)| *c).max().unwrap_or(1).max(1);
    let bar_max = 300usize;
    for (label, count, color, target) in funnel {
        let width = count * bar_max / max_count;
        let pct = count as f64 / active_count as f64 * 100.0;
        let count_text = format!("{count} ({pct:.0}%)");
        // Empty rows have no evidence to jump to — leave them unlinked.
        let count_html = if count > 0 {
            anchor(target, &count_text)
        } else {
            count_text
        };
        html.push_str(&format!(
            "<div style=\"margin:6px 0;display:flex;align-items:center;gap:10px\"><span style=\"width:110px;font-weight:700;color:{color}\">{label}</span><span class=\"bar\" style=\"width:{width}px;background:{color}\"></span><span style=\"color:#8b949e;font-size:13px\">{count_html}</span></div>\n"
        ));
    }

    // ── By Team ──

    // Instance origin (scheme://host) for team group links — derived from any
    // project's web_url; teams stay unlinked when no host is known.
    let origin: Option<String> = results
        .iter()
        .map(|r| r.web_url.as_str())
        .chain(scan.dormant.iter().map(|d| d.web_url.as_str()))
        .filter(|u| !u.is_empty())
        .find_map(|u| {
            let parsed = url::Url::parse(u).ok()?;
            let host = parsed.host_str()?.to_string();
            Some(format!("{}://{host}", parsed.scheme()))
        });

    html.push_str("<h2 id=\"by-team\">By Team</h2>\n<table>\n<tr><th>Team</th><th>Repos</th><th>Adopting</th><th>Best Level</th><th>Trajectory</th><th>AI-visible Usage</th><th>Dormant</th></tr>\n");
    for (name, s) in &teams {
        let team_url = match &origin {
            Some(o) if name != "(root)" => format!("{o}/{group_path}/{name}"),
            _ => String::new(),
        };
        let level_class = match s.best_level {
            3 => "g",
            2 => "b",
            1 => "y",
            _ => "r",
        };
        let mut traj_parts: Vec<String> = Vec::new();
        if s.up > 0 {
            traj_parts.push(format!("<span class=\"g\">{}&uarr;</span>", s.up));
        }
        if s.steady > 0 {
            traj_parts.push(format!("<span class=\"gr\">{}&rarr;</span>", s.steady));
        }
        if s.down > 0 {
            traj_parts.push(format!("<span class=\"r\">{}&darr;</span>", s.down));
        }
        let traj_str = if traj_parts.is_empty() {
            "&ndash;".to_string()
        } else {
            traj_parts.join(" ")
        };
        let avg_pct = if s.adopting_ai_pcts.is_empty() {
            "&ndash;".to_string()
        } else {
            format!(
                "{:.0}%",
                s.adopting_ai_pcts.iter().sum::<f64>() / s.adopting_ai_pcts.len() as f64
            )
        };
        // Non-zero counts jump to their evidence section; zeros stay plain.
        let adopting_cell = if s.adopting > 0 {
            anchor("#adopting", &s.adopting.to_string())
        } else {
            s.adopting.to_string()
        };
        let dormant_cell = if s.dormant > 0 {
            anchor("#dormant", &s.dormant.to_string())
        } else {
            s.dormant.to_string()
        };
        html.push_str(&format!(
            "<tr><td><b>{}</b></td><td>{}</td><td>{adopting_cell}</td><td class=\"{level_class}\"><b>L{}</b></td><td>{traj_str}</td><td>{avg_pct}</td><td>{dormant_cell}</td></tr>\n",
            link(&team_url, &esc(name)),
            s.repos,
            s.best_level,
        ));
    }
    html.push_str("</table>\n");

    // ── Adopting Repos ──

    let mut adopting: Vec<&RepoResult> = results.iter().filter(|r| r.level() >= 1).collect();
    adopting.sort_by(|a, b| {
        b.level().cmp(&a.level()).then(
            b.markers
                .ai_pct()
                .partial_cmp(&a.markers.ai_pct())
                .unwrap_or(std::cmp::Ordering::Equal),
        )
    });

    html.push_str("<h2 id=\"adopting\">Adopting Repos</h2>\n");
    if adopting.is_empty() {
        html.push_str("<p class=\"sub\">No repos with AI adoption markers found.</p>\n");
    } else {
        html.push_str("<table>\n<tr><th>Repo</th><th>Level</th><th>Traj</th><th>Markers</th><th>Usage</th><th>Flags</th></tr>\n");
        for r in &adopting {
            let m = &r.markers;
            let level = r.level();
            let level_class = match level {
                3 => "g",
                2 => "b",
                _ => "y",
            };
            let traj_cell = match trajectory(m) {
                "↑" => "<span class=\"g\">&uarr;</span>",
                "↓" => "<span class=\"r\">&darr;</span>",
                "→" => "<span class=\"gr\">&rarr;</span>",
                _ => "",
            };
            let usage_base = if m.total_commits == 0 {
                "0%".to_string()
            } else {
                format!("{:.0}% ({}/{})", m.ai_pct(), m.ai_commits, m.total_commits)
            };
            let commits_url =
                sub_url(&r.web_url, &format!("/-/commits/{}", r.default_branch));
            let mut usage = link(&commits_url, &usage_base);
            if m.ai_mr_count > 0 {
                let mr_url = sub_url(&r.web_url, "/-/merge_requests?scope=all&state=merged");
                usage.push(' ');
                usage.push_str(&link(&mr_url, &format!("+{} MRs", m.ai_mr_count)));
            }
            if m.tasks_recent_commits > 0 {
                // Path-scoped commit history: only commits touching .tasks.
                let tasks_url = sub_url(
                    &r.web_url,
                    &format!("/-/commits/{}/.tasks", r.default_branch),
                );
                usage.push(' ');
                usage.push_str(&link(
                    &tasks_url,
                    &format!("+{} task commits", m.tasks_recent_commits),
                ));
            }
            let flags = quality_flags(m);
            let flags_str = if flags.is_empty() {
                "&ndash;".to_string()
            } else {
                esc(&flags.join(", "))
            };
            let short_path = r
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&r.path);
            html.push_str(&format!(
                "<tr><td><b>{}</b></td><td class=\"{level_class}\"><b>L{level}</b></td><td>{traj_cell}</td><td>{}</td><td>{usage}</td><td>{flags_str}</td></tr>\n",
                link(&r.web_url, &esc(short_path)),
                markers_html(m, &r.web_url, &r.default_branch),
            ));
        }
        html.push_str("</table>\n");
    }

    // ── In-flight pipeline ──

    if !in_flight.is_empty() {
        html.push_str("<h2 id=\"in-flight\">In-flight (adoption pipeline)</h2>\n");
        html.push_str("<p class=\"sub\">AI work happening on feature branches — no config on the default branch yet.</p>\n");
        for r in &in_flight {
            let short_path = r
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&r.path);
            let branch = r
                .markers
                .branch_hits
                .first()
                .map(String::as_str)
                .unwrap_or("?");
            let last = if r.last_activity.is_empty() {
                "&ndash;".to_string()
            } else {
                esc(&r.last_activity)
            };
            let branch_url = sub_url(
                &r.web_url,
                &format!("/-/tree/{}", urlencoding::encode(branch)),
            );
            html.push_str(&format!(
                "<div class=\"issue warn\"><b>{}</b> &mdash; branch <code>{}</code><div class=\"m\">Last activity {last} &middot; adoption pipeline: AI work in flight, expect config to land on default.</div></div>\n",
                link(&r.web_url, &esc(short_path)),
                link(&branch_url, &esc(branch)),
            ));
        }
    }

    // ── Invisible usage: heavy AI users with zero config ──

    if !invisible.is_empty() {
        html.push_str("<h2 id=\"invisible\">Invisible usage (no config)</h2>\n");
        html.push_str("<p class=\"sub\">Devs adopted Claude on their own &mdash; the repo gives it no context. Cheapest win: add a CLAUDE.md.</p>\n");
        html.push_str("<table>\n<tr><th>Repo</th><th>AI Commits</th><th>Attribution</th></tr>\n");
        for r in &invisible {
            let short_path = r
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&r.path);
            let m = &r.markers;
            let attribution = if m.ai_commits_default == 0 {
                "<span class=\"y\">squash-hidden (branches only)</span>"
            } else {
                "<span class=\"g\">visible on default</span>"
            };
            html.push_str(&format!(
                "<tr><td><b>{}</b></td><td>{:.0}% ({}/{})</td><td>{attribution}</td></tr>\n",
                link(&r.web_url, &esc(short_path)),
                m.ai_pct(),
                m.ai_commits,
                m.total_commits
            ));
        }
        html.push_str("</table>\n");
    }

    // ── Quality Flags ──

    let mut flag_boxes: Vec<String> = Vec::new();
    for r in results.iter() {
        let m = &r.markers;
        let repo = link(&r.web_url, &esc(&r.path));
        for flag in quality_flags(m) {
            match flag.as_str() {
                "stale config (30+ commits behind)" => flag_boxes.push(format!(
                    "<div class=\"issue risk\"><b>{repo} &mdash; stale config</b><div class=\"m\">CLAUDE.md last touched {} but {}+ commits since &mdash; refresh it.</div></div>\n",
                    esc(m.claude_md_last_touch.as_deref().map(|t| &t[..t.len().min(10)]).unwrap_or("?")),
                    m.total_commits,
                )),
                "setup unused" => flag_boxes.push(format!(
                    "<div class=\"issue risk\"><b>{repo} &mdash; setup unused</b><div class=\"m\">.claude/agents present, 0 AI commits in {days}d.</div></div>\n"
                )),
                "no attribution" => flag_boxes.push(format!(
                    "<div class=\"issue warn\"><b>{repo} &mdash; no attribution</b><div class=\"m\">{} .tasks / {} .claude commits in {days}d but 0 AI-trailed commits &mdash; enable Co-Authored-By attribution for measurable adoption.</div></div>\n",
                    m.tasks_recent_commits, m.claude_recent_commits,
                )),
                "squash-hidden usage" => flag_boxes.push(format!(
                    "<div class=\"issue warn\"><b>{repo} &mdash; squash-hidden usage</b><div class=\"m\">{} AI-trailed commits on feature branches, 0 on default &mdash; squash strips attribution at merge.</div></div>\n",
                    m.ai_commits,
                )),
                "stub CLAUDE.md" => flag_boxes.push(format!(
                    "<div class=\"issue warn\"><b>{repo} &mdash; stub CLAUDE.md</b><div class=\"m\">{}B &mdash; add real project context.</div></div>\n",
                    m.claude_md_size,
                )),
                "bloated CLAUDE.md" => flag_boxes.push(format!(
                    "<div class=\"issue warn\"><b>{repo} &mdash; bloated CLAUDE.md</b><div class=\"m\">{}KB &mdash; trim to essentials.</div></div>\n",
                    m.claude_md_size / 1024,
                )),
                "usage w/o config" => flag_boxes.push(format!(
                    "<div class=\"issue warn\"><b>{repo} &mdash; usage without config</b><div class=\"m\">{:.0}% AI commits but no CLAUDE.md &mdash; add one.</div></div>\n",
                    m.ai_pct(),
                )),
                // in-flight flags have their own section above
                _ => {}
            }
        }
    }
    if !flag_boxes.is_empty() {
        html.push_str("<h2 id=\"flags\">Quality Flags</h2>\n");
        for b in &flag_boxes {
            html.push_str(b);
        }
    }

    // ── Recommendations (same logic as the markdown scorecard) ──

    let mut recs: Vec<String> = Vec::new();
    for (name, s) in &teams {
        if s.adopting == 0 && s.repos > 0 {
            // Pilot candidate: most recently active repo of this team
            // (results preserve the API's last_activity_at desc ordering)
            let (pilot, pilot_url) = results
                .iter()
                .find(|r| &r.team == name)
                .map(|r| (r.path.as_str(), r.web_url.as_str()))
                .unwrap_or(("?", ""));
            recs.push(format!(
                "<div class=\"issue warn\"><b>{} team: 0 adoption</b><div class=\"m\">No markers across {} active repos &mdash; pilot candidate: <code>{}</code>.</div></div>\n",
                esc(name),
                s.repos,
                link(pilot_url, &esc(pilot)),
            ));
        }
    }
    let count_flag = |flag: &str| {
        results
            .iter()
            .filter(|r| has_flag(&quality_flags(&r.markers), flag))
            .count()
    };
    let no_config_count = count_flag("usage w/o config");
    if no_config_count > 0 {
        recs.push(format!(
            "<div class=\"issue warn\"><b>{no_config_count} repo(s) with AI commits but no CLAUDE.md</b><div class=\"m\">Quick win: add one.</div></div>\n"
        ));
    }
    let no_attribution_count = count_flag("no attribution");
    if no_attribution_count > 0 {
        recs.push(format!(
            "<div class=\"issue warn\"><b>{no_attribution_count} repo(s) with agent activity but no commit attribution</b><div class=\"m\">Standardize Co-Authored-By trailers to measure adoption.</div></div>\n"
        ));
    }
    let squash_hidden_count = count_flag("squash-hidden usage");
    if squash_hidden_count > 0 {
        recs.push(format!(
            "<div class=\"issue warn\"><b>{squash_hidden_count} repo(s) lose attribution at merge</b><div class=\"m\">Check squash settings or rely on MR descriptions.</div></div>\n"
        ));
    }
    if !in_flight.is_empty() {
        recs.push(format!(
            "<div class=\"issue warn\"><b>{} repo(s) with AI work on feature branches</b><div class=\"m\">Adoption pipeline &mdash; support these teams so config lands on default.</div></div>\n",
            in_flight.len(),
        ));
    }
    html.push_str("<h2>Recommendations</h2>\n");
    if recs.is_empty() {
        html.push_str("<div class=\"issue ok\"><b>No recommendations</b><div class=\"m\">Adoption practices look healthy for this period.</div></div>\n");
    } else {
        for r in &recs {
            html.push_str(r);
        }
    }

    // ── Dormant repos (collapsed — keeps the leadership view uncluttered) ──

    if !scan.dormant.is_empty() {
        let sorted = sorted_dormant(&scan.dormant);
        html.push_str(&format!(
            "<details id=\"dormant\"><summary>Dormant repos ({}) &mdash; archive candidates</summary>\n<p>Inactive {dormant_days}+ days and not archived &mdash; consider archiving to reduce noise.</p>\n<table>\n<tr><th>Repo</th><th>Team</th><th>Last Activity</th></tr>\n",
            sorted.len(),
        ));
        for d in &sorted {
            let short_path = d
                .path
                .strip_prefix(&format!("{group_path}/"))
                .unwrap_or(&d.path);
            let last = if d.last_activity.is_empty() {
                "&ndash;".to_string()
            } else {
                esc(&d.last_activity)
            };
            html.push_str(&format!(
                "<tr><td><b>{}</b></td><td>{}</td><td>{last}</td></tr>\n",
                link(&d.web_url, &esc(short_path)),
                esc(&d.team),
            ));
        }
        html.push_str("</table>\n</details>\n");
    }

    // ── Methodology footnote ──

    html.push_str(&format!(
        "<details id=\"methodology\"><summary>Methodology</summary><p>Levels: <b>L0</b> no AI tooling markers; <b>L1 Exploring</b> any config marker (CLAUDE.md, AGENTS.md, .cursorrules, .mcp.json); <b>L2 Practicing</b> CLAUDE.md plus shared workflow assets (commands, settings, MCP config, hooks, or an ADR log) &mdash; or agents configured but not yet used; <b>L3 Scaling</b> agents plus measurable usage (&ge;10% AI-trailed commits, recent <code>.tasks</code> activity, or AI-marked MR descriptions). Usage is measured across <i>all</i> branches over the last {days} days because squash-merge strips commit trailers from the default branch; MR descriptions and <code>.tasks</code>/<code>.claude</code> path activity count as first-class evidence. <b>In-flight</b> repos have AI-named feature branches but no config merged yet. Trajectory: &uarr; actively building (live AI branches or recent config/ADR maintenance), &rarr; steady use, &darr; markers present but unused and unmaintained. Attribution rate = share of adopting repos with usage evidence whose usage is visible via Co-Authored-By trailers. Repos with no activity in {dormant_days} days are skipped as dormant and listed as archive candidates.</p></details>\n"
    ));

    // ── Footer ──

    html.push_str(&format!(
        "\n<footer>gl-mcp v{} &middot; {date_str}</footer>\n\n</body>\n</html>",
        env!("CARGO_PKG_VERSION"),
    ));

    Ok(html)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> RepoMarkers {
        RepoMarkers::default()
    }

    /// Exact-match flag lookup (quality_flags returns owned Strings).
    fn fl(m: &RepoMarkers, flag: &str) -> bool {
        quality_flags(m).iter().any(|f| f == flag)
    }

    #[test]
    fn test_level_0_no_markers() {
        let m = empty();
        assert_eq!(adoption_level(&m), 0);
        assert!(quality_flags(&m).is_empty());
    }

    #[test]
    fn test_level_0_commits_only() {
        // Commits without markers don't make a repo "adopting" by themselves
        let m = RepoMarkers { total_commits: 50, ..empty() };
        assert_eq!(adoption_level(&m), 0);
    }

    #[test]
    fn test_level_1_only_claude_md() {
        let m = RepoMarkers { claude_md: true, claude_md_size: 1000, ..empty() };
        assert_eq!(adoption_level(&m), 1);
    }

    #[test]
    fn test_level_1_other_config_markers() {
        assert_eq!(adoption_level(&RepoMarkers { agents_md: true, ..empty() }), 1);
        assert_eq!(adoption_level(&RepoMarkers { cursor: true, ..empty() }), 1);
    }

    #[test]
    fn test_level_2_claude_md_plus_adr() {
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1000,
            adr_count: 3,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 2);
    }

    #[test]
    fn test_level_2_claude_md_plus_commands() {
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1000,
            commands: true,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 2);
    }

    #[test]
    fn test_level_2_agents_without_ai_commits() {
        let m = RepoMarkers {
            agents_count: 4,
            total_commits: 30,
            ai_commits: 0,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 2);
    }

    #[test]
    fn test_level_3_full_agentic_workflow() {
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 2000,
            agents_count: 6,
            skills_count: 9,
            total_commits: 50,
            ai_commits: 17, // 34% >= 10%
            ..empty()
        };
        assert_eq!(adoption_level(&m), 3);
    }

    #[test]
    fn test_level_3_via_recent_task_commits() {
        // Real-world case: agents in use, attribution disabled/squash-stripped —
        // live .tasks activity is the usage evidence.
        let m = RepoMarkers {
            agents_count: 2,
            tasks_dir: true,
            tasks_recent_commits: 2,
            total_commits: 30,
            ai_commits: 0,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 3);
        assert!(!fl(&m, "setup unused"));
        assert!(fl(&m, "no attribution"));
    }

    #[test]
    fn test_level_2_tasks_dir_without_recent_commits() {
        // A stale .tasks dir is a marker, not active usage
        let m = RepoMarkers {
            agents_count: 2,
            skills_count: 1,
            tasks_dir: true,
            tasks_recent_commits: 0,
            total_commits: 0,
            ai_commits: 0,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 2);
        assert!(fl(&m, "setup unused"));
    }

    #[test]
    fn test_level_3_agents_with_ai_pct_no_skills() {
        // Skills are a marker, not a gate: agents + measurable usage = scaling
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 2_000,
            agents_count: 3,
            skills_count: 0,
            total_commits: 50,
            ai_commits: 10, // 20% >= 10%
            ai_commits_default: 10,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 3);
        assert!(quality_flags(&m).is_empty());
    }

    #[test]
    fn test_flag_setup_unused() {
        let m = RepoMarkers {
            agents_count: 3,
            total_commits: 40,
            ai_commits: 0,
            tasks_recent_commits: 0,
            claude_recent_commits: 0,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 2);
        assert!(fl(&m, "setup unused"));
    }

    #[test]
    fn test_no_setup_unused_when_claude_dir_active() {
        // Recent .claude commits mean the setup is being maintained, not abandoned
        let m = RepoMarkers {
            agents_count: 3,
            total_commits: 40,
            ai_commits: 0,
            claude_recent_commits: 4,
            ..empty()
        };
        assert!(!fl(&m, "setup unused"));
        assert!(fl(&m, "no attribution"));
        // .claude activity alone is not usage evidence — stays L2
        assert_eq!(adoption_level(&m), 2);
    }

    #[test]
    fn test_no_attribution_not_flagged_when_ai_commits_present() {
        let m = RepoMarkers {
            agents_count: 2,
            tasks_recent_commits: 5,
            total_commits: 40,
            ai_commits: 8, // 20%
            ai_commits_default: 8,
            ..empty()
        };
        assert!(!fl(&m, "no attribution"));
        assert_eq!(adoption_level(&m), 3);
    }

    #[test]
    fn test_flag_usage_without_config() {
        let m = RepoMarkers {
            claude_md: false,
            total_commits: 100,
            ai_commits: 15, // 15% > 10%
            ..empty()
        };
        assert!(fl(&m, "usage w/o config"));
        // Has AI commits but no config markers — still L0 by marker rules
        assert_eq!(adoption_level(&m), 0);
    }

    #[test]
    fn test_flag_stub_claude_md() {
        let m = RepoMarkers { claude_md: true, claude_md_size: 50, ..empty() };
        assert!(fl(&m, "stub CLAUDE.md"));
    }

    #[test]
    fn test_flag_bloated_claude_md() {
        let m = RepoMarkers { claude_md: true, claude_md_size: 22_000, ..empty() };
        assert!(fl(&m, "bloated CLAUDE.md"));
    }

    #[test]
    fn test_healthy_claude_md_no_flags() {
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 3_000,
            total_commits: 20,
            ai_commits: 5,
            ai_commits_default: 5,
            ..empty()
        };
        assert!(quality_flags(&m).is_empty());
    }

    #[test]
    fn test_ai_pct() {
        let m = RepoMarkers { total_commits: 50, ai_commits: 17, ..empty() };
        assert!((m.ai_pct() - 34.0).abs() < 0.01);
        assert_eq!(empty().ai_pct(), 0.0);
    }

    #[test]
    fn test_team_of() {
        assert_eq!(team_of("my-org/wordpress/site-repo"), "wordpress");
        assert_eq!(team_of("group/repo"), "(root)");
        assert_eq!(team_of("a/b/c/d"), "b");
    }

    // ── v2 signals ──────────────────────────────────────────────────────────

    #[test]
    fn test_flag_squash_hidden_usage() {
        // Trailers on feature branches but none on default = squash strips them
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            total_commits: 50,
            ai_commits: 5,
            ai_commits_default: 0,
            ..empty()
        };
        assert!(fl(&m, "squash-hidden usage"));

        let ok = RepoMarkers { ai_commits_default: 5, ..m };
        assert!(!fl(&ok, "squash-hidden usage"));
    }

    #[test]
    fn test_in_flight_branch_only() {
        // No markers at all — only an AI-named feature branch
        let m = RepoMarkers {
            branch_hits: vec!["feature/claude-import".into()],
            total_commits: 12,
            ..empty()
        };
        assert_eq!(adoption_level(&m), 0);
        assert_eq!(trajectory(&m), "↑");
        assert!(fl(&m, "in-flight (branch: feature/claude-import)"));

        // With a marker present, the repo is no longer "in-flight only"
        let with_marker = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            branch_hits: vec!["feature/claude-import".into()],
            ..empty()
        };
        assert!(!quality_flags(&with_marker).iter().any(|f| f.starts_with("in-flight")));
    }

    #[test]
    fn test_level_3_via_ai_mr_descriptions() {
        // Squash drops trailers, but MR descriptions survive — usage evidence
        let m = RepoMarkers {
            agents_count: 2,
            total_commits: 40,
            ai_commits: 0,
            ai_mr_count: 2,
            total_mr_count: 10,
            ..empty()
        };
        assert!(has_active_usage(&m));
        assert_eq!(adoption_level(&m), 3);
        assert!(!fl(&m, "setup unused"));
    }

    #[test]
    fn test_flag_stale_config() {
        // Precomputed during scan: last touch before window AND >= 30 commits
        let stale = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            claude_md_last_touch: Some("2025-01-15T10:00:00Z".into()),
            claude_md_stale: true,
            total_commits: 45,
            ..empty()
        };
        assert!(fl(&stale, "stale config (30+ commits behind)"));

        let fresh = RepoMarkers { claude_md_stale: false, ..stale };
        assert!(!fl(&fresh, "stale config (30+ commits behind)"));
    }

    #[test]
    fn test_trajectory_up_via_maintenance() {
        // Markers + recent .claude or docs/adr commits = actively building
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            claude_recent_commits: 3,
            ..empty()
        };
        assert_eq!(trajectory(&m), "↑");

        let adr = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            adr_count: 4,
            adr_recent_commits: 2,
            claude_recent_commits: 0,
            ..empty()
        };
        assert_eq!(trajectory(&adr), "↑");
    }

    #[test]
    fn test_trajectory_up_wins_over_down() {
        // Decaying usage but a live AI branch → still "↑" (building beats decaying)
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            branch_hits: vec!["agentic-refactor".into()],
            total_commits: 30,
            ai_commits: 0,
            ..empty()
        };
        assert_eq!(trajectory(&m), "↑");
    }

    #[test]
    fn test_trajectory_steady() {
        // Markers + active usage, no config churn = steady
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            total_commits: 50,
            ai_commits: 20,
            ai_commits_default: 20,
            ..empty()
        };
        assert_eq!(trajectory(&m), "→");
    }

    #[test]
    fn test_trajectory_decaying() {
        // Markers but no usage and no maintenance = decaying
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            total_commits: 30,
            ai_commits: 0,
            ..empty()
        };
        assert_eq!(trajectory(&m), "↓");
    }

    #[test]
    fn test_trajectory_empty_for_no_signals() {
        assert_eq!(trajectory(&empty()), "");
        // Commits alone are not a signal
        let m = RepoMarkers { total_commits: 80, ..empty() };
        assert_eq!(trajectory(&m), "");
    }

    #[test]
    fn test_ai_branch_regex() {
        assert!(is_ai_branch("feature/claude-import"));
        assert!(is_ai_branch("COPILOT-fixes"));
        assert!(is_ai_branch("llm-experiments"));
        assert!(is_ai_branch("agentic-refactor"));
        assert!(is_ai_branch("agent/task-42"));
        assert!(is_ai_branch("ai-assist"));
        assert!(is_ai_branch("ai_review"));
        assert!(!is_ai_branch("main"));
        assert!(!is_ai_branch("fix/email-validation")); // "ai" inside a word, no -/_
        assert!(!is_ai_branch("release/2.0"));
        // Browser user-agent branches are NOT AI work (real-world false positive)
        assert!(!is_ai_branch("Feature/x-775-script-loading-rocket/UserAgent"));
        assert!(!is_ai_branch("fix/user-agent-parsing"));
        assert!(!is_ai_branch("feature/user_agents-table"));
        assert!(!is_ai_branch("marketing/agency-page"));
        // But a genuine agent branch alongside the word still matches
        assert!(is_ai_branch("user-agent-and-claude-agents"));
    }

    // ── HTML report helpers ──────────────────────────────────────────────────

    #[test]
    fn test_attribution_rate() {
        // Trailer-visible usage
        let visible = RepoMarkers {
            claude_md: true,
            total_commits: 50,
            ai_commits: 10, // 20% — active usage AND trailer-visible
            ..empty()
        };
        // Active usage via .tasks, but zero trailers — invisible to attribution
        let hidden = RepoMarkers {
            agents_count: 2,
            tasks_dir: true,
            tasks_recent_commits: 3,
            total_commits: 40,
            ai_commits: 0,
            ..empty()
        };
        // Adopting (markers) but no usage at all — excluded from the denominator
        let no_usage = RepoMarkers {
            claude_md: true,
            claude_md_size: 1_000,
            total_commits: 30,
            ..empty()
        };

        let rate = attribution_rate(&[&visible, &hidden, &no_usage]).unwrap();
        assert!((rate - 50.0).abs() < 0.01); // 1 of 2 usage repos is trailer-visible

        // All visible → 100%
        assert!((attribution_rate(&[&visible]).unwrap() - 100.0).abs() < 0.01);
        // No usage evidence anywhere → None (nothing to attribute)
        assert!(attribution_rate(&[&no_usage]).is_none());
        assert!(attribution_rate(&[]).is_none());
    }

    // ── Dormant repo visibility ──────────────────────────────────────────────

    fn dr(path: &str, last: &str) -> DormantRepo {
        DormantRepo {
            path: path.into(),
            team: team_of(path),
            last_activity: last.into(),
            web_url: String::new(),
        }
    }

    #[test]
    fn test_link_helper() {
        // Empty URL → plain text passthrough (no anchor)
        assert_eq!(link("", "my-org/repo"), "my-org/repo");
        // URL is escaped; text is passed through as-is (pre-escaped by callers)
        assert_eq!(
            link("https://gitlab.example.com/my-org/repo?a=1&b=2", "repo"),
            "<a href=\"https://gitlab.example.com/my-org/repo?a=1&amp;b=2\">repo</a>"
        );
    }

    #[test]
    fn test_dormant_team_mapping() {
        // Same team_of() mapping as active repos
        assert_eq!(dr("my-org/backend/old-api", "2025-06-01").team, "backend");
        assert_eq!(dr("my-org/legacy-site", "2024-11-20").team, "(root)");

        let dormant = vec![
            dr("my-org/backend/old-api", "2025-06-01"),
            dr("my-org/legacy-site", "2024-11-20"),
            dr("my-org/mobile/abandoned-app", "2025-01-03"),
            dr("my-org/backend/dead-cron", "2025-02-10"),
        ];
        let by_team = dormant_by_team(&dormant);
        assert_eq!(by_team.get("backend"), Some(&2));
        assert_eq!(by_team.get("mobile"), Some(&1));
        assert_eq!(by_team.get("(root)"), Some(&1));
        assert!(dormant_by_team(&[]).is_empty());
    }

    #[test]
    fn test_sorted_dormant_oldest_first() {
        let dormant = vec![
            dr("my-org/backend/old-api", "2025-06-01"),
            dr("my-org/legacy-site", "2024-11-20"),
            dr("my-org/backend/dead-cron", ""), // unknown date sorts last
            dr("my-org/mobile/abandoned-app", "2025-01-03"),
        ];
        let order: Vec<&str> = sorted_dormant(&dormant)
            .iter()
            .map(|d| d.path.as_str())
            .collect();
        assert_eq!(
            order,
            vec![
                "my-org/legacy-site",
                "my-org/mobile/abandoned-app",
                "my-org/backend/old-api",
                "my-org/backend/dead-cron",
            ]
        );
    }
}
