//! GitLab repository tools: search code, tree, languages, compare, tags, stats.

use crate::client::GitLabClient;
use crate::error::{Error, Result};
use serde_json::Value;
use std::collections::BTreeMap;
use crate::tools::commits::detect_language;

/// Search code across a project (GitLab blobs search).
pub async fn search_code(
    client: &GitLabClient,
    project_id: &str,
    query: &str,
    ref_name: &str,
    per_page: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let per_page_str = per_page.to_string();

    let mut params: Vec<(&str, &str)> = vec![
        ("scope", "blobs"),
        ("search", query),
        ("per_page", &per_page_str),
    ];
    if !ref_name.is_empty() {
        params.push(("ref", ref_name));
    }

    let results: Vec<Value> = client
        .get(&format!("/projects/{encoded}/search"), &params)
        .await
        ?;

    if results.is_empty() {
        return Ok(format!("No results for '{query}' in {project_id}."));
    }

    let mut lines = vec![format!(
        "**Search '{query}' in {project_id}: {} results**\n",
        results.len()
    )];

    for r in &results {
        let path = r["path"].as_str().unwrap_or("?");
        let startline = r["startline"].as_u64().unwrap_or(0);
        let data = r["data"].as_str().unwrap_or("").trim();

        // Truncate long matches
        let preview = if data.len() > 200 {
            let truncated: String = data.chars().take(200).collect();
            format!("{truncated}...")
        } else {
            data.to_string()
        };

        lines.push(format!("**{}:{}**", path, startline));
        lines.push(format!("```\n{}\n```\n", preview));
    }

    Ok(lines.join("\n"))
}

/// Get project language breakdown.
pub async fn get_languages(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let langs: Value = client
        .get(&format!("/projects/{encoded}/languages"), &[])
        .await
        ?;

    let obj = langs.as_object().ok_or(Error::Other("Invalid response".into()))?;
    if obj.is_empty() {
        return Ok(format!("No language data for {project_id}."));
    }

    // Sort by percentage descending
    let mut entries: Vec<(&String, f64)> = obj
        .iter()
        .filter_map(|(k, v)| v.as_f64().map(|pct| (k, pct)))
        .collect();
    entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

    let mut lines = vec![format!("**{project_id} — Languages**\n")];
    for (lang, pct) in &entries {
        let bar_len = (*pct / 5.0) as usize; // 20 chars = 100%
        let bar: String = "█".repeat(bar_len);
        lines.push(format!("{:>12} {:5.1}% {}", lang, pct, bar));
    }

    Ok(lines.join("\n"))
}

/// Get repository tree (directory listing).
pub async fn get_tree(
    client: &GitLabClient,
    project_id: &str,
    path: &str,
    ref_name: &str,
    recursive: bool,
    per_page: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let per_page_str = per_page.to_string();
    let recursive_str = if recursive { "true" } else { "false" };

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
        ("recursive", recursive_str),
    ];
    if !path.is_empty() {
        params.push(("path", path));
    }
    if !ref_name.is_empty() {
        params.push(("ref", ref_name));
    }

    let entries: Vec<Value> = client
        .get(&format!("/projects/{encoded}/repository/tree"), &params)
        .await
        ?;

    if entries.is_empty() {
        let path_str = if path.is_empty() { "root" } else { path };
        return Ok(format!("No entries at '{path_str}' in {project_id}."));
    }

    let path_label = if path.is_empty() { "/" } else { path };
    let mut lines = vec![format!(
        "**{project_id}:{path_label}** ({} entries)\n",
        entries.len()
    )];

    for entry in &entries {
        let name = entry["name"].as_str().unwrap_or("?");
        let entry_path = entry["path"].as_str().unwrap_or(name);
        let entry_type = entry["type"].as_str().unwrap_or("?");

        let icon = match entry_type {
            "tree" => "📁",
            "blob" => "📄",
            _ => "❓",
        };

        let display = if recursive { entry_path } else { name };
        lines.push(format!("{icon} {display}"));
    }

    Ok(lines.join("\n"))
}

