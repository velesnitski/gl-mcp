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

/// Repos with no activity in this many days are skipped as dormant.
const DORMANT_DAYS: i64 = 180;

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
    pub ai_commits: usize,
    pub total_commits: usize,
    /// Commits touching `.tasks/` in the scan window — agent activity even without attribution.
    pub tasks_recent_commits: usize,
    /// Commits touching `.claude/` in the scan window.
    pub claude_recent_commits: usize,
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

/// Active usage = measurable AI commit share OR recent commits touching `.tasks/`.
/// Teams often disable Co-Authored-By attribution (or squash strips it), so live
/// task-state files are first-class usage evidence.
pub(crate) fn has_active_usage(m: &RepoMarkers) -> bool {
    m.ai_pct() >= 10.0 || m.tasks_recent_commits > 0
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
pub(crate) fn quality_flags(m: &RepoMarkers) -> Vec<&'static str> {
    let mut flags = Vec::new();
    if m.claude_md && m.claude_md_size < 200 {
        flags.push("stub CLAUDE.md");
    }
    if m.claude_md && m.claude_md_size > 15000 {
        flags.push("bloated CLAUDE.md");
    }
    if m.agents_count > 0
        && m.ai_commits == 0
        && m.tasks_recent_commits == 0
        && m.claude_recent_commits == 0
    {
        flags.push("setup unused");
    }
    if (m.tasks_recent_commits > 0 || m.claude_recent_commits > 0) && m.ai_commits == 0 {
        // Workflow is active but commits carry no Co-Authored-By trailer.
        flags.push("no attribution");
    }
    if m.ai_pct() > 10.0 && !m.claude_md {
        flags.push("usage w/o config");
    }
    flags
}

/// Scan result for one repository.
struct RepoResult {
    path: String,
    team: String,
    markers: RepoMarkers,
}

impl RepoResult {
    fn level(&self) -> u8 {
        adoption_level(&self.markers)
    }

    /// Compact marker list for the table, e.g. "CLAUDE.md, agents(6), skills(9), tasks".
    fn marker_list(&self) -> String {
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
            parts.push(format!("adr({})", m.adr_count));
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
            m.skills_count = count_md_files(client, project_id, ".claude/skills", "100").await;
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

    // 5. AI commit usage (one page is enough)
    let commits: Vec<Value> = client
        .get(
            &format!("/projects/{project_id}/repository/commits"),
            &[("since", since), ("per_page", "100")],
        )
        .await
        .unwrap_or_default();

    m.total_commits = commits.len();
    m.ai_commits = commits
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
        .count();

    // 6. Agent activity without attribution: recent commits touching .tasks / .claude.
    // Only queried for repos that have the marker — no extra calls for unmarked repos.
    if m.tasks_dir {
        m.tasks_recent_commits = count_path_commits(client, project_id, ".tasks", since).await;
    }
    if has_claude_dir {
        m.claude_recent_commits = count_path_commits(client, project_id, ".claude", since).await;
    }

    m
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

/// Scan a GitLab group for AI-assisted development adoption markers and
/// return a per-team scorecard.
pub async fn get_ai_adoption(
    client: &GitLabClient,
    group_path: &str,
    days: u32,
    summary_only: bool,
) -> Result<String> {
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
        return Ok(format!("No projects found in group '{group_path}'."));
    }

    let dormant_cutoff = chrono::Utc::now() - chrono::Duration::days(DORMANT_DAYS);
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    // Extract metadata, skip dormant repos (saves ~5 API calls per repo)
    struct ProjectMeta {
        id: u64,
        path: String,
        default_branch: String,
    }

    let mut active: Vec<ProjectMeta> = Vec::new();
    let mut dormant = 0usize;

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
        if is_dormant {
            dormant += 1;
            continue;
        }
        active.push(ProjectMeta {
            id,
            path,
            default_branch: p["default_branch"].as_str().unwrap_or("").to_string(),
        });
    }

    if active.is_empty() {
        return Ok(format!(
            "Group '{group_path}': all {dormant} repos dormant (no activity in {DORMANT_DAYS}d). Nothing to scan."
        ));
    }

    // Step 3: per-repo marker detection, batched 10× concurrent
    let mut results: Vec<RepoResult> = Vec::new();
    for chunk in active.chunks(10) {
        let futs: Vec<_> = chunk
            .iter()
            .map(|meta| {
                let client = client.clone();
                let id = meta.id;
                let path = meta.path.clone();
                let default_branch = meta.default_branch.clone();
                let since = since.clone();
                async move {
                    let markers = scan_repo(&client, id, &default_branch, &since).await;
                    RepoResult {
                        team: team_of(&path),
                        path,
                        markers,
                    }
                }
            })
            .collect();
        results.extend(join_all(futs).await);
    }

