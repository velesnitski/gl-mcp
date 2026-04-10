//! GitLab commit and diff tools with smart filtering and token compression.

use crate::client::GitLabClient;
use crate::error::{Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;

// ─── Smart filtering ───

const SKIP_FILES: &[&str] = &[
    "package-lock.json", "yarn.lock", "composer.lock",
    "go.sum", "Cargo.lock", "Gemfile.lock",
    "pnpm-lock.yaml", "poetry.lock", "Pipfile.lock",
];

const SKIP_PATTERNS: &[&str] = &[
    ".min.js", ".min.css", ".map",
    "vendor/", "node_modules/",
    "__generated__", ".pb.go",
    "dist/", "build/",
];

fn should_skip_file(path: &str) -> bool {
    SKIP_FILES.iter().any(|f| path.ends_with(f))
        || SKIP_PATTERNS.iter().any(|p| path.contains(p))
}

pub fn detect_language(path: &str) -> &str {
    match path.rsplit('.').next() {
        Some("php") => "PHP",
        Some("go") => "Go",
        Some("kt" | "kts") => "Kotlin",
        Some("java") => "Java",
        Some("swift") => "Swift",
        Some("ts" | "tsx") => "TypeScript",
        Some("js" | "jsx") => "JavaScript",
        Some("yml" | "yaml") => "YAML/Ansible",
        Some("rs") => "Rust",
        Some("py") => "Python",
        Some("rb") => "Ruby",
        Some("sh" | "bash") => "Shell",
        Some("sql") => "SQL",
        Some("vue") => "Vue",
        Some("css" | "scss" | "less") => "CSS",
        Some("html" | "twig") => "HTML",
        Some("json") => "JSON",
        Some("toml") => "TOML",
        Some("xml") => "XML",
        Some("md") => "Markdown",
        Some("gradle") => "Gradle",
        Some("j2") => "Jinja2/Ansible",
        Some("cfg" | "ini" | "conf") => "Config",
        Some("csv") => "CSV",
        Some("tf" | "tfvars") => "Terraform",
        Some("hcl") => "HCL",
        _ if path.contains("Dockerfile") => "Docker",
        _ if path.contains("Makefile") => "Make",
        _ if path.contains(".github/") || path.contains(".gitlab-ci") => "CI/CD",
        _ if path.contains("inventory/") => "Ansible/Inventory",
        _ if path.contains("ansible/") && !path.contains('.') => "Ansible/Inventory",
        _ if path.contains("ansible/") => "YAML/Ansible",
        _ => "Other",
    }
}

fn count_diff_lines(diff: &str) -> (usize, usize) {
    let mut additions = 0;
    let mut deletions = 0;
    for line in diff.lines() {
        if line.starts_with('+') && !line.starts_with("+++") {
            additions += 1;
        } else if line.starts_with('-') && !line.starts_with("---") {
            deletions += 1;
        }
    }
    (additions, deletions)
}

fn truncate_diff(diff: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() <= max_lines {
        return (diff.to_string(), 0);
    }
    let truncated = lines[..max_lines].join("\n");
    let remaining = lines.len() - max_lines;
    (truncated, remaining)
}

/// Shared diff processing: parse diffs into language-grouped structure.
struct DiffFile {
    path: String,
    lang: String,
    status: &'static str,
    additions: usize,
    deletions: usize,
    diff_text: String,
}

fn process_diffs(
    diffs: &[Value],
    skip_generated: bool,
    file_filter: &str,
) -> (Vec<DiffFile>, Vec<(String, usize, usize)>) {
    let mut files = Vec::new();
    let mut skipped = Vec::new();

    for diff in diffs {
        let new_path = diff["new_path"].as_str().unwrap_or("?");
        let old_path = diff["old_path"].as_str().unwrap_or(new_path);
        let diff_text = diff["diff"].as_str().unwrap_or("");
        let is_new = diff["new_file"].as_bool().unwrap_or(false);
        let is_deleted = diff["deleted_file"].as_bool().unwrap_or(false);
        let is_renamed = diff["renamed_file"].as_bool().unwrap_or(false);

        let display_path = if is_renamed && old_path != new_path {
            format!("{old_path} → {new_path}")
        } else {
            new_path.to_string()
        };

        // File filter: only include matching file
        if !file_filter.is_empty() && !new_path.contains(file_filter) && !old_path.contains(file_filter) {
            continue;
        }

        let (additions, deletions) = count_diff_lines(diff_text);

        if skip_generated && should_skip_file(new_path) {
            skipped.push((display_path, additions, deletions));
            continue;
        }

        let status = if is_new {
            " (new)"
        } else if is_deleted {
            " (deleted)"
        } else if is_renamed {
            " (renamed)"
        } else {
            ""
        };

        files.push(DiffFile {
            path: display_path,
            lang: detect_language(new_path).to_string(),
            status,
            additions,
            deletions,
            diff_text: diff_text.to_string(),
        });
    }
    (files, skipped)
}

/// Format files as summary only (no diff content) — minimal tokens.
fn format_summary(
    files: &[DiffFile],
    skipped: &[(String, usize, usize)],
) -> Vec<String> {
    let mut by_lang: BTreeMap<&str, (usize, usize, usize)> = BTreeMap::new();
    for f in files {
        let e = by_lang.entry(&f.lang).or_default();
        e.0 += 1;
        e.1 += f.additions;
        e.2 += f.deletions;
    }

    let mut parts = Vec::new();
    // Language breakdown
    for (lang, (count, add, del)) in &by_lang {
        parts.push(format!("- **{lang}**: {count} files (+{add} -{del})"));
    }

    // File list (compact, no diffs)
    parts.push(String::new());
    for f in files {
        parts.push(format!(
            "  {}{} +{} -{} [{}]",
            f.path, f.status, f.additions, f.deletions, f.lang
        ));
    }

    if !skipped.is_empty() {
        let total_add: usize = skipped.iter().map(|(_, a, _)| a).sum();
        let total_del: usize = skipped.iter().map(|(_, _, d)| d).sum();
        parts.push(format!(
            "\n*Skipped {} generated/lock files (+{total_add} -{total_del})*",
            skipped.len()
        ));
    }
    parts
}

/// Format files with full diffs grouped by language.
fn format_full_diffs(
    files: &[DiffFile],
    skipped: &[(String, usize, usize)],
    max_lines_per_file: usize,
    compact: bool,
) -> Vec<String> {
    let mut by_language: BTreeMap<&str, Vec<&DiffFile>> = BTreeMap::new();
    for f in files {
        by_language.entry(&f.lang).or_default().push(f);
    }

    let mut parts = Vec::new();

    for (lang, lang_files) in &by_language {
        let total_add: usize = lang_files.iter().map(|f| f.additions).sum();
        let total_del: usize = lang_files.iter().map(|f| f.deletions).sum();

        if compact {
            parts.push(format!("{lang}|{}f|+{total_add}|-{total_del}", lang_files.len()));
        } else {
            parts.push(format!(
                "### {} ({} files, +{} -{})",
                lang, lang_files.len(), total_add, total_del,
            ));
        }
        parts.push(String::new());

        for f in lang_files {
            let (display_diff, truncated) = truncate_diff(&f.diff_text, max_lines_per_file);

            if compact {
                parts.push(format!("{}{}|+{}|-{}", f.path, f.status, f.additions, f.deletions));
                if !display_diff.is_empty() {
                    parts.push(display_diff);
                }
            } else {
                parts.push(format!(
                    "**{}{}** (+{} -{})",
                    f.path, f.status, f.additions, f.deletions
                ));
                if !display_diff.is_empty() {
                    parts.push(format!("```diff\n{display_diff}\n```"));
                }
            }

            if truncated > 0 {
                parts.push(format!("*...{truncated} more lines truncated*"));
            }
            parts.push(String::new());
        }
    }

    if !skipped.is_empty() {
        let total_add: usize = skipped.iter().map(|(_, a, _)| a).sum();
        let total_del: usize = skipped.iter().map(|(_, _, d)| d).sum();
        if compact {
            parts.push(format!("Skipped|{}f|+{total_add}|-{total_del}", skipped.len()));
        } else {
            parts.push(format!(
                "### Skipped ({} files, +{} -{})",
                skipped.len(), total_add, total_del,
            ));
        }
        for (path, add, del) in skipped {
            parts.push(format!("- {path} (+{add} -{del})"));
        }
    }
    parts
}

// ─── Tool implementations ───

/// List commits for a project.
pub async fn list_commits(
    client: &GitLabClient,
    project_id: &str,
    branch: &str,
    author: &str,
    since: &str,
    until: &str,
    per_page: u32,
    summary_only: bool,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/projects/{}/repository/commits",
        urlencoding::encode(project_id)
    );

    // Don't send author to API — GitLab does exact match which breaks on
    // Cyrillic names (e.g. "Malykhin" won't match "Владимир Малыхин").
    // Fetch all commits and filter client-side with fuzzy contains.
    let mut params: Vec<(&str, &str)> = vec![("per_page", &per_page_str)];
    if !branch.is_empty() {
        params.push(("ref_name", branch));
    }
    if !since.is_empty() {
        params.push(("since", since));
    }
    if !until.is_empty() {
        params.push(("until", until));
    }

    let all_commits: Vec<Value> = client.get(&path, &params).await?;

    // Client-side author filter: case-insensitive contains on name AND email
    let commits: Vec<&Value> = if author.is_empty() {
        all_commits.iter().collect()
    } else {
        let query = author.to_lowercase();
        all_commits.iter().filter(|c| {
            let name = c["author_name"].as_str().unwrap_or("").to_lowercase();
            let email = c["author_email"].as_str().unwrap_or("").to_lowercase();
            name.contains(&query) || email.contains(&query)
        }).collect()
    };

    if commits.is_empty() {
        return Ok("No commits found.".to_string());
    }

    if summary_only {
        // Compact: one line per commit
        let mut lines = vec![format!("{} commits", commits.len())];
        for c in &commits {
            let sha = c["short_id"].as_str().unwrap_or("?");
            let title = c["title"].as_str().unwrap_or("?");
            let author = c["author_name"].as_str().unwrap_or("?");
            lines.push(format!("{sha}|{author}|{title}"));
        }
        return Ok(lines.join("\n"));
    }

    let mut by_author: BTreeMap<String, Vec<&&Value>> = BTreeMap::new();
    for c in &commits {
        let author = c["author_name"].as_str().unwrap_or("Unknown").to_string();
        by_author.entry(author).or_default().push(c);
    }

    let mut lines = vec![format!("**Found: {} commits**\n", commits.len())];
    lines.push("### By author".to_string());
    for (author, author_commits) in &by_author {
        lines.push(format!("- **{}**: {} commits", author, author_commits.len()));
    }
    lines.push(String::new());
    lines.push("### Commits".to_string());
    for c in &commits {
        let sha = c["short_id"].as_str().unwrap_or("?");
        let title = c["title"].as_str().unwrap_or("?");
        let author = c["author_name"].as_str().unwrap_or("?");
        let date = c["created_at"].as_str().unwrap_or("?");
        let date_short = if date.len() > 16 { &date[..16] } else { date };
        lines.push(format!("- `{sha}` {title} — @{author} ({date_short})"));
    }
    Ok(lines.join("\n"))
}

