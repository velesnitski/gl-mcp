//! GitLab repository tools: search code, tree, languages, compare, tags.

use crate::client::GitLabClient;
use crate::error::{Error, Result};
use serde_json::Value;

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
            format!("{}...", &data[..200])
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