    let active_count = results.len();

    // Step 4: aggregate per team
    struct TeamStats {
        repos: usize,
        with_markers: usize,
        best_level: u8,
        adopting_ai_pcts: Vec<f64>,
    }

    let mut teams: BTreeMap<String, TeamStats> = BTreeMap::new();
    for r in &results {
        let level = r.level();
        let stats = teams.entry(r.team.clone()).or_insert(TeamStats {
            repos: 0,
            with_markers: 0,
            best_level: 0,
            adopting_ai_pcts: Vec::new(),
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
        return Ok(format!(
            "{group_path}: {active_count} repos scanned, {dormant} dormant. {}",
            team_parts.join(" | ")
        ));
    }

    let mut out = vec![
        format!(
            "## AI Adoption: {group_path} (last {days}d, {active_count} active repos, {dormant} dormant skipped)"
        ),
        String::new(),
        "### By Team".to_string(),
        String::new(),
        "| Team | Repos | With markers | Best level | AI commits % (avg of adopting) |".to_string(),
        "|------|-------|--------------|-----------|--------------------------------|".to_string(),
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
            "| {name} | {} | {} | L{} | {avg_pct} |",
            s.repos, s.with_markers, s.best_level
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
        out.push("| Repo | Level | Markers | AI commits | Flags |".to_string());
        out.push("|------|-------|---------|-----------|-------|".to_string());

        let shown = adopting.len().min(25);
        for r in adopting.iter().take(25) {
            let m = &r.markers;
            let mut ai_str = if m.total_commits == 0 {
                "0%".to_string()
            } else {
                format!("{:.0}% ({}/{})", m.ai_pct(), m.ai_commits, m.total_commits)
            };
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
                "| {short_path} | L{} | {} | {ai_str} | {flags_str} |",
                r.level(),
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

    // Quality flags section
    let mut flag_lines: Vec<String> = Vec::new();
    for r in &results {
        let m = &r.markers;
        for flag in quality_flags(m) {
            match flag {
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
        .filter(|r| quality_flags(&r.markers).contains(&"usage w/o config"))
        .count();
    if no_config_count > 0 {
        recs.push(format!(
            "- {no_config_count} repos have AI commits but no CLAUDE.md — quick win: add one"
        ));
    }
    let no_attribution_count = results
        .iter()
        .filter(|r| quality_flags(&r.markers).contains(&"no attribution"))
        .count();
    if no_attribution_count > 0 {
        recs.push(format!(
            "- {no_attribution_count} repos show agent activity without commit attribution — standardize Co-Authored-By trailers to measure adoption"
        ));
    }
    if !recs.is_empty() {
        out.push(String::new());
        out.push("### Recommendations".to_string());
        out.extend(recs);
    }

    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> RepoMarkers {
        RepoMarkers::default()
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
        let flags = quality_flags(&m);
        assert!(!flags.contains(&"setup unused"));
        assert!(flags.contains(&"no attribution"));
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
        assert!(quality_flags(&m).contains(&"setup unused"));
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
        assert!(quality_flags(&m).contains(&"setup unused"));
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
        let flags = quality_flags(&m);
        assert!(!flags.contains(&"setup unused"));
        assert!(flags.contains(&"no attribution"));
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
            ..empty()
        };
        assert!(!quality_flags(&m).contains(&"no attribution"));
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
        assert!(quality_flags(&m).contains(&"usage w/o config"));
        // Has AI commits but no config markers — still L0 by marker rules
        assert_eq!(adoption_level(&m), 0);
    }

    #[test]
    fn test_flag_stub_claude_md() {
        let m = RepoMarkers { claude_md: true, claude_md_size: 50, ..empty() };
        assert!(quality_flags(&m).contains(&"stub CLAUDE.md"));
    }

    #[test]
    fn test_flag_bloated_claude_md() {
        let m = RepoMarkers { claude_md: true, claude_md_size: 22_000, ..empty() };
        assert!(quality_flags(&m).contains(&"bloated CLAUDE.md"));
    }

    #[test]
    fn test_healthy_claude_md_no_flags() {
        let m = RepoMarkers {
            claude_md: true,
            claude_md_size: 3_000,
            total_commits: 20,
            ai_commits: 5,
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
}