/// Get commit diff with smart filtering.
///
/// Modes:
/// - summary_only=true: file list + stats, no diff content (~10x smaller)
/// - file="path": only show diff for matching file
/// - compact=true: strip markdown formatting
pub async fn get_commit_diff(
    client: &GitLabClient,
    project_id: &str,
    sha: &str,
    max_lines_per_file: usize,
    skip_generated: bool,
    summary_only: bool,
    file_filter: &str,
    compact: bool,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let commit: Value = client
        .get(&format!("/projects/{encoded}/repository/commits/{sha}"), &[])
        .await
        ?;

    let _title = commit["title"].as_str().unwrap_or("?");
    let author = commit["author_name"].as_str().unwrap_or("?");
    let date = commit["created_at"].as_str().unwrap_or("?");
    let message = commit["message"].as_str().unwrap_or("");
    let stats_add = commit["stats"]["additions"].as_u64().unwrap_or(0);
    let stats_del = commit["stats"]["deletions"].as_u64().unwrap_or(0);

    let diffs: Vec<Value> = client
        .get(&format!("/projects/{encoded}/repository/commits/{sha}/diff"), &[])
        .await
        ?;

    let (files, skipped) = process_diffs(&diffs, skip_generated, file_filter);

    let mut parts = if compact {
        vec![
            format!("{project_id}|{sha}|{author}|{date}"),
            message.to_string(),
            format!("+{stats_add}|-{stats_del}|{}f", diffs.len()),
            String::new(),
        ]
    } else {
        vec![
            format!("## Commit `{sha}` in {project_id}"),
            format!("**Author:** {author} | **Date:** {date}"),
            format!("**Message:** {message}"),
            format!("**Stats:** +{stats_add} -{stats_del} in {} files", diffs.len()),
            String::new(),
        ]
    };

    if summary_only {
        parts.extend(format_summary(&files, &skipped));
    } else {
        parts.extend(format_full_diffs(&files, &skipped, max_lines_per_file, compact));
    }

    let result = parts.join("\n");
    Ok(result)
}

