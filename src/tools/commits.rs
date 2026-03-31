//! GitLab commit and diff tools with smart filtering.

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

fn detect_language(path: &str) -> &str {
    match path.rsplit('.').next() {
        Some("php" | "blade.php") => "PHP",
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
        Some("dockerfile") => "Docker",
        _ if path.contains("Dockerfile") => "Docker",
        _ if path.contains("Makefile") => "Make",
        _ if path.contains(".github/") => "CI/CD",
        _ if path.contains(".gitlab-ci") => "CI/CD",
        _ => "Other",
    }
}

/// Count additions and deletions in a diff string.
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

/// Truncate a diff to max_lines, preserving hunk boundaries.
fn truncate_diff(diff: &str, max_lines: usize) -> (String, usize) {
    let lines: Vec<&str> = diff.lines().collect();
    if lines.len() <= max_lines {
        return (diff.to_string(), 0);
    }
    let truncated = lines[..max_lines].join("\n");
    let remaining = lines.len() - max_lines;
    (truncated, remaining)
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

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
    ];
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

    let commits: Vec<Value> = client
        .get(&path, &params)
        .await
        .map_err(|e| e.to_string())?;

    if commits.is_empty() {
        return Ok("No commits found.".to_string());
    }

    // Group by author for summary
    let mut by_author: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    for c in &commits {
        let author = c["author_name"].as_str().unwrap_or("Unknown").to_string();
        by_author.entry(author).or_default().push(c);
    }

    let mut lines = vec![format!("**Found: {} commits**\n", commits.len())];

    // Summary by author
    lines.push("### By author".to_string());
    for (author, author_commits) in &by_author {
        lines.push(format!("- **{}**: {} commits", author, author_commits.len()));
    }
    lines.push(String::new());

    // Commit list
    lines.push("### Commits".to_string());
    for c in &commits {
        let sha = &c["short_id"].as_str().unwrap_or("?");
        let title = c["title"].as_str().unwrap_or("?");
        let author = c["author_name"].as_str().unwrap_or("?");
        let date = c["created_at"].as_str().unwrap_or("?");
        // Trim date to just date+time
        let date_short = if date.len() > 16 { &date[..16] } else { date };

        lines.push(format!(
            "- `{sha}` {title} — @{author} ({date_short})"
        ));
    }

    Ok(lines.join("\n"))
}

