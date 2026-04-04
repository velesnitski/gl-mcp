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