/// Get MR changes (aggregated diff across all commits).
pub async fn get_mr_changes(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
    max_lines_per_file: usize,
    skip_generated: bool,
    summary_only: bool,
    file_filter: &str,
    compact: bool,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let mr: Value = client
        .get(&format!("/projects/{encoded}/merge_requests/{mr_iid}"), &[])
        .await
        ?;

    let title = mr["title"].as_str().unwrap_or("?");
    let author = mr["author"]["username"].as_str().unwrap_or("?");
    let source = mr["source_branch"].as_str().unwrap_or("?");
    let target = mr["target_branch"].as_str().unwrap_or("?");
    let state = mr["state"].as_str().unwrap_or("?");

    let changes_data: Value = client
        .get(&format!("/projects/{encoded}/merge_requests/{mr_iid}/changes"), &[])
        .await
        ?;

    let changes = changes_data["changes"].as_array().cloned().unwrap_or_default();
    let (files, skipped) = process_diffs(&changes, skip_generated, file_filter);

    let mut parts = if compact {
        vec![
            format!("{project_id}|!{mr_iid}|{title}|{state}"),
            format!("{author}|{source}→{target}|{}f", changes.len()),
            String::new(),
        ]
    } else {
        vec![
            format!("## {project_id} !{mr_iid}: {title}"),
            format!("**Author:** @{author} | **State:** {state}"),
            format!("**Branch:** {source} → {target}"),
            format!("**Files changed:** {}", changes.len()),
            String::new(),
        ]
    };

    if summary_only {
        parts.extend(format_summary(&files, &skipped));
    } else {
        parts.extend(format_full_diffs(&files, &skipped, max_lines_per_file, compact));
    }

    let result = parts.join("\n");
    Ok(result)
}

/// Get file content at a specific ref.
pub async fn get_file_content(
    client: &GitLabClient,
    project_id: &str,
    file_path: &str,
    ref_name: &str,
) -> Result<String> {
    let encoded_project = urlencoding::encode(project_id);
    let encoded_file = urlencoding::encode(file_path);

    let data: Value = client
        .get(
            &format!("/projects/{encoded_project}/repository/files/{encoded_file}"),
            &[("ref", ref_name)],
        )
        .await
        ?;

    let content_b64 = data["content"].as_str().unwrap_or("");
    let encoding = data["encoding"].as_str().unwrap_or("base64");
    let size = data["size"].as_u64().unwrap_or(0);
    let lang = detect_language(file_path);

    let content = if encoding == "base64" {
        let decoded = base64_decode(content_b64).map_err(|e| Error::Other(format!("Base64 decode error: {e}")))?;
        String::from_utf8_lossy(&decoded).to_string()
    } else {
        content_b64.to_string()
    };

    let parts = vec![
        format!("## {file_path}"),
        format!("**Ref:** {ref_name} | **Size:** {size} bytes | **Language:** {lang}"),
        String::new(),
        format!("```{}", lang.to_lowercase()),
        content,
        "```".to_string(),
    ];

    let result = parts.join("\n");
    Ok(result)
}

