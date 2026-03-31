//! GitLab commit and diff tools with smart filtering and token compression.

use crate::client::GitLabClient;
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
        _ if path.contains("Dockerfile") => "Docker",
        _ if path.contains("Makefile") => "Make",
        _ if path.contains(".github/") || path.contains(".gitlab-ci") => "CI/CD",
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
) -> Result<String, String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/projects/{}/repository/commits",
        urlencoding::encode(project_id)
    );

    let mut params: Vec<(&str, &str)> = vec![("per_page", &per_page_str)];
    if !branch.is_empty() {
        params.push(("ref_name", branch));
    }
    if !author.is_empty() {
        params.push(("author", author));
    }
    if !since.is_empty() {
        params.push(("since", since));
    }
    if !until.is_empty() {
        params.push(("until", until));
    }

    let commits: Vec<Value> = client.get(&path, &params).await.map_err(|e| e.to_string())?;

    if commits.is_empty() {
        return Ok("No commits found.".to_string());
    }

    let mut by_author: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
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
) -> Result<String, String> {
    let encoded = urlencoding::encode(project_id);

    let commit: Value = client
        .get(&format!("/projects/{encoded}/repository/commits/{sha}"), &[])
        .await
        .map_err(|e| e.to_string())?;

    let title = commit["title"].as_str().unwrap_or("?");
    let author = commit["author_name"].as_str().unwrap_or("?");
    let date = commit["created_at"].as_str().unwrap_or("?");
    let message = commit["message"].as_str().unwrap_or("");
    let stats_add = commit["stats"]["additions"].as_u64().unwrap_or(0);
    let stats_del = commit["stats"]["deletions"].as_u64().unwrap_or(0);

    let diffs: Vec<Value> = client
        .get(&format!("/projects/{encoded}/repository/commits/{sha}/diff"), &[])
        .await
        .map_err(|e| e.to_string())?;

    let (files, skipped) = process_diffs(&diffs, skip_generated, file_filter);

    let mut parts = if compact {
        vec![
            format!("{sha}|{author}|{date}"),
            message.to_string(),
            format!("+{stats_add}|-{stats_del}|{}f", diffs.len()),
            String::new(),
        ]
    } else {
        vec![
            format!("## Commit `{sha}` by {author}"),
            format!("**Date:** {date}"),
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
) -> Result<String, String> {
    let encoded = urlencoding::encode(project_id);

    let mr: Value = client
        .get(&format!("/projects/{encoded}/merge_requests/{mr_iid}"), &[])
        .await
        .map_err(|e| e.to_string())?;

    let title = mr["title"].as_str().unwrap_or("?");
    let author = mr["author"]["username"].as_str().unwrap_or("?");
    let source = mr["source_branch"].as_str().unwrap_or("?");
    let target = mr["target_branch"].as_str().unwrap_or("?");
    let state = mr["state"].as_str().unwrap_or("?");

    let changes_data: Value = client
        .get(&format!("/projects/{encoded}/merge_requests/{mr_iid}/changes"), &[])
        .await
        .map_err(|e| e.to_string())?;

    let changes = changes_data["changes"].as_array().cloned().unwrap_or_default();
    let (files, skipped) = process_diffs(&changes, skip_generated, file_filter);

    let mut parts = if compact {
        vec![
            format!("!{mr_iid}|{title}|{state}"),
            format!("{author}|{source}→{target}|{}f", changes.len()),
            String::new(),
        ]
    } else {
        vec![
            format!("## MR !{mr_iid}: {title}"),
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
) -> Result<String, String> {
    let encoded_project = urlencoding::encode(project_id);
    let encoded_file = urlencoding::encode(file_path);

    let data: Value = client
        .get(
            &format!("/projects/{encoded_project}/repository/files/{encoded_file}"),
            &[("ref", ref_name)],
        )
        .await
        .map_err(|e| e.to_string())?;

    let content_b64 = data["content"].as_str().unwrap_or("");
    let encoding = data["encoding"].as_str().unwrap_or("base64");
    let size = data["size"].as_u64().unwrap_or(0);
    let lang = detect_language(file_path);

    let content = if encoding == "base64" {
        let decoded = base64_decode(content_b64).map_err(|e| format!("Base64 decode error: {e}"))?;
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
pub async fn get_user_activity(
    client: &GitLabClient,
    username: &str,
    hours: u32,
) -> Result<String, String> {
    // Resolve user ID
    let users: Vec<Value> = client
        .get("/users", &[("username", username)])
        .await
        .map_err(|e| e.to_string())?;

    let user = users.first().ok_or_else(|| format!("User @{username} not found"))?;
    let user_id = user["id"].as_u64().ok_or("User has no ID")?;
    let display_name = user["name"].as_str().unwrap_or(username);

    // Calculate since date
    let since = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
    let since_str = since.format("%Y-%m-%d").to_string();

    // Fetch events (paginated)
    let mut all_events: Vec<Value> = Vec::new();
    let mut page = 1u32;
    loop {
        let page_str = page.to_string();
        let events: Vec<Value> = client
            .get(
                &format!("/users/{user_id}/events"),
                &[("after", &since_str), ("per_page", "100"), ("page", &page_str)],
            )
            .await
            .map_err(|e| e.to_string())?;

        if events.is_empty() {
            break;
        }
        all_events.extend(events);
        page += 1;
        if page > 5 {
            break; // Safety limit
        }
    }

    // Filter to exact time window
    let since_ts = since.timestamp();
    let filtered: Vec<&Value> = all_events
        .iter()
        .filter(|e| {
            e["created_at"]
                .as_str()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.timestamp() >= since_ts)
                .unwrap_or(false)
        })
        .collect();

    // Aggregate
    let mut commits = 0u64;
    let mut mr_opened = 0u64;
    let mut mr_merged = 0u64;
    let mut mr_approved = 0u64;
    let mut projects: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut actions: BTreeMap<String, u64> = BTreeMap::new();

    for event in &filtered {
        let action = event["action_name"].as_str().unwrap_or("unknown");
        *actions.entry(action.to_string()).or_default() += 1;

        if let Some(proj) = event["project_id"].as_u64() {
            projects.insert(proj.to_string());
        }

        // Count commits from push events
        if action == "pushed to" || action == "pushed new" {
            let count = event["push_data"]["commit_count"].as_u64().unwrap_or(1);
            commits += count;
        }

        // MR events
        let target_type = event["target_type"].as_str().unwrap_or("");
        if target_type == "MergeRequest" {
            match action {
                "opened" => mr_opened += 1,
                "accepted" => mr_merged += 1,
                "approved" => mr_approved += 1,
                _ => {}
            }
        }
    }

    let mut lines = vec![
        format!("## Activity: @{username} ({display_name})"),
        format!("**Period:** last {hours}h | **Total events:** {}", filtered.len()),
        String::new(),
        format!("| Metric | Count |"),
        format!("|--------|-------|"),
        format!("| Commits | {commits} |"),
        format!("| MRs opened | {mr_opened} |"),
        format!("| MRs merged | {mr_merged} |"),
        format!("| MRs approved | {mr_approved} |"),
        format!("| Projects active | {} |", projects.len()),
    ];

    if !actions.is_empty() {
        lines.push(String::new());
        lines.push("### Activity breakdown".to_string());
        for (action, count) in &actions {
            lines.push(format!("- {action}: {count}"));
        }
    }

    Ok(lines.join("\n"))
}

/// List projects in a GitLab group (with subgroups).
pub async fn list_group_projects(
    client: &GitLabClient,
    group_path: &str,
    per_page: u32,
) -> Result<String, String> {
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
        .map_err(|e| e.to_string())?;

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

fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
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