/// Compare two branches/tags/commits.
pub async fn compare_branches(
    client: &GitLabClient,
    project_id: &str,
    from: &str,
    to: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let data: Value = client
        .get(
            &format!("/projects/{encoded}/repository/compare"),
            &[("from", from), ("to", to)],
        )
        .await
        ?;

    let commits = data["commits"].as_array().map(|a| a.len()).unwrap_or(0);
    let diffs = data["diffs"].as_array().map(|a| a.len()).unwrap_or(0);

    let mut total_add: u64 = 0;
    let mut total_del: u64 = 0;
    let mut files: Vec<String> = Vec::new();

    if let Some(diff_array) = data["diffs"].as_array() {
        for d in diff_array {
            let path = d["new_path"].as_str().unwrap_or("?");
            let diff_text = d["diff"].as_str().unwrap_or("");
            let is_new = d["new_file"].as_bool().unwrap_or(false);
            let is_deleted = d["deleted_file"].as_bool().unwrap_or(false);

            let mut add: u64 = 0;
            let mut del: u64 = 0;
            for line in diff_text.lines() {
                if line.starts_with('+') && !line.starts_with("+++") { add += 1; }
                if line.starts_with('-') && !line.starts_with("---") { del += 1; }
            }
            total_add += add;
            total_del += del;

            let status = if is_new { " (new)" } else if is_deleted { " (deleted)" } else { "" };
            files.push(format!("  {path}{status} +{add} -{del}"));
        }
    }

    let mut lines = vec![
        format!("**Compare {from} → {to}** in {project_id}"),
        format!("**Commits:** {commits} | **Files:** {diffs} | **Lines:** +{total_add} -{total_del}"),
        String::new(),
    ];

    // Commit list
    if let Some(commit_array) = data["commits"].as_array() {
        lines.push("### Commits".to_string());
        for c in commit_array.iter().rev().take(20) {
            let sha = c["short_id"].as_str().unwrap_or("?");
            let title = c["title"].as_str().unwrap_or("?");
            let author = c["author_name"].as_str().unwrap_or("?");
            lines.push(format!("- `{sha}` {title} — @{author}"));
        }
        if commit_array.len() > 20 {
            lines.push(format!("  ...and {} more", commit_array.len() - 20));
        }
        lines.push(String::new());
    }

    // Files
    if !files.is_empty() {
        lines.push(format!("### Files ({})", files.len()));
        lines.extend(files.into_iter().take(50));
        if diffs > 50 {
            lines.push(format!("  ...and {} more files", diffs - 50));
        }
    }

    Ok(lines.join("\n"))
}

/// List tags for a project.
pub async fn list_tags(
    client: &GitLabClient,
    project_id: &str,
    search: &str,
    per_page: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let per_page_str = per_page.to_string();

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
        ("order_by", "updated"),
        ("sort", "desc"),
    ];
    if !search.is_empty() {
        params.push(("search", search));
    }

    let tags: Vec<Value> = client
        .get(&format!("/projects/{encoded}/repository/tags"), &params)
        .await
        ?;

    if tags.is_empty() {
        return Ok("No tags found.".to_string());
    }

    let mut lines = vec![format!("**Tags: {}**\n", tags.len())];
    for t in &tags {
        let name = t["name"].as_str().unwrap_or("?");
        let msg = t["message"].as_str().unwrap_or("");
        let sha = t["commit"]["short_id"].as_str().unwrap_or("?");
        let date = t["commit"]["created_at"].as_str().unwrap_or("?");
        let date_short = if date.len() > 10 { &date[..10] } else { date };

        let msg_str = if msg.is_empty() { String::new() } else { format!(" — {msg}") };
        lines.push(format!("- **{name}** `{sha}` ({date_short}){msg_str}"));
    }

    Ok(lines.join("\n"))
}

/// Get MR approvals info.
pub async fn get_mr_approvals(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let data: Value = client
        .get(
            &format!("/projects/{encoded}/merge_requests/{mr_iid}/approvals"),
            &[],
        )
        .await
        ?;

    let approved = data["approved"].as_bool().unwrap_or(false);
    let approvals_required = data["approvals_required"].as_u64().unwrap_or(0);
    let approvals_left = data["approvals_left"].as_u64().unwrap_or(0);

    let approved_by: Vec<String> = data["approved_by"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|u| u["user"]["username"].as_str().map(|s| format!("@{s}")))
                .collect()
        })
        .unwrap_or_default();

    let mut lines = vec![
        format!("**MR !{mr_iid} Approvals**"),
        format!(
            "**Status:** {} | **Required:** {} | **Remaining:** {}",
            if approved { "Approved" } else { "Pending" },
            approvals_required,
            approvals_left,
        ),
    ];

    if !approved_by.is_empty() {
        lines.push(format!("**Approved by:** {}", approved_by.join(", ")));
    }

    Ok(lines.join("\n"))
}