/// Get user activity (events) for the last N hours.
/// Fetch user events for a time window. Shared by get_user_activity and get_user_daily_report.
pub async fn fetch_user_events(
    client: &GitLabClient,
    user_id: u64,
    since_ts: i64,
) -> Result<Vec<Value>> {
    let all_events: Vec<Value> = client
        .get_all_pages(
            &format!("/users/{user_id}/events"),
            &[("sort", "desc")],
            10,
        )
        .await?;

    Ok(all_events
        .into_iter()
        .filter(|e| {
            e["created_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp() >= since_ts)
                .unwrap_or(false)
        })
        .collect())
}

/// Resolve project IDs to names via concurrent batch lookup.
pub async fn resolve_project_names(
    client: &GitLabClient,
    project_ids: &std::collections::BTreeSet<u64>,
) -> BTreeMap<u64, String> {
    use futures::future::join_all;

    let futures: Vec<_> = project_ids.iter().map(|&pid| {
        let client = client.clone();
        async move {
            let cache_key = format!("project:{pid}");
            let name = client
                .get_cached::<Value>(&cache_key, &format!("/projects/{pid}"), &[("simple", "true")], 60)
                .await
                .ok()
                .and_then(|proj| proj["path_with_namespace"].as_str().map(|s| s.to_string()))
                .unwrap_or_else(|| "?".to_string());
            (pid, name)
        }
    }).collect();

    join_all(futures).await.into_iter().collect()
}

/// Per-day, per-project activity struct.
struct DayProjectStats {
    pushes: u64,
    commits: u64,
    merges: u64,
    mr_opened: u64,
    mr_merged: u64,
    mr_approved: u64,
    other_events: u64,
}

impl DayProjectStats {
    fn new() -> Self {
        Self { pushes: 0, commits: 0, merges: 0, mr_opened: 0, mr_merged: 0, mr_approved: 0, other_events: 0 }
    }
    fn total(&self) -> u64 {
        self.pushes + self.mr_opened + self.mr_merged + self.mr_approved + self.other_events
    }
}

/// Get developer activity with per-day, per-project breakdown.
pub async fn get_user_activity(
    client: &GitLabClient,
    username: &str,
    hours: u32,
) -> Result<String> {
    let cache_key = format!("user:{username}");
    let users: Vec<Value> = client
        .get_cached(&cache_key, "/users", &[("username", username)], 60)
        .await
        ?;

    let user = users.first().ok_or_else(|| Error::NotFound(format!("User @{username} not found")))?;
    let user_id = user["id"].as_u64().ok_or(Error::Other("User has no ID".into()))?;
    let display_name = user["name"].as_str().unwrap_or(username);

    let since = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
    let events = fetch_user_events(client, user_id, since.timestamp()).await?;

    if events.is_empty() {
        return Ok(format!(
            "## @{username} ({display_name})\n**Period:** last {hours}h\n\nNo activity."
        ));
    }

    // Collect unique project IDs
    let mut project_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    for e in &events {
        if let Some(pid) = e["project_id"].as_u64() {
            project_ids.insert(pid);
        }
    }

    // Resolve project names
    let project_names = resolve_project_names(client, &project_ids).await;

    // Aggregate: day → project → stats
    // Also aggregate totals
    let mut by_day: BTreeMap<String, BTreeMap<u64, DayProjectStats>> = BTreeMap::new();
    let mut total = DayProjectStats::new();

    for event in &events {
        let date = event["created_at"]
            .as_str()
            .and_then(|s| s.get(..10))
            .unwrap_or("?")
            .to_string();
        let pid = event["project_id"].as_u64().unwrap_or(0);
        let action = event["action_name"].as_str().unwrap_or("");
        let target_type = event["target_type"].as_str().unwrap_or("");

        let stats = by_day.entry(date).or_default().entry(pid).or_insert_with(DayProjectStats::new);

        if action == "pushed to" || action == "pushed new" {
            let raw_count = event["push_data"]["commit_count"].as_u64().unwrap_or(1);
            // Detect merge commits: >20 commits in a single push is almost certainly a branch merge
            let is_merge = raw_count > 20;
            let display_count = if is_merge { 1 } else { raw_count };
            stats.pushes += 1;
            stats.commits += display_count;
            if is_merge { stats.merges += 1; }
            total.pushes += 1;
            total.commits += display_count;
            if is_merge { total.merges += 1; }
        } else if target_type == "MergeRequest" {
            match action {
                "opened" => { stats.mr_opened += 1; total.mr_opened += 1; }
                "accepted" => { stats.mr_merged += 1; total.mr_merged += 1; }
                "approved" => { stats.mr_approved += 1; total.mr_approved += 1; }
                _ => { stats.other_events += 1; total.other_events += 1; }
            }
        } else {
            stats.other_events += 1;
            total.other_events += 1;
        }
    }

    // Format output
    let mut lines = vec![
        format!("## @{username} ({display_name})"),
        format!(
            "**Period:** last {hours}h | **Events:** {} | **Projects:** {}",
            events.len(),
            project_ids.len()
        ),
        format!(
            "**Totals:** {} pushes ({} commits, {} branch merges), {} MRs opened, {} merged, {} approved",
            total.pushes, total.commits, total.merges, total.mr_opened, total.mr_merged, total.mr_approved
        ),
        String::new(),
    ];

    // Per-day breakdown (newest first)
    for (day, projects) in by_day.iter().rev() {
        let day_total: u64 = projects.values().map(|s| s.total()).sum();
        let day_commits: u64 = projects.values().map(|s| s.commits).sum();
        lines.push(format!("### {day} ({day_total} events, {day_commits} commits)"));

        for (pid, stats) in projects {
            let proj_name = project_names
                .get(pid)
                .map(|s| s.as_str())
                .unwrap_or("unknown");
            // Short name: last 2 path segments
            let short = proj_name
                .rsplit('/')
                .take(2)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect::<Vec<_>>()
                .join("/");

            let mut parts = Vec::new();
            if stats.pushes > 0 {
                let merge_note = if stats.merges > 0 {
                    format!(" + {} branch merge", stats.merges)
                } else {
                    String::new()
                };
                parts.push(format!("{} pushes ({} commits{})", stats.pushes, stats.commits, merge_note));
            }
            if stats.mr_opened > 0 {
                parts.push(format!("{} MR opened", stats.mr_opened));
            }
            if stats.mr_merged > 0 {
                parts.push(format!("{} MR merged", stats.mr_merged));
            }
            if stats.mr_approved > 0 {
                parts.push(format!("{} MR approved", stats.mr_approved));
            }
            if stats.other_events > 0 {
                parts.push(format!("{} other", stats.other_events));
            }

            lines.push(format!("- **{}**: {}", short, parts.join(", ")));
        }
        lines.push(String::new());
    }

    Ok(lines.join("\n"))
}

/// Get team activity — multiple users in one call.
pub async fn get_team_activity(
    client: &GitLabClient,
    usernames: &[&str],
    hours: u32,
) -> Result<String> {
    let since = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
    let since_ts = since.timestamp();

    struct UserSummary {
        display_name: String,
        events: u64,
        pushes: u64,
        commits: u64,
        merges: u64,
        mr_opened: u64,
        mr_merged: u64,
        mr_approved: u64,
        projects: std::collections::BTreeSet<String>,
    }

    let mut all_project_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();

    // Resolve all users and fetch events concurrently
    let user_futures: Vec<_> = usernames.iter().map(|&username| {
        let client = client.clone();
        async move {
            let cache_key = format!("user:{username}");
            let users: Vec<serde_json::Value> = match client
                .get_cached(&cache_key, "/users", &[("username", username)], 60)
                .await
            {
                Ok(u) => u,
                Err(_) => return None,
            };

            let user = users.first()?;
            let user_id = user["id"].as_u64()?;
            let display_name = user["name"].as_str().unwrap_or(username).to_string();

            let events = fetch_user_events(&client, user_id, since_ts).await.ok()?;

            Some((username.to_string(), display_name, events))
        }
    }).collect();

    let results = futures::future::join_all(user_futures).await;

    let mut user_summaries: Vec<(String, UserSummary)> = Vec::new();
    for result in results.into_iter().flatten() {
        let (username, display_name, events) = result;
        let mut summary = UserSummary {
            display_name,
            events: events.len() as u64,
            pushes: 0, commits: 0, merges: 0,
            mr_opened: 0, mr_merged: 0, mr_approved: 0,
            projects: std::collections::BTreeSet::new(),
        };

        for event in &events {
            let action = event["action_name"].as_str().unwrap_or("");
            let target_type = event["target_type"].as_str().unwrap_or("");

            if let Some(pid) = event["project_id"].as_u64() {
                summary.projects.insert(pid.to_string());
                all_project_ids.insert(pid);
            }

            if action == "pushed to" || action == "pushed new" {
                let raw = event["push_data"]["commit_count"].as_u64().unwrap_or(1);
                let is_merge = raw > 20;
                summary.pushes += 1;
                summary.commits += if is_merge { 1 } else { raw };
                if is_merge { summary.merges += 1; }
            } else if target_type == "MergeRequest" {
                match action {
                    "opened" => summary.mr_opened += 1,
                    "accepted" => summary.mr_merged += 1,
                    "approved" => summary.mr_approved += 1,
                    _ => {}
                }
            }
        }

        user_summaries.push((username, summary));
    }

    if user_summaries.is_empty() {
        return Ok("No users found.".to_string());
    }

    // Resolve project names
    let project_names = resolve_project_names(client, &all_project_ids).await;

    // Format
    let total_events: u64 = user_summaries.iter().map(|(_, s)| s.events).sum();
    let total_commits: u64 = user_summaries.iter().map(|(_, s)| s.commits).sum();

    let mut lines = vec![
        format!("## Team Activity ({} members, last {hours}h)", user_summaries.len()),
        format!("**Total:** {total_events} events, {total_commits} commits, {} projects\n", all_project_ids.len()),
        format!(
            "| Developer | Events | Commits | MRs opened | MRs merged | Approved | Projects |"
        ),
        "|-----------|--------|---------|------------|------------|----------|----------|".to_string(),
    ];

    // Sort by events descending
    user_summaries.sort_by(|a, b| b.1.events.cmp(&a.1.events));

    for (username, s) in &user_summaries {
        let proj_names: Vec<String> = s.projects.iter().filter_map(|pid_str| {
            pid_str.parse::<u64>().ok().and_then(|pid| {
                project_names.get(&pid).map(|name| {
                    name.rsplit('/').next().unwrap_or(name).to_string()
                })
            })
        }).collect();

        let proj_str = if proj_names.is_empty() {
            "–".to_string()
        } else {
            proj_names.join(", ")
        };

        lines.push(format!(
            "| @{} ({}) | {} | {} | {} | {} | {} | {} |",
            username, s.display_name,
            s.events, s.commits, s.mr_opened, s.mr_merged, s.mr_approved, proj_str
        ));
    }

    // Flag inactive users
    let inactive: Vec<&str> = user_summaries.iter()
        .filter(|(_, s)| s.events == 0)
        .map(|(u, _)| u.as_str())
        .collect();

    if !inactive.is_empty() {
        lines.push(String::new());
        lines.push(format!("**Inactive:** {}", inactive.join(", ")));
    }

    Ok(lines.join("\n"))
}

/// Get activity for all members of a GitLab group.
pub async fn get_group_activity(
    client: &GitLabClient,
    group_path: &str,
    hours: u32,
) -> Result<String> {
    // Get group members
    let path = format!("/groups/{}/members", urlencoding::encode(group_path));
    let members: Vec<Value> = client.get(&path, &[("per_page", "100")]).await?;

    if members.is_empty() {
        return Ok(format!("No members found in group {group_path}"));
    }

    let since = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
    let since_ts = since.timestamp();

    let mut lines = vec![format!("**Group activity: {group_path}**\nPeriod: last {hours}h\n")];
    let mut total_commits = 0u64;
    let mut total_mrs = 0u64;
    let mut active_count = 0u32;

    for member in &members {
        let username = match member["username"].as_str() {
            Some(u) => u,
            None => continue,
        };
        let name = member["name"].as_str().unwrap_or(username);
        let user_id = match member["id"].as_u64() {
            Some(id) => id,
            None => continue,
        };

        // Skip bots
        if username.contains("bot") || username.starts_with("group_") {
            continue;
        }

        let events = fetch_user_events(client, user_id, since_ts).await?;
        if events.is_empty() {
            lines.push(format!("- @{username} ({name}): no activity"));
            continue;
        }

        active_count += 1;
        let mut commits = 0u64;
        let mut mrs_opened = 0u64;
        let mut mrs_merged = 0u64;
        let mut project_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();

        for event in &events {
            let action = event["action_name"].as_str().unwrap_or("");
            let target_type = event["target_type"].as_str().unwrap_or("");
            let project_id = event["project_id"].as_u64().unwrap_or(0);

            if action == "pushed to" || action == "pushed new" {
                let push_commits = event["push_data"]["commit_count"].as_u64().unwrap_or(0);
                commits += push_commits;
            }
            if target_type == "MergeRequest" {
                if action == "opened" { mrs_opened += 1; }
                if action == "accepted" { mrs_merged += 1; }
            }
            if project_id > 0 {
                project_ids.insert(project_id);
            }
        }

        // Resolve project names
        let project_names = resolve_project_names(client, &project_ids).await;
        let proj_list: Vec<&str> = project_names.values().map(|s| s.as_str()).collect();

        total_commits += commits;
        total_mrs += mrs_opened;

        lines.push(format!(
            "- @{username} ({name}): {commits} commits, {mrs_opened} MRs opened, {mrs_merged} merged | {}",
            if proj_list.is_empty() { "\u{2013}".to_string() } else { proj_list.join(", ") }
        ));
    }

    lines.insert(1, format!("Active: {active_count}/{} members | {total_commits} commits, {total_mrs} MRs\n",
        members.iter().filter(|m| !m["username"].as_str().unwrap_or("").contains("bot")).count()));

    Ok(lines.join("\n"))
}

/// List projects in a GitLab group (with subgroups).
pub async fn list_group_projects(
    client: &GitLabClient,
    group_path: &str,
    per_page: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(group_path);
    let per_page_str = per_page.to_string();

    let projects: Vec<Value> = client
        .get(
            &format!("/groups/{encoded}/projects"),
            &[
                ("per_page", per_page_str.as_str()),
                ("include_subgroups", "true"),
                ("order_by", "last_activity_at"),
                ("sort", "desc"),
            ],
        )
        .await
        ?;

    if projects.is_empty() {
        return Ok(format!("No projects found in group '{group_path}'."));
    }

    let mut lines = vec![format!("**Group '{group_path}': {} projects**\n", projects.len())];
    for p in &projects {
        let name = p["path_with_namespace"].as_str().unwrap_or("?");
        let id = p["id"].as_u64().unwrap_or(0);
        let last_activity = p["last_activity_at"].as_str().unwrap_or("?");
        let date_short = if last_activity.len() > 10 {
            &last_activity[..10]
        } else {
            last_activity
        };
        lines.push(format!("- **{name}** (id: {id}) last: {date_short}"));
    }
    Ok(lines.join("\n"))
}

fn base64_decode(input: &str) -> std::result::Result<Vec<u8>, String> {
    let clean: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::with_capacity(clean.len() * 3 / 4);
    let mut buf: u32 = 0;
    let mut bits: u32 = 0;
    for byte in clean.bytes() {
        let val = if byte == b'=' {
            break;
        } else if let Some(pos) = TABLE.iter().position(|&b| b == byte) {
            pos as u32
        } else {
            return Err(format!("Invalid base64 character: {}", byte as char));
        };
        buf = (buf << 6) | val;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
            buf &= (1 << bits) - 1;
        }
    }
    Ok(out)
}