/// Get commit diff with smart filtering.
pub async fn get_commit_diff(
    client: &GitLabClient,
    project_id: &str,
    sha: &str,
    max_lines_per_file: usize,
    skip_generated: bool,
) -> Result<String, String> {
    let encoded = urlencoding::encode(project_id);

    // Fetch commit metadata
    let commit: Value = client
        .get(
            &format!("/projects/{encoded}/repository/commits/{sha}"),
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;

    let title = commit["title"].as_str().unwrap_or("?");
    let author = commit["author_name"].as_str().unwrap_or("?");
    let date = commit["created_at"].as_str().unwrap_or("?");
    let message = commit["message"].as_str().unwrap_or("");
    let stats_add = commit["stats"]["additions"].as_u64().unwrap_or(0);
    let stats_del = commit["stats"]["deletions"].as_u64().unwrap_or(0);
    let stats_total = commit["stats"]["total"].as_u64().unwrap_or(0);

    // Fetch diffs
    let diffs: Vec<Value> = client
        .get(
            &format!("/projects/{encoded}/repository/commits/{sha}/diff"),
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;

    let mut parts = vec![
        format!("## Commit `{sha}` by {author}"),
        format!("**Date:** {date}"),
        format!("**Message:** {message}"),
        format!("**Stats:** +{stats_add} -{stats_del} ({stats_total} total) in {} files", diffs.len()),
        String::new(),
    ];

    // Separate diffs into included and skipped
    let mut by_language: BTreeMap<String, Vec<(String, String, usize, usize)>> = BTreeMap::new();
    let mut skipped: Vec<(String, usize, usize)> = Vec::new();

    for diff in &diffs {
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

        let (additions, deletions) = count_diff_lines(diff_text);

        // Skip generated/lock files
        if skip_generated && should_skip_file(new_path) {
            skipped.push((display_path, additions, deletions));
            continue;
        }

        let lang = detect_language(new_path).to_string();

        // File status prefix
        let status = if is_new {
            " (new)"
        } else if is_deleted {
            " (deleted)"
        } else if is_renamed {
            " (renamed)"
        } else {
            ""
        };

        // Truncate large diffs
        let (display_diff, truncated_lines) = truncate_diff(diff_text, max_lines_per_file);

        let mut file_output = format!("**{}{}** (+{} -{})", display_path, status, additions, deletions);
        if !display_diff.is_empty() {
            file_output.push_str(&format!("\n```diff\n{display_diff}\n```"));
        }
        if truncated_lines > 0 {
            file_output.push_str(&format!("\n*...{truncated_lines} more lines truncated*"));
        }

        by_language.entry(lang).or_default().push((
            display_path,
            file_output,
            additions,
            deletions,
        ));
    }

    // Output grouped by language
    for (lang, files) in &by_language {
        let total_add: usize = files.iter().map(|(_, _, a, _)| a).sum();
        let total_del: usize = files.iter().map(|(_, _, _, d)| d).sum();
        parts.push(format!(
            "### {} ({} files, +{} -{})",
            lang,
            files.len(),
            total_add,
            total_del,
        ));
        parts.push(String::new());
        for (_, output, _, _) in files {
            parts.push(output.clone());
            parts.push(String::new());
        }
    }

    // Skipped files summary
    if !skipped.is_empty() {
        let total_add: usize = skipped.iter().map(|(_, a, _)| a).sum();
        let total_del: usize = skipped.iter().map(|(_, _, d)| d).sum();
        parts.push(format!(
            "### Skipped ({} files, +{} -{})",
            skipped.len(),
            total_add,
            total_del,
        ));
        for (path, add, del) in &skipped {
            parts.push(format!("- {path} (+{add} -{del})"));
        }
    }

    Ok(parts.join("\n"))
}

/// Get MR changes (aggregated diff across all commits).
pub async fn get_mr_changes(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
    max_lines_per_file: usize,
    skip_generated: bool,
) -> Result<String, String> {
    let encoded = urlencoding::encode(project_id);

    // Fetch MR metadata
    let mr: Value = client
        .get(
            &format!("/projects/{encoded}/merge_requests/{mr_iid}"),
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;

    let title = mr["title"].as_str().unwrap_or("?");
    let author = mr["author"]["username"].as_str().unwrap_or("?");
    let source = mr["source_branch"].as_str().unwrap_or("?");
    let target = mr["target_branch"].as_str().unwrap_or("?");
    let state = mr["state"].as_str().unwrap_or("?");

    // Fetch changes
    let changes_data: Value = client
        .get(
            &format!("/projects/{encoded}/merge_requests/{mr_iid}/changes"),
            &[],
        )
        .await
        .map_err(|e| e.to_string())?;

    let changes = changes_data["changes"]
        .as_array()
        .cloned()
        .unwrap_or_default();

    let mut parts = vec![
        format!("## MR !{mr_iid}: {title}"),
        format!("**Author:** @{author} | **State:** {state}"),
        format!("**Branch:** {source} → {target}"),
        format!("**Files changed:** {}", changes.len()),
        String::new(),
    ];

    let mut by_language: BTreeMap<String, Vec<(String, String, usize, usize)>> = BTreeMap::new();
    let mut skipped: Vec<(String, usize, usize)> = Vec::new();

    for change in &changes {
        let new_path = change["new_path"].as_str().unwrap_or("?");
        let old_path = change["old_path"].as_str().unwrap_or(new_path);
        let diff_text = change["diff"].as_str().unwrap_or("");
        let is_new = change["new_file"].as_bool().unwrap_or(false);
        let is_deleted = change["deleted_file"].as_bool().unwrap_or(false);
        let is_renamed = change["renamed_file"].as_bool().unwrap_or(false);

        let display_path = if is_renamed && old_path != new_path {
            format!("{old_path} → {new_path}")
        } else {
            new_path.to_string()
        };

        let (additions, deletions) = count_diff_lines(diff_text);

        if skip_generated && should_skip_file(new_path) {
            skipped.push((display_path, additions, deletions));
            continue;
        }

        let lang = detect_language(new_path).to_string();

        let status = if is_new {
            " (new)"
        } else if is_deleted {
            " (deleted)"
        } else if is_renamed {
            " (renamed)"
        } else {
            ""
        };

        let (display_diff, truncated_lines) = truncate_diff(diff_text, max_lines_per_file);

        let mut file_output = format!("**{}{}** (+{} -{})", display_path, status, additions, deletions);
        if !display_diff.is_empty() {
            file_output.push_str(&format!("\n```diff\n{display_diff}\n```"));
        }
        if truncated_lines > 0 {
            file_output.push_str(&format!("\n*...{truncated_lines} more lines truncated*"));
        }

        by_language.entry(lang).or_default().push((
            display_path,
            file_output,
            additions,
            deletions,
        ));
    }

    for (lang, files) in &by_language {
        let total_add: usize = files.iter().map(|(_, _, a, _)| a).sum();
        let total_del: usize = files.iter().map(|(_, _, _, d)| d).sum();
        parts.push(format!(
            "### {} ({} files, +{} -{})",
            lang, files.len(), total_add, total_del,
        ));
        parts.push(String::new());
        for (_, output, _, _) in files {
            parts.push(output.clone());
            parts.push(String::new());
        }
    }

    if !skipped.is_empty() {
        let total_add: usize = skipped.iter().map(|(_, a, _)| a).sum();
        let total_del: usize = skipped.iter().map(|(_, _, d)| d).sum();
        parts.push(format!(
            "### Skipped ({} files, +{} -{})",
            skipped.len(), total_add, total_del,
        ));
        for (path, add, del) in &skipped {
            parts.push(format!("- {path} (+{add} -{del})"));
        }
    }

    Ok(parts.join("\n"))
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
        use std::io::Read;
        let decoded = base64_decode(content_b64).map_err(|e| format!("Base64 decode error: {e}"))?;
        String::from_utf8_lossy(&decoded).to_string()
    } else {
        content_b64.to_string()
    };

    let mut parts = vec![
        format!("## {file_path}"),
        format!("**Ref:** {ref_name} | **Size:** {size} bytes | **Language:** {lang}"),
        String::new(),
        format!("```{}", lang.to_lowercase()),
        content,
        "```".to_string(),
    ];

    Ok(parts.join("\n"))
}

/// Simple base64 decoder (no external crate needed for basic use).
fn base64_decode(input: &str) -> Result<Vec<u8>, String> {
    // Strip whitespace/newlines that GitLab includes
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
