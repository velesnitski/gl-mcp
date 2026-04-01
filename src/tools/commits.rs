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

// ─── Response size warning ───

const RESPONSE_SIZE_WARN: usize = 15000;

fn maybe_warn_size(text: &str) -> String {
    if text.len() > RESPONSE_SIZE_WARN {
        let kb = text.len() / 1024;
        format!(
            "*Warning: Large response ({kb}KB). Use `summary_only=true` or `file=\"specific.php\"` to reduce token usage.*\n\n{text}"
        )
    } else {
        text.to_string()
    }
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
    Ok(maybe_warn_size(&result))
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
    Ok(maybe_warn_size(&result))
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
    Ok(maybe_warn_size(&result))
}

/// Get user activity (events) for the last N hours.
/// Fetch user events for a time window. Shared by get_user_activity and get_user_daily_report.
pub async fn fetch_user_events(
    client: &GitLabClient,
    user_id: u64,
    since_ts: i64,
) -> Result<Vec<Value>> {
    let mut all_events: Vec<Value> = Vec::new();
    let mut page = 1u32;
    loop {
        let page_str = page.to_string();
        let events: Vec<Value> = client
            .get(
                &format!("/users/{user_id}/events"),
                &[("per_page", "100"), ("page", &page_str), ("sort", "desc")],
            )
            .await
            ?;

        if events.is_empty() {
            break;
        }

        let oldest_in_window = events.iter().any(|e| {
            e["created_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp() >= since_ts)
                .unwrap_or(false)
        });

        all_events.extend(events);
        page += 1;

        if !oldest_in_window || page > 10 {
            break;
        }
    }

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

/// Resolve project IDs to names via batch lookup. Caches per-call.
pub async fn resolve_project_names(
    client: &GitLabClient,
    project_ids: &std::collections::BTreeSet<u64>,
) -> BTreeMap<u64, String> {
    let mut names = BTreeMap::new();
    for &pid in project_ids {
        if let Ok(proj) = client
            .get::<Value>(&format!("/projects/{pid}"), &[("simple", "true")])
            .await
        {
            let name = proj["path_with_namespace"]
                .as_str()
                .unwrap_or("?")
                .to_string();
            names.insert(pid, name);
        }
    }
    names
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
    let users: Vec<Value> = client
        .get("/users", &[("username", username)])
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
    let mut user_summaries: Vec<(String, UserSummary)> = Vec::new();

    for &username in usernames {
        // Resolve user
        let users: Vec<serde_json::Value> = client
            .get("/users", &[("username", username)])
            .await?;

        let user = match users.first() {
            Some(u) => u,
            None => continue,
        };
        let user_id = match user["id"].as_u64() {
            Some(id) => id,
            None => continue,
        };
        let display_name = user["name"].as_str().unwrap_or(username).to_string();

        // Fetch events
        let events = fetch_user_events(client, user_id, since_ts).await?;

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

        user_summaries.push((username.to_string(), summary));
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