/// Compare multiple developers' performance in a project over a given period.
pub async fn compare_developers(
    client: &GitLabClient,
    project_id: &str,
    usernames: &[&str],
    days: u32,
    summary_only: bool,
) -> Result<String> {
    use futures::future::join_all;

    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    // Support comma-separated project IDs
    let projects: Vec<&str> = project_id.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()).collect();

    struct DevStats {
        username: String,
        mrs_opened: u64,
        mrs_merged: u64,
        mrs_reviewed: u64,
        reviewed_authors: BTreeMap<String, u64>,
        approvals_given: u64,
        avg_merge_hours: f64,
        mr_comments: u64,
        commits: u64,
        additions: u64,
        deletions: u64,
        files_changed: u64,
        mr_sizes: (u64, u64, u64),
    }

    let futures: Vec<_> = usernames.iter().map(|&username| {
        let client = client.clone();
        let since = since.clone();
        let projects = projects.clone();
        async move {
            let mut total_opened: u64 = 0;
            let mut total_merged_count: u64 = 0;
            let mut total_reviewed_count: u64 = 0;
            let mut all_reviewed_authors: BTreeMap<String, u64> = BTreeMap::new();
            let mut all_merge_hours: Vec<f64> = Vec::new();
            let mut total_additions: u64 = 0;
            let mut total_deletions: u64 = 0;
            let mut total_files: u64 = 0;
            let mut mr_small: u64 = 0;
            let mut mr_medium: u64 = 0;
            let mut mr_large: u64 = 0;
            let mut total_approvals: u64 = 0;
            let mut total_comments: u64 = 0;
            let mut total_commits: u64 = 0;

            for proj in &projects {
                let encoded_project = urlencoding::encode(proj);
                let mr_path = format!("/projects/{encoded_project}/merge_requests");

                // 1) Fetch opened MRs by this author
                let opened_mrs: Vec<Value> = client.get(&mr_path, &[
                    ("author_username", username),
                    ("state", "opened"),
                    ("created_after", &since),
                    ("per_page", "100"),
                ]).await.unwrap_or_default();
                total_opened += opened_mrs.len() as u64;

                // 2) Fetch merged MRs by this author
                let merged_mrs: Vec<Value> = client.get(&mr_path, &[
                    ("author_username", username),
                    ("state", "merged"),
                    ("created_after", &since),
                    ("per_page", "100"),
                ]).await.unwrap_or_default();
                total_merged_count += merged_mrs.len() as u64;

                // 3) Fetch MRs where this user is reviewer (merged)
                let reviewed_mrs: Vec<Value> = client.get(&mr_path, &[
                    ("reviewer_username", username),
                    ("state", "merged"),
                    ("created_after", &since),
                    ("per_page", "100"),
                ]).await.unwrap_or_default();
                total_reviewed_count += reviewed_mrs.len() as u64;

                // 4) Calculate avg merge time + LOC/files from merged MRs
                for mr in &merged_mrs {
                    let created = mr["created_at"].as_str().unwrap_or("");
                    let merged = mr["merged_at"].as_str().unwrap_or("");
                    let created_dt = chrono::DateTime::parse_from_rfc3339(created).ok();
                    let merged_dt = chrono::DateTime::parse_from_rfc3339(merged).ok();
                    if let (Some(c), Some(m)) = (created_dt, merged_dt) {
                        all_merge_hours.push((m - c).num_minutes() as f64 / 60.0);
                    }

                    let mr_iid = mr["iid"].as_u64().unwrap_or(0);
                    if mr_iid > 0 {
                        let changes: std::result::Result<Value, _> = client
                            .get(
                                &format!("/projects/{encoded_project}/merge_requests/{mr_iid}/changes"),
                                &[("access_raw_diffs", "true")],
                            )
                            .await;
                        if let Ok(detail) = changes {
                            if let Some(files) = detail["changes"].as_array() {
                                let file_count = files.len() as u64;
                                total_files += file_count;
                                match file_count {
                                    0..=9 => mr_small += 1,
                                    10..=50 => mr_medium += 1,
                                    _ => mr_large += 1,
                                }
                                for f in files {
                                    let diff = f["diff"].as_str().unwrap_or("");
                                    for line in diff.lines() {
                                        if line.starts_with('+') && !line.starts_with("+++") {
                                            total_additions += 1;
                                        } else if line.starts_with('-') && !line.starts_with("---") {
                                            total_deletions += 1;
                                        }
                                    }
                                }
                            }
                        }
                    }
                }

                // 5) Build review matrix
                for mr in &reviewed_mrs {
                    let author = mr["author"]["username"].as_str().unwrap_or("?").to_string();
                    *all_reviewed_authors.entry(author).or_insert(0) += 1;
                }

                // 6) Count approvals/comments/commits via events API
                // Only fetch events once (for the first project), then filter across all projects
                if proj == projects.first().unwrap() {
                    let cache_key = format!("user:{username}");
                    let users: Vec<Value> = client
                        .get_cached(&cache_key, "/users", &[("username", username)], 60)
                        .await
                        .unwrap_or_default();

                    let user_id = users.first().and_then(|u| u["id"].as_u64()).unwrap_or(0);
                    let since_ts = (chrono::Utc::now() - chrono::Duration::days(days as i64)).timestamp();
                    let events = if user_id > 0 {
                        fetch_user_events(&client, user_id, since_ts).await.unwrap_or_default()
                    } else {
                        Vec::new()
                    };

                    // Resolve all project numeric IDs for filtering
                    let mut project_numeric_ids: Vec<u64> = Vec::new();
                    for p in &projects {
                        let enc = urlencoding::encode(p);
                        let info: Option<Value> = client
                            .get_cached(
                                &format!("project_info:{enc}"),
                                &format!("/projects/{enc}"),
                                &[("simple", "true")],
                                60,
                            )
                            .await
                            .ok();
                        if let Some(id) = info.as_ref().and_then(|v| v["id"].as_u64()) {
                            project_numeric_ids.push(id);
                        }
                    }

                    for event in &events {
                        let event_pid = event["project_id"].as_u64();
                        if !project_numeric_ids.is_empty() {
                            if let Some(epid) = event_pid {
                                if !project_numeric_ids.contains(&epid) {
                                    continue;
                                }
                            }
                        }

                        let action = event["action_name"].as_str().unwrap_or("");
                        let target_type = event["target_type"].as_str().unwrap_or("");

                        match (action, target_type) {
                            ("approved", "MergeRequest") => total_approvals += 1,
                            ("commented on", "MergeRequest") => total_comments += 1,
                            ("pushed to", _) | ("pushed new", _) => {
                                let raw = event["push_data"]["commit_count"].as_u64().unwrap_or(1);
                                let count = if raw > 20 { 1 } else { raw };
                                total_commits += count;
                            }
                            _ => {}
                        }
                    }
                }
            }

            let avg_merge = if all_merge_hours.is_empty() {
                0.0
            } else {
                all_merge_hours.iter().sum::<f64>() / all_merge_hours.len() as f64
            };

            DevStats {
                username: username.to_string(),
                mrs_opened: total_opened,
                mrs_merged: total_merged_count,
                mrs_reviewed: total_reviewed_count,
                reviewed_authors: all_reviewed_authors,
                approvals_given: total_approvals,
                avg_merge_hours: avg_merge,
                mr_comments: total_comments,
                commits: total_commits,
                additions: total_additions,
                deletions: total_deletions,
                files_changed: total_files,
                mr_sizes: (mr_small, mr_medium, mr_large),
            }
        }
    }).collect();

    let results = join_all(futures).await;

    if summary_only {
        let mut lines: Vec<String> = Vec::new();
        for s in &results {
            let merge_str = if s.mrs_merged == 0 { "–".to_string() } else { format!("{:.1}h avg merge", s.avg_merge_hours) };
            lines.push(format!(
                "@{}: {} commits, +{}/-{} LOC, {} MRs merged, {} reviewed, {}",
                s.username, s.commits, s.additions, s.deletions,
                s.mrs_merged, s.mrs_reviewed, merge_str
            ));
        }
        return Ok(lines.join("\n"));
    }

    // Resolve project display name(s)
    let project_label = if projects.len() == 1 {
        let encoded_project = urlencoding::encode(projects[0]);
        client
            .get_cached::<Value>(
                &format!("project_info:{encoded_project}"),
                &format!("/projects/{encoded_project}"),
                &[("simple", "true")],
                60,
            )
            .await
            .ok()
            .and_then(|p| p["path_with_namespace"].as_str().map(|s| s.to_string()))
            .unwrap_or_else(|| project_id.to_string())
    } else {
        format!("{} projects", projects.len())
    };

    // Build markdown table
    let mut header = "| Metric".to_string();
    for stats in &results {
        header.push_str(&format!(" | @{}", stats.username));
    }
    header.push_str(" |");

    let mut separator = "|--------".to_string();
    for _ in &results {
        separator.push_str("|-------");
    }
    separator.push_str("|");

    let metrics: Vec<(&str, Box<dyn Fn(&DevStats) -> String>)> = vec![
        ("Commits", Box::new(|s: &DevStats| s.commits.to_string())),
        ("Lines added", Box::new(|s: &DevStats| format!("+{}", s.additions))),
        ("Lines deleted", Box::new(|s: &DevStats| format!("-{}", s.deletions))),
        ("Files changed", Box::new(|s: &DevStats| s.files_changed.to_string())),
        ("MRs opened", Box::new(|s: &DevStats| s.mrs_opened.to_string())),
        ("MRs merged", Box::new(|s: &DevStats| s.mrs_merged.to_string())),
        ("MR sizes (S/M/L)", Box::new(|s: &DevStats| format!("{}/{}/{}", s.mr_sizes.0, s.mr_sizes.1, s.mr_sizes.2))),
        ("MRs reviewed", Box::new(|s: &DevStats| s.mrs_reviewed.to_string())),
        ("Approvals given", Box::new(|s: &DevStats| s.approvals_given.to_string())),
        ("Avg merge time", Box::new(|s: &DevStats| {
            if s.mrs_merged == 0 { "–".to_string() } else { format!("{:.1}h", s.avg_merge_hours) }
        })),
        ("Comments on MRs", Box::new(|s: &DevStats| s.mr_comments.to_string())),
    ];

    let mut lines = vec![
        format!("## Developer Comparison: {} (last {} days)\n", project_label, days),
        header,
        separator,
    ];

    for (metric_name, formatter) in &metrics {
        let mut row = format!("| {metric_name}");
        for stats in &results {
            row.push_str(&format!(" | {}", formatter(stats)));
        }
        row.push_str(" |");
        lines.push(row);
    }

    // Review matrix
    let all_usernames: Vec<&str> = results.iter().map(|s| s.username.as_str()).collect();
    let has_reviews = results.iter().any(|s| !s.reviewed_authors.is_empty());
    if has_reviews {
        lines.push(String::new());
        lines.push("### Review Matrix (who reviewed whom)\n".to_string());
        let mut matrix_header = "| Reviewer \\ Author".to_string();
        for u in &all_usernames {
            matrix_header.push_str(&format!(" | @{u}"));
        }
        matrix_header.push_str(" |");
        lines.push(matrix_header);

        let mut matrix_sep = "|---".to_string();
        for _ in &all_usernames {
            matrix_sep.push_str("|---");
        }
        matrix_sep.push_str("|");
        lines.push(matrix_sep);

        for reviewer in &results {
            let mut row = format!("| @{}", reviewer.username);
            for author_name in &all_usernames {
                let count = reviewer.reviewed_authors.get(*author_name).unwrap_or(&0);
                let cell = if *count == 0 { "–".to_string() } else { count.to_string() };
                row.push_str(&format!(" | {cell}"));
            }
            row.push_str(" |");
            lines.push(row);
        }
    }

    Ok(lines.join("\n"))
}