/// Create or update a file in a GitLab repository.
/// Always creates a new branch — never writes to main/master directly.
pub async fn update_file(
    client: &GitLabClient,
    project_id: &str,
    file_path: &str,
    content: &str,
    branch: &str,
    commit_message: &str,
    source_branch: &str,
    create_mr: bool,
) -> Result<String> {
    let encoded_project = urlencoding::encode(project_id);
    let encoded_file = urlencoding::encode(file_path);

    // Safety: never write to main/master/develop directly
    let protected = ["main", "master", "develop", "release", "production"];
    if protected.iter().any(|p| branch.eq_ignore_ascii_case(p)) {
        return Err(Error::Other(format!(
            "Cannot write directly to protected branch '{branch}'. Use a feature branch."
        )));
    }

    let from_branch = if source_branch.is_empty() { "main" } else { source_branch };

    // Check if file exists (create vs update)
    let action = {
        let check = client
            .get::<Value>(
                &format!("/projects/{encoded_project}/repository/files/{encoded_file}"),
                &[("ref", from_branch)],
            )
            .await;
        if check.is_ok() { "update" } else { "create" }
    };

    // Commit via commits API (handles branch creation automatically)
    let payload = serde_json::json!({
        "branch": branch,
        "start_branch": from_branch,
        "commit_message": commit_message,
        "actions": [{
            "action": action,
            "file_path": file_path,
            "content": content,
        }]
    });

    let result: Value = client
        .post(&format!("/projects/{encoded_project}/repository/commits"), &payload)
        .await?;

    let sha = result["id"].as_str().unwrap_or("?");
    let short_sha = if sha.len() > 8 { &sha[..8] } else { sha };
    let web_url = result["web_url"].as_str().unwrap_or("");

    let mut lines = vec![
        format!("**{action}d** `{file_path}` on branch `{branch}`"),
        format!("**Commit:** `{short_sha}` — {commit_message}"),
    ];

    if !web_url.is_empty() {
        lines.push(format!("**URL:** {web_url}"));
    }

    if create_mr {
        let mr_payload = serde_json::json!({
            "source_branch": branch,
            "target_branch": from_branch,
            "title": commit_message,
            "remove_source_branch": true,
        });

        match client
            .post::<Value>(
                &format!("/projects/{encoded_project}/merge_requests"),
                &mr_payload,
            )
            .await
        {
            Ok(mr) => {
                let mr_iid = mr["iid"].as_u64().unwrap_or(0);
                let mr_url = mr["web_url"].as_str().unwrap_or("");
                lines.push(format!("**MR:** !{mr_iid} — {mr_url}"));
            }
            Err(e) => {
                lines.push(format!("**MR creation failed:** {e}"));
            }
        }
    }

    Ok(lines.join("\n"))
}

/// List project environments (deployments).
pub async fn list_environments(
    client: &GitLabClient,
    project_id: &str,
    per_page: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let per_page_str = per_page.to_string();

    let envs: Vec<Value> = client
        .get(
            &format!("/projects/{encoded}/environments"),
            &[("per_page", &per_page_str)],
        )
        .await?;

    if envs.is_empty() {
        return Ok(format!("No environments found for {project_id}."));
    }

    let mut lines = vec![format!("**{project_id} — {} environments**\n", envs.len())];

    for env in &envs {
        let name = env["name"].as_str().unwrap_or("?");
        let state = env["state"].as_str().unwrap_or("?");
        let url = env["external_url"].as_str().unwrap_or("");

        let deploy = &env["last_deployment"];
        let deploy_info = if deploy.is_null() {
            "no deployments".to_string()
        } else {
            let sha = deploy["sha"].as_str().unwrap_or("?");
            let short_sha = if sha.len() > 8 { &sha[..8] } else { sha };
            let ref_name = deploy["ref"].as_str().unwrap_or("?");
            let status = deploy["status"].as_str().unwrap_or("?");
            let created = deploy["created_at"].as_str().unwrap_or("?");
            let date_short = if created.len() > 16 { &created[..16] } else { created };
            let deployer = deploy["user"]["username"].as_str().unwrap_or("?");
            format!("`{short_sha}` on `{ref_name}` [{status}] by @{deployer} ({date_short})")
        };

        let url_str = if url.is_empty() { String::new() } else { format!(" — {url}") };
        lines.push(format!("- **{name}** [{state}]{url_str}"));
        lines.push(format!("  Last deploy: {deploy_info}"));
    }

    Ok(lines.join("\n"))
}

