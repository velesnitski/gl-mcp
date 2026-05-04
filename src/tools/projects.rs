//! GitLab project tools.

use crate::client::GitLabClient;
use crate::error::Result;
use serde_json::Value;

/// List projects accessible to the authenticated user.
pub async fn list_projects(
    client: &GitLabClient,
    search: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let mut params = vec![
        ("per_page", per_page_str.as_str()),
        ("order_by", "last_activity_at"),
        ("sort", "desc"),
    ];
    if !search.is_empty() {
        params.push(("search", search));
    }

    let projects: Vec<Value> = client
        .get("/projects", &params)
        .await
        ?;

    if projects.is_empty() {
        return Ok("No projects found.".to_string());
    }

    let mut lines = vec![format!("**Found: {} projects**\n", projects.len())];
    for p in &projects {
        let name = p["path_with_namespace"].as_str().unwrap_or("?");
        let id = p["id"].as_u64().unwrap_or(0);
        let desc = p["description"].as_str().unwrap_or("");
        let desc_short = if desc.len() > 80 {
            let truncated: String = desc.chars().take(80).collect();
            format!("{truncated}...")
        } else {
            desc.to_string()
        };
        let stars = p["star_count"].as_u64().unwrap_or(0);
        let visibility = p["visibility"].as_str().unwrap_or("?");

        let desc_part = if desc_short.is_empty() {
            String::new()
        } else {
            format!(" — {desc_short}")
        };

        lines.push(format!(
            "- **{name}** (id: {id}) [{visibility}] ★{stars}{desc_part}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Get detailed info about a single project.
pub async fn get_project(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let path = format!("/projects/{}", urlencoding::encode(project_id));
    let p: Value = client
        .get(&path, &[])
        .await
        ?;

    let name = p["path_with_namespace"].as_str().unwrap_or("?");
    let id = p["id"].as_u64().unwrap_or(0);
    let desc = p["description"].as_str().unwrap_or("No description");
    let default_branch = p["default_branch"].as_str().unwrap_or("main");
    let visibility = p["visibility"].as_str().unwrap_or("?");
    let web_url = p["web_url"].as_str().unwrap_or("");
    let stars = p["star_count"].as_u64().unwrap_or(0);
    let forks = p["forks_count"].as_u64().unwrap_or(0);
    let open_issues = p["open_issues_count"].as_u64().unwrap_or(0);
    let created = p["created_at"].as_str().unwrap_or("?");
    let updated = p["last_activity_at"].as_str().unwrap_or("?");

    let topics: Vec<&str> = p["topics"]
        .as_array()
        .map(|arr| arr.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut parts = vec![
        format!("# {name}"),
        String::new(),
        format!("**ID:** {id}"),
        format!("**Visibility:** {visibility}"),
        format!("**Default branch:** {default_branch}"),
        format!("**URL:** {web_url}"),
        format!("**Stars:** {stars} | **Forks:** {forks} | **Open issues:** {open_issues}"),
        format!("**Created:** {created}"),
        format!("**Last activity:** {updated}"),
    ];

    if !topics.is_empty() {
        parts.push(format!("**Topics:** {}", topics.join(", ")));
    }

    if desc != "No description" {
        parts.push(String::new());
        parts.push(format!("## Description\n{desc}"));
    }

    Ok(parts.join("\n"))
}

/// List project members.
pub async fn list_members(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let path = format!("/projects/{}/members/all", urlencoding::encode(project_id));
    let members: Vec<Value> = client
        .get(&path, &[("per_page", "100")])
        .await
        ?;

    if members.is_empty() {
        return Ok("No members found.".to_string());
    }

    let mut lines = vec![format!("**Members: {}**\n", members.len())];
    for m in &members {
        let name = m["name"].as_str().unwrap_or("?");
        let username = m["username"].as_str().unwrap_or("?");
        let access = match m["access_level"].as_u64().unwrap_or(0) {
            10 => "Guest",
            20 => "Reporter",
            30 => "Developer",
            40 => "Maintainer",
            50 => "Owner",
            _ => "?",
        };
        lines.push(format!("- **{name}** (@{username}) — {access}"));
    }

    Ok(lines.join("\n"))
}

/// List branches for a project.
pub async fn list_branches(
    client: &GitLabClient,
    project_id: &str,
    search: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/projects/{}/repository/branches",
        urlencoding::encode(project_id)
    );

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
        ("order_by", "updated"),
        ("sort", "desc"),
    ];
    if !search.is_empty() {
        params.push(("search", search));
    }

    let branches: Vec<Value> = client
        .get(&path, &params)
        .await
        ?;

    if branches.is_empty() {
        return Ok("No branches found.".to_string());
    }

    let mut lines = vec![format!("**Branches: {}**\n", branches.len())];
    for b in &branches {
        let name = b["name"].as_str().unwrap_or("?");
        let is_default = b["default"].as_bool().unwrap_or(false);
        let is_protected = b["protected"].as_bool().unwrap_or(false);
        let author = b["commit"]["author_name"].as_str().unwrap_or("?");
        let date = b["commit"]["created_at"].as_str().unwrap_or("?");
        let date_short = if date.len() > 10 { &date[..10] } else { date };
        let message = b["commit"]["title"].as_str().unwrap_or("");

        let mut flags = Vec::new();
        if is_default { flags.push("default"); }
        if is_protected { flags.push("protected"); }
        let flag_str = if flags.is_empty() {
            String::new()
        } else {
            format!(" [{}]", flags.join(", "))
        };

        lines.push(format!(
            "- **{name}**{flag_str} — {message} (@{author}, {date_short})"
        ));
    }

    Ok(lines.join("\n"))
}

/// Delete a branch from a project.
pub async fn delete_branch(
    client: &GitLabClient,
    project_id: &str,
    branch: &str,
) -> Result<String> {
    let path = format!(
        "/projects/{}/repository/branches/{}",
        urlencoding::encode(project_id),
        urlencoding::encode(branch)
    );
    client.delete(&path).await?;
    Ok(format!("Branch `{branch}` deleted from **{project_id}**."))
}

/// Look up a GitLab user by username or numeric ID.
pub async fn get_user(
    client: &GitLabClient,
    username: &str,
    user_id: Option<u32>,
) -> Result<String> {
    let user: Value = if let Some(id) = user_id {
        let cache_key = format!("user_id:{id}");
        client.get_cached(&cache_key, &format!("/users/{id}"), &[], 60).await?
    } else {
        let cache_key = format!("user:{username}");
        let users: Vec<Value> = client
            .get_cached(&cache_key, "/users", &[("username", username)], 60)
            .await?;
        users.into_iter().next().ok_or_else(|| {
            crate::error::Error::Other(format!("User not found: {username}"))
        })?
    };

    let username = user["username"].as_str().unwrap_or("?");
    let name = user["name"].as_str().unwrap_or("?");
    let email = user["email"].as_str().or(user["public_email"].as_str()).unwrap_or("–");
    let state = user["state"].as_str().unwrap_or("?");
    let id = user["id"].as_u64().unwrap_or(0);
    let created = user["created_at"].as_str().unwrap_or("?");
    let last_sign_in = user["last_sign_in_at"].as_str().unwrap_or("–");
    let is_admin = user["is_admin"].as_bool().unwrap_or(false);
    let web_url = user["web_url"].as_str().unwrap_or("");

    let admin_str = if is_admin { " (admin)" } else { "" };

    let parts = vec![
        format!("# @{username} — {name}{admin_str}"),
        String::new(),
        format!("**ID:** {id}"),
        format!("**State:** {state}"),
        format!("**Email:** {email}"),
        format!("**Created:** {created}"),
        format!("**Last sign-in:** {last_sign_in}"),
        format!("**URL:** {web_url}"),
    ];

    Ok(parts.join("\n"))
}

/// Search GitLab users by name, username, or email.
pub async fn search_users(
    client: &GitLabClient,
    query: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let params = vec![
        ("search", query),
        ("per_page", &per_page_str),
    ];

    let users: Vec<Value> = client.get("/users", &params).await?;

    if users.is_empty() {
        return Ok("No users found.".to_string());
    }

    let mut lines = vec![format!("**Found: {} users**\n", users.len())];
    for u in &users {
        let username = u["username"].as_str().unwrap_or("?");
        let name = u["name"].as_str().unwrap_or("?");
        let state = u["state"].as_str().unwrap_or("?");
        let email = u["email"]
            .as_str()
            .or(u["public_email"].as_str())
            .unwrap_or("–");
        let is_admin = u["is_admin"].as_bool().unwrap_or(false);
        let admin_str = if is_admin { " (admin)" } else { "" };

        lines.push(format!(
            "- **{name}** (@{username}) [{state}] {email}{admin_str}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Get all members of a group (including inherited members).
pub async fn get_group_members(
    client: &GitLabClient,
    group_id: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/groups/{}/members/all",
        urlencoding::encode(group_id)
    );

    let members: Vec<Value> = client
        .get(&path, &[("per_page", per_page_str.as_str())])
        .await?;

    if members.is_empty() {
        return Ok("No members found.".to_string());
    }

    let mut lines = vec![format!("**Members: {}**\n", members.len())];
    for m in &members {
        let name = m["name"].as_str().unwrap_or("?");
        let username = m["username"].as_str().unwrap_or("?");
        let state = m["state"].as_str().unwrap_or("?");
        let access = match m["access_level"].as_u64().unwrap_or(0) {
            10 => "Guest",
            20 => "Reporter",
            30 => "Developer",
            40 => "Maintainer",
            50 => "Owner",
            _ => "?",
        };
        let state_str = if state != "active" {
            format!(" [{state}]")
        } else {
            String::new()
        };
        lines.push(format!("- **{name}** (@{username}) — {access}{state_str}"));
    }

    Ok(lines.join("\n"))
}

/// Get recent project events (activity feed).
pub async fn get_project_events(
    client: &GitLabClient,
    project_id: &str,
    action: &str,
    per_page: u32,
    summary_only: bool,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/projects/{}/events",
        urlencoding::encode(project_id)
    );

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
    ];
    if !action.is_empty() {
        params.push(("action", action));
    }

    let events: Vec<Value> = client.get(&path, &params).await?;

    if events.is_empty() {
        return Ok("No events found.".to_string());
    }

    if summary_only {
        use std::collections::BTreeSet;
        let total = events.len();
        let mut pushes = 0u32;
        let mut mrs = 0u32;
        let mut comments = 0u32;
        let mut authors: BTreeSet<String> = BTreeSet::new();
        for e in &events {
            let action_name = e["action_name"].as_str().unwrap_or("");
            let target_type = e["target_type"].as_str().unwrap_or("");
            if e["push_data"].is_object() || action_name.contains("pushed") {
                pushes += 1;
            } else if target_type == "MergeRequest" || action_name.contains("merge") {
                mrs += 1;
            } else if target_type == "Note" || target_type == "DiffNote" || target_type == "DiscussionNote" || action_name.contains("comment") {
                comments += 1;
            }
            if let Some(a) = e["author"]["username"].as_str() {
                authors.insert(a.to_string());
            }
        }
        return Ok(format!(
            "{project_id}: {total} events ({pushes} pushes, {mrs} MRs, {comments} comments) by {} authors",
            authors.len()
        ));
    }

    let mut lines = vec![format!("**Events: {}**\n", events.len())];
    for e in &events {
        let author = e["author"]["username"].as_str().unwrap_or("?");
        let action_name = e["action_name"].as_str().unwrap_or("?");
        let target_type = e["target_type"].as_str().unwrap_or("");
        let target_title = e["target_title"].as_str().unwrap_or("");
        let created = e["created_at"].as_str().unwrap_or("?");
        let date_short = if created.len() > 10 { &created[..10] } else { created };

        let push_data = if e["push_data"].is_object() {
            let ref_type = e["push_data"]["ref_type"].as_str().unwrap_or("branch");
            let ref_name = e["push_data"]["ref"].as_str().unwrap_or("?");
            let commit_count = e["push_data"]["commit_count"].as_u64().unwrap_or(0);
            format!(" {ref_type} `{ref_name}` ({commit_count} commits)")
        } else {
            String::new()
        };

        let target_str = if !target_title.is_empty() {
            format!(" {target_type}: {target_title}")
        } else if !target_type.is_empty() {
            format!(" {target_type}")
        } else {
            String::new()
        };

        lines.push(format!(
            "- [{date_short}] @{author} {action_name}{push_data}{target_str}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Check protection status for a single branch.
pub async fn check_branch_protection(
    client: &GitLabClient,
    project_id: &str,
    branch: &str,
) -> Result<String> {
    let path = format!(
        "/projects/{}/protected_branches/{}",
        urlencoding::encode(project_id),
        urlencoding::encode(branch)
    );

    let result: std::result::Result<Value, _> = client.get(&path, &[]).await;

    let pb = match result {
        Ok(v) => v,
        Err(crate::error::Error::GitLab { status, .. }) if status.as_u16() == 404 => {
            return Ok(format!("Branch '{branch}' is not protected."));
        }
        Err(e) => return Err(e),
    };

    let name = pb["name"].as_str().unwrap_or(branch);
    let allow_force_push = pb["allow_force_push"].as_bool().unwrap_or(false);
    let code_owner_required = pb["code_owner_approval_required"].as_bool().unwrap_or(false);

    fn level_label(level: u64) -> &'static str {
        match level {
            0 => "No access",
            30 => "Developer",
            40 => "Maintainer",
            60 => "Admin",
            _ => "?",
        }
    }

    fn format_access_levels(arr: Option<&Vec<Value>>) -> String {
        match arr {
            Some(items) if !items.is_empty() => items
                .iter()
                .map(|v| {
                    let level = v["access_level"].as_u64().unwrap_or(0);
                    let desc = v["access_level_description"].as_str().unwrap_or("");
                    if !desc.is_empty() && desc != level_label(level) {
                        format!("{} ({desc})", level_label(level))
                    } else {
                        level_label(level).to_string()
                    }
                })
                .collect::<Vec<_>>()
                .join(", "),
            _ => "–".to_string(),
        }
    }

    let push_access = format_access_levels(pb["push_access_levels"].as_array());
    let merge_access = format_access_levels(pb["merge_access_levels"].as_array());

    let lines = vec![
        format!("# Protected branch: {name}"),
        String::new(),
        format!("**Push access:** {push_access}"),
        format!("**Merge access:** {merge_access}"),
        format!("**Allow force push:** {allow_force_push}"),
        format!("**Code owner approval required:** {code_owner_required}"),
    ];

    Ok(lines.join("\n"))
}

/// Update branch protection settings (delete + recreate, since GitLab has no PATCH).
pub async fn update_branch_protection(
    client: &GitLabClient,
    project_id: &str,
    branch: &str,
    push_access_level: u32,
    merge_access_level: u32,
    allow_force_push: bool,
    code_owner_approval_required: bool,
) -> Result<String> {
    let enc_project = urlencoding::encode(project_id).to_string();
    let enc_branch = urlencoding::encode(branch).to_string();

    // Validate access levels
    let valid_levels = [0u32, 30, 40, 60];
    if !valid_levels.contains(&push_access_level) {
        return Ok(format!(
            "**Error:** Invalid push_access_level {push_access_level}. Use 0 (None), 30 (Developer), 40 (Maintainer), or 60 (Admin)."
        ));
    }
    if !valid_levels.contains(&merge_access_level) {
        return Ok(format!(
            "**Error:** Invalid merge_access_level {merge_access_level}. Use 0 (None), 30 (Developer), 40 (Maintainer), or 60 (Admin)."
        ));
    }

    // Delete existing protection (if any) — ignore 404
    let delete_path = format!("/projects/{enc_project}/protected_branches/{enc_branch}");
    if let Err(e) = client.delete(&delete_path).await {
        match e {
            crate::error::Error::GitLab { status, .. } if status.as_u16() == 404 => {
                // Not previously protected — that's fine
            }
            other => return Err(other),
        }
    }

    // Create new protection — encode params in URL since GitLab API takes query params here
    let create_path = format!(
        "/projects/{enc_project}/protected_branches?name={enc_branch}&push_access_level={push_access_level}&merge_access_level={merge_access_level}&allow_force_push={allow_force_push}&code_owner_approval_required={code_owner_approval_required}",
    );
    let body = serde_json::json!({});
    let _: Value = client.post(&create_path, &body).await?;

    fn level_label(level: u32) -> &'static str {
        match level {
            0 => "No access",
            30 => "Developer",
            40 => "Maintainer",
            60 => "Admin",
            _ => "?",
        }
    }

    Ok(format!(
        "Branch protection updated for `{branch}` on **{project_id}**:\n- Push: {} ({push_access_level})\n- Merge: {} ({merge_access_level})\n- Allow force push: {allow_force_push}\n- Code owner approval required: {code_owner_approval_required}",
        level_label(push_access_level),
        level_label(merge_access_level)
    ))
}

/// Find stale branches: merged but not deleted, or inactive for N days.
pub async fn get_stale_branches(
    client: &GitLabClient,
    project_id: &str,
    inactive_days: u32,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let cutoff = chrono::Utc::now() - chrono::Duration::days(inactive_days as i64);
    let cutoff_ts = cutoff.timestamp();

    // Fetch all branches (paginate)
    let mut all_branches: Vec<Value> = Vec::new();
    let mut page = 1u32;
    loop {
        let page_str = page.to_string();
        let branches: Vec<Value> = client.get(
            &format!("/projects/{encoded}/repository/branches"),
            &[("per_page", "100"), ("page", &page_str)],
        ).await?;
        if branches.is_empty() { break; }
        let count = branches.len();
        all_branches.extend(branches);
        if count < 100 { break; }
        page += 1;
        if page > 10 { break; } // cap at 1000 branches
    }

    if all_branches.is_empty() {
        return Ok(format!("No branches found for {project_id}."));
    }

    let total = all_branches.len();
    let mut stale: Vec<(String, String, String, bool)> = Vec::new(); // (name, last_commit_date, author, is_merged)

    for b in &all_branches {
        let name = b["name"].as_str().unwrap_or("?");
        let is_default = b["default"].as_bool().unwrap_or(false);
        let is_protected = b["protected"].as_bool().unwrap_or(false);
        let merged = b["merged"].as_bool().unwrap_or(false);

        // Skip default and protected branches
        if is_default || is_protected { continue; }

        let committed_date = b["commit"]["committed_date"]
            .as_str()
            .or(b["commit"]["created_at"].as_str())
            .unwrap_or("");

        let is_old = chrono::DateTime::parse_from_rfc3339(committed_date)
            .ok()
            .map(|dt| dt.timestamp() < cutoff_ts)
            .unwrap_or(false);

        if merged || is_old {
            let date_short = if committed_date.len() > 10 { &committed_date[..10] } else { committed_date };
            let author = b["commit"]["author_name"].as_str().unwrap_or("?");
            stale.push((name.to_string(), date_short.to_string(), author.to_string(), merged));
        }
    }

    if stale.is_empty() {
        return Ok(format!("No stale branches in {project_id} ({total} branches, cutoff: {inactive_days} days)."));
    }

    let merged_count = stale.iter().filter(|s| s.3).count();
    let inactive_count = stale.iter().filter(|s| !s.3).count();

    let mut lines = vec![
        format!("**Stale Branches: {project_id}** ({} stale / {total} total)\n", stale.len()),
        format!("| Type | Count |"),
        format!("|------|-------|"),
        format!("| Merged (safe to delete) | {merged_count} |"),
        format!("| Inactive (>{inactive_days}d, not merged) | {inactive_count} |"),
    ];

    // List merged first (safe to delete)
    if merged_count > 0 {
        lines.push(String::new());
        lines.push("**Merged (safe to delete):**".to_string());
        for (name, date, author, merged) in &stale {
            if *merged {
                lines.push(format!("- `{name}` last: {date} by {author}"));
            }
        }
    }

    if inactive_count > 0 {
        lines.push(String::new());
        lines.push(format!("**Inactive (>{inactive_days}d, NOT merged):**"));
        for (name, date, author, merged) in &stale {
            if !*merged {
                lines.push(format!("- `{name}` last: {date} by {author}"));
            }
        }
    }

    Ok(lines.join("\n"))
}
