//! GitLab project tools.

use crate::client::GitLabClient;
use serde_json::Value;

/// List projects accessible to the authenticated user.
pub async fn list_projects(
    client: &GitLabClient,
    search: &str,
    per_page: u32,
) -> Result<String, String> {
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
        .map_err(|e| e.to_string())?;

    if projects.is_empty() {
        return Ok("No projects found.".to_string());
    }

    let mut lines = vec![format!("**Found: {} projects**\n", projects.len())];
    for p in &projects {
        let name = p["path_with_namespace"].as_str().unwrap_or("?");
        let id = p["id"].as_u64().unwrap_or(0);
        let desc = p["description"].as_str().unwrap_or("");
        let desc_short = if desc.len() > 80 {
            format!("{}...", &desc[..80])
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
) -> Result<String, String> {
    let path = format!("/projects/{}", urlencoding::encode(project_id));
    let p: Value = client
        .get(&path, &[])
        .await
        .map_err(|e| e.to_string())?;

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
) -> Result<String, String> {
    let path = format!("/projects/{}/members/all", urlencoding::encode(project_id));
    let members: Vec<Value> = client
        .get(&path, &[("per_page", "100")])
        .await
        .map_err(|e| e.to_string())?;

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