/// Get project contributor stats (all-time).
pub async fn get_contributors(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let contributors: Vec<Value> = client
        .get(&format!("/projects/{encoded}/repository/contributors"), &[("order_by", "commits"), ("sort", "desc")])
        .await?;

    if contributors.is_empty() {
        return Ok(format!("No contributor data for {project_id}."));
    }

    let total_commits: u64 = contributors.iter().map(|c| c["commits"].as_u64().unwrap_or(0)).sum();
    let total_add: u64 = contributors.iter().map(|c| c["additions"].as_u64().unwrap_or(0)).sum();
    let total_del: u64 = contributors.iter().map(|c| c["deletions"].as_u64().unwrap_or(0)).sum();

    let mut lines = vec![
        format!("**{project_id} — {} contributors**", contributors.len()),
        format!("**Total:** {total_commits} commits, +{total_add} -{total_del}\n"),
        format!("| Contributor | Commits | Additions | Deletions | % |"),
        format!("|------------|---------|-----------|-----------|---|"),
    ];

    for c in contributors.iter().take(20) {
        let name = c["name"].as_str().unwrap_or("?");
        let email = c["email"].as_str().unwrap_or("?");
        let commits = c["commits"].as_u64().unwrap_or(0);
        let additions = c["additions"].as_u64().unwrap_or(0);
        let deletions = c["deletions"].as_u64().unwrap_or(0);
        let pct = if total_commits > 0 { commits as f64 / total_commits as f64 * 100.0 } else { 0.0 };

        lines.push(format!("| {name} ({email}) | {commits} | +{additions} | -{deletions} | {pct:.0}% |"));
    }

    if contributors.len() > 20 {
        lines.push(format!("| ...and {} more | | | | |", contributors.len() - 20));
    }

    Ok(lines.join("\n"))
}

/// Get project-level MR approval rules.
pub async fn get_approval_rules(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    let rules: Vec<Value> = client
        .get(&format!("/projects/{encoded}/approval_rules"), &[])
        .await?;

    if rules.is_empty() {
        return Ok(format!("No approval rules configured for {project_id}."));
    }

    let mut lines = vec![format!("**{project_id} — {} approval rules**\n", rules.len())];

    for rule in &rules {
        let name = rule["name"].as_str().unwrap_or("?");
        let approvals_required = rule["approvals_required"].as_u64().unwrap_or(0);
        let rule_type = rule["rule_type"].as_str().unwrap_or("?");

        let eligible: Vec<&str> = rule["eligible_approvers"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v["username"].as_str()).collect())
            .unwrap_or_default();

        let groups: Vec<&str> = rule["groups"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v["name"].as_str()).collect())
            .unwrap_or_default();

        lines.push(format!("- **{name}** (type: {rule_type}, required: {approvals_required})"));
        if !eligible.is_empty() {
            lines.push(format!("  Approvers: {}", eligible.iter().map(|u| format!("@{u}")).collect::<Vec<_>>().join(", ")));
        }
        if !groups.is_empty() {
            lines.push(format!("  Groups: {}", groups.join(", ")));
        }
    }

    Ok(lines.join("\n"))
}

/// Get project statistics: file counts by type, languages, binary files, repo size.
pub async fn get_project_stats(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    // Fetch project metadata with statistics
    let project: Value = client
        .get(
            &format!("/projects/{encoded}"),
            &[("statistics", "true")],
        )
        .await?;

    let project_name = project["name"].as_str().unwrap_or(project_id);
    let repo_size = project["statistics"]["repository_size"].as_u64().unwrap_or(0);
    let storage_size = project["statistics"]["storage_size"].as_u64().unwrap_or(0);

    // Fetch tree recursively
    let entries: Vec<Value> = client
        .get_all_pages(
            &format!("/projects/{encoded}/repository/tree"),
            &[("recursive", "true")],
            10,
        )
        .await?;

    let total_files = entries.iter().filter(|e| e["type"].as_str() == Some("blob")).count();

    // Categorize files
    let source_extensions: &[&str] = &[
        ".swift", ".kt", ".kts", ".java", ".go", ".rs", ".py", ".rb",
        ".php", ".ts", ".tsx", ".js", ".jsx", ".vue", ".c", ".cpp", ".h",
        ".m", ".mm", ".cs", ".sql", ".sh", ".bash", ".r", ".scala",
    ];
    let config_extensions: &[&str] = &[
        ".json", ".yaml", ".yml", ".toml", ".xml", ".plist", ".properties",
        ".env", ".ini", ".cfg", ".conf", ".gradle", ".tf", ".tfvars", ".hcl",
    ];
    let doc_extensions: &[&str] = &[
        ".md", ".txt", ".rst", ".adoc", ".html", ".css", ".scss", ".less",
    ];
    let binary_extensions: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".ico", ".svg", ".bmp", ".tiff",
        ".woff", ".woff2", ".ttf", ".eot", ".otf",
        ".zip", ".tar", ".gz", ".rar", ".7z",
        ".pdf", ".doc", ".docx", ".xls", ".xlsx",
        ".mp3", ".mp4", ".wav", ".avi", ".mov",
        ".o", ".obj", ".exe", ".dll", ".class", ".jar",
        ".a", ".dylib", ".so", ".framework",
        ".dat", ".bin", ".db", ".sqlite",
    ];
    let binary_dirs: &[&str] = &[
        ".xcframework/", ".framework/",
    ];

    let mut source_count = 0usize;
    let mut config_count = 0usize;
    let mut doc_count = 0usize;
    let mut binary_count = 0usize;
    let mut other_count = 0usize;
    let mut binary_files: Vec<String> = Vec::new();

    // Language distribution from source files
    let mut lang_counts: BTreeMap<String, usize> = BTreeMap::new();

    for entry in &entries {
        if entry["type"].as_str() != Some("blob") {
            continue;
        }
        let path = entry["path"].as_str().unwrap_or("?");

        let is_binary_dir = binary_dirs.iter().any(|d| path.contains(d));
        let is_binary_ext = binary_extensions.iter().any(|ext| path.ends_with(ext));

        if is_binary_dir || is_binary_ext {
            binary_count += 1;
            binary_files.push(path.to_string());
        } else if source_extensions.iter().any(|ext| path.ends_with(ext)) {
            source_count += 1;
            let lang = detect_language(path);
            *lang_counts.entry(lang.to_string()).or_insert(0) += 1;
        } else if config_extensions.iter().any(|ext| path.ends_with(ext)) {
            config_count += 1;
        } else if doc_extensions.iter().any(|ext| path.ends_with(ext)) {
            doc_count += 1;
        } else {
            other_count += 1;
        }
    }

    // Fetch language breakdown from GitLab API
    let langs: Value = client
        .get(&format!("/projects/{encoded}/languages"), &[])
        .await
        .unwrap_or(Value::Object(serde_json::Map::new()));

    // Format sizes
    fn format_size(bytes: u64) -> String {
        if bytes >= 1_073_741_824 {
            format!("{:.1} GB", bytes as f64 / 1_073_741_824.0)
        } else if bytes >= 1_048_576 {
            format!("{:.1} MB", bytes as f64 / 1_048_576.0)
        } else if bytes >= 1024 {
            format!("{:.1} KB", bytes as f64 / 1024.0)
        } else {
            format!("{} B", bytes)
        }
    }

    let mut out = vec![
        format!("## Project Stats: {project_name}\n"),
        "| Metric | Value |".to_string(),
        "|--------|-------|".to_string(),
        format!("| Repository size | {} |", format_size(repo_size)),
        format!("| Storage size | {} |", format_size(storage_size)),
        format!("| Total files | {} |", total_files),
        format!("| Source files | {} |", source_count),
        format!("| Config files | {} |", config_count),
        format!("| Documentation | {} |", doc_count),
        format!("| Binary files | {} |", binary_count),
        format!("| Other | {} |", other_count),
    ];

    // Languages from GitLab API
    if let Some(obj) = langs.as_object() {
        if !obj.is_empty() {
            let mut lang_entries: Vec<(&String, f64)> = obj
                .iter()
                .filter_map(|(k, v)| v.as_f64().map(|pct| (k, pct)))
                .collect();
            lang_entries.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

            let lang_str: String = lang_entries
                .iter()
                .map(|(lang, pct)| format!("{} {:.1}%", lang, pct))
                .collect::<Vec<_>>()
                .join(", ");

            out.push(format!("\n### Languages\n{}", lang_str));
        }
    }

    // Source files by language (from tree analysis)
    if !lang_counts.is_empty() {
        let mut sorted_langs: Vec<_> = lang_counts.iter().collect();
        sorted_langs.sort_by(|a, b| b.1.cmp(a.1));

        out.push("\n### Source Files by Language".to_string());
        for (lang, count) in sorted_langs {
            out.push(format!("- **{lang}**: {count} files"));
        }
    }

    // Binary files
    if !binary_files.is_empty() {
        out.push(format!(
            "\n### Binary Files ({} — consider LFS or CI builds)",
            binary_files.len()
        ));
        for f in binary_files.iter().take(30) {
            out.push(format!("- {f}"));
        }
        if binary_files.len() > 30 {
            out.push(format!("  ...and {} more", binary_files.len() - 30));
        }
    }

    Ok(out.join("\n"))
}

/// Get deployment frequency for a project (DORA metric).
pub async fn get_deploy_frequency(
    client: &GitLabClient,
    project_id: &str,
    environment: &str,
    days: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", "100"),
        ("updated_after", &since),
        ("order_by", "updated_at"),
        ("sort", "desc"),
        ("status", "success"),
    ];
    if !environment.is_empty() {
        params.push(("environment", environment));
    }

    let deployments: Vec<Value> = client
        .get(&format!("/projects/{encoded}/deployments"), &params)
        .await?;

    if deployments.is_empty() {
        return Ok(format!("No successful deployments in the last {days} days for {project_id}."));
    }

    // Group by day and environment
    let mut by_env: BTreeMap<String, BTreeMap<String, u32>> = BTreeMap::new();
    let mut deployers: BTreeMap<String, u32> = BTreeMap::new();

    for d in &deployments {
        let env_name = d["environment"]["name"].as_str().unwrap_or("?").to_string();
        let created = d["created_at"].as_str().unwrap_or("");
        let day = if created.len() >= 10 { &created[..10] } else { created };
        let deployer = d["user"]["username"].as_str().unwrap_or("?").to_string();

        *by_env.entry(env_name).or_default().entry(day.to_string()).or_default() += 1;
        *deployers.entry(deployer).or_default() += 1;
    }

    let total = deployments.len();
    let freq = total as f64 / days as f64;

    let mut lines = vec![
        format!("**Deploy Frequency: {project_id}** (last {days}d)\n"),
        format!("| Metric | Value |"),
        format!("|--------|-------|"),
        format!("| Total deploys | {} |", total),
        format!("| Per day | {:.1} |", freq),
    ];

    // Per-environment breakdown
    lines.push(String::new());
    lines.push("**By environment:**".to_string());
    for (env, days_map) in &by_env {
        let env_total: u32 = days_map.values().sum();
        let env_freq = env_total as f64 / days as f64;
        lines.push(format!("- **{env}**: {env_total} deploys ({env_freq:.1}/day)"));
    }

    // Per-deployer
    lines.push(String::new());
    lines.push("**By deployer:**".to_string());
    let mut sorted_deployers: Vec<_> = deployers.iter().collect();
    sorted_deployers.sort_by(|a, b| b.1.cmp(a.1));
    for (deployer, count) in sorted_deployers {
        lines.push(format!("- @{deployer}: {count} deploys"));
    }

    // Daily timeline (last 7 days max)
    let mut all_days: BTreeMap<String, u32> = BTreeMap::new();
    for days_map in by_env.values() {
        for (day, count) in days_map {
            *all_days.entry(day.clone()).or_default() += count;
        }
    }
    if all_days.len() > 1 {
        lines.push(String::new());
        lines.push("**Daily:**".to_string());
        for (day, count) in all_days.iter().rev().take(7) {
            lines.push(format!("- {day}: {count} deploys"));
        }
    }

    Ok(lines.join("\n"))
}
