//! GitLab issue tools.

use crate::client::GitLabClient;
use crate::error::Result;
use serde_json::Value;

/// Search issues across projects, within a project, or within a group.
pub async fn search_issues(
    client: &GitLabClient,
    project_id: &str,
    group_id: &str,
    search: &str,
    state: &str,
    labels: &str,
    assignee: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = if !group_id.is_empty() {
        format!("/groups/{}/issues", urlencoding::encode(group_id))
    } else if project_id.is_empty() {
        "/issues".to_string()
    } else {
        format!("/projects/{}/issues", urlencoding::encode(project_id))
    };

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ];
    if !search.is_empty() {
        params.push(("search", search));
    }
    if !state.is_empty() {
        params.push(("state", state));
    }
    if !labels.is_empty() {
        params.push(("labels", labels));
    }
    if !assignee.is_empty() {
        params.push(("assignee_username", assignee));
    }

    let issues: Vec<Value> = client
        .get(&path, &params)
        .await
        ?;

    if issues.is_empty() {
        return Ok("No issues found.".to_string());
    }

    let count = issues.len();
    let mut lines = vec![format!("**Found: {count} issues**")];
    if count >= per_page as usize {
        lines[0].push_str(&format!(" (showing first {per_page}, more may exist)"));
    }
    lines.push(String::new());

    for issue in &issues {
        let iid = issue["iid"].as_u64().unwrap_or(0);
        let title = issue["title"].as_str().unwrap_or("?");
        let state = issue["state"].as_str().unwrap_or("?");
        let project = issue["references"]["full"].as_str().unwrap_or("?");
        let assignee = issue["assignee"]["username"]
            .as_str()
            .unwrap_or("Unassigned");
        let labels: Vec<&str> = issue["labels"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
            .unwrap_or_default();

        let label_str = if labels.is_empty() {
            String::new()
        } else {
            format!(" [{}]", labels.join(", "))
        };

        lines.push(format!(
            "- **{project}#{iid}** [{state}] {title}{label_str} → @{assignee}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Get a single issue with full details.
pub async fn get_issue(
    client: &GitLabClient,
    project_id: &str,
    issue_iid: u64,
    include_notes: bool,
) -> Result<String> {
    let path = format!(
        "/projects/{}/issues/{}",
        urlencoding::encode(project_id),
        issue_iid
    );
    let issue: Value = client.get(&path, &[]).await?;

    let title = issue["title"].as_str().unwrap_or("?");
    let state = issue["state"].as_str().unwrap_or("?");
    let desc = issue["description"].as_str().unwrap_or("");
    let author = issue["author"]["username"].as_str().unwrap_or("?");
    let assignee = issue["assignee"]["username"]
        .as_str()
        .unwrap_or("Unassigned");
    let created = issue["created_at"].as_str().unwrap_or("?");
    let updated = issue["updated_at"].as_str().unwrap_or("?");
    let web_url = issue["web_url"].as_str().unwrap_or("");
    let milestone = issue["milestone"]["title"].as_str().unwrap_or("None");

    let labels: Vec<&str> = issue["labels"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut parts = vec![
        format!("# #{issue_iid}: {title}"),
        String::new(),
        format!("**State:** {state}"),
        format!("**Author:** @{author}"),
        format!("**Assignee:** @{assignee}"),
        format!("**Milestone:** {milestone}"),
        format!("**Created:** {created}"),
        format!("**Updated:** {updated}"),
        format!("**URL:** {web_url}"),
    ];

    if !labels.is_empty() {
        parts.push(format!("**Labels:** {}", labels.join(", ")));
    }

    if !desc.is_empty() {
        parts.push(String::new());
        parts.push(format!("## Description\n{desc}"));
    }

    // Notes (comments)
    if include_notes {
        let notes_path = format!(
            "/projects/{}/issues/{}/notes",
            urlencoding::encode(project_id),
            issue_iid
        );
        let notes: Vec<Value> = client
            .get(&notes_path, &[("per_page", "50"), ("sort", "asc")])
            .await
            ?;

        let user_notes: Vec<&Value> = notes
            .iter()
            .filter(|n| !n["system"].as_bool().unwrap_or(true))
            .collect();

        if !user_notes.is_empty() {
            parts.push(String::new());
            parts.push(format!("## Comments ({})", user_notes.len()));
            for n in &user_notes {
                let author = n["author"]["username"].as_str().unwrap_or("?");
                let body = n["body"].as_str().unwrap_or("");
                parts.push(format!("**@{author}:** {body}"));
                parts.push(String::new());
            }
        }
    }

    Ok(parts.join("\n"))
}

/// Create an issue.
pub async fn create_issue(
    client: &GitLabClient,
    project_id: &str,
    title: &str,
    description: &str,
    labels: &str,
    assignee: &str,
    milestone_id: Option<u64>,
) -> Result<String> {
    let path = format!("/projects/{}/issues", urlencoding::encode(project_id));

    let mut body = serde_json::json!({ "title": title });
    if !description.is_empty() {
        body["description"] = Value::String(description.to_string());
    }
    if !labels.is_empty() {
        body["labels"] = Value::String(labels.to_string());
    }
    if !assignee.is_empty() {
        // Look up user ID
        let users: Vec<Value> = client
            .get("/users", &[("username", assignee)])
            .await
            ?;
        if let Some(user) = users.first() {
            if let Some(id) = user["id"].as_u64() {
                body["assignee_ids"] = serde_json::json!([id]);
            }
        }
    }
    if let Some(mid) = milestone_id {
        body["milestone_id"] = Value::Number(mid.into());
    }

    let issue: Value = client.post(&path, &body).await?;
    let iid = issue["iid"].as_u64().unwrap_or(0);
    let web_url = issue["web_url"].as_str().unwrap_or("");

    Ok(format!("Created: **#{iid}** — {title}\n**URL:** {web_url}"))
}

/// Update an issue.
pub async fn update_issue(
    client: &GitLabClient,
    project_id: &str,
    issue_iid: u64,
    title: Option<&str>,
    description: Option<&str>,
    state_event: Option<&str>,
    labels: Option<&str>,
    assignee: Option<&str>,
) -> Result<String> {
    let path = format!(
        "/projects/{}/issues/{}",
        urlencoding::encode(project_id),
        issue_iid
    );

    let mut body = serde_json::json!({});
    if let Some(t) = title {
        body["title"] = Value::String(t.to_string());
    }
    if let Some(d) = description {
        body["description"] = Value::String(d.to_string());
    }
    if let Some(s) = state_event {
        body["state_event"] = Value::String(s.to_string());
    }
    if let Some(l) = labels {
        body["labels"] = Value::String(l.to_string());
    }
    if let Some(a) = assignee {
        if a.is_empty() {
            body["assignee_ids"] = serde_json::json!([]);
        } else {
            let users: Vec<Value> = client
                .get("/users", &[("username", a)])
                .await
                ?;
            if let Some(user) = users.first() {
                if let Some(id) = user["id"].as_u64() {
                    body["assignee_ids"] = serde_json::json!([id]);
                }
            }
        }
    }

    let issue: Value = client
        .put(&path, &body)
        .await
        ?;

    let state = issue["state"].as_str().unwrap_or("?");
    let web_url = issue["web_url"].as_str().unwrap_or("");
    Ok(format!(
        "Updated: **#{issue_iid}** [{state}]\n**URL:** {web_url}"
    ))
}

/// Add a note (comment) to an issue or merge request.
pub async fn add_note(
    client: &GitLabClient,
    project_id: &str,
    iid: u64,
    note_type: &str,
    body: &str,
) -> Result<String> {
    let kind = if note_type == "mr" {
        "merge_requests"
    } else {
        "issues"
    };
    let path = format!(
        "/projects/{}/{}/{}/notes",
        urlencoding::encode(project_id),
        kind,
        iid
    );

    let note: Value = client
        .post(&path, &serde_json::json!({ "body": body }))
        .await
        ?;

    let note_id = note["id"].as_u64().unwrap_or(0);
    let symbol = if note_type == "mr" { "!" } else { "#" };
    Ok(format!(
        "Comment added to **{symbol}{iid}** (note id: {note_id})"
    ))
}

/// List project labels.
pub async fn list_labels(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let path = format!(
        "/projects/{}/labels",
        urlencoding::encode(project_id)
    );

    let labels: Vec<Value> = client
        .get(&path, &[("per_page", "100")])
        .await?;

    if labels.is_empty() {
        return Ok("No labels found.".to_string());
    }

    let mut lines = vec![format!("**Labels: {}**\n", labels.len())];
    for l in &labels {
        let name = l["name"].as_str().unwrap_or("?");
        let color = l["color"].as_str().unwrap_or("?");
        let desc = l["description"].as_str().unwrap_or("");
        let open_issues = l["open_issues_count"].as_u64().unwrap_or(0);

        let desc_part = if desc.is_empty() {
            String::new()
        } else {
            format!(" — {desc}")
        };

        lines.push(format!(
            "- **{name}** ({color}) issues: {open_issues}{desc_part}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Create a project label.
pub async fn create_label(
    client: &GitLabClient,
    project_id: &str,
    name: &str,
    color: &str,
    description: &str,
) -> Result<String> {
    let path = format!(
        "/projects/{}/labels",
        urlencoding::encode(project_id)
    );

    let mut body = serde_json::json!({
        "name": name,
        "color": color,
    });
    if !description.is_empty() {
        body["description"] = Value::String(description.to_string());
    }

    let label: Value = client.post(&path, &body).await?;

    let created_name = label["name"].as_str().unwrap_or("?");
    let created_color = label["color"].as_str().unwrap_or("?");

    Ok(format!(
        "Created label: **{created_name}** ({created_color})"
    ))
}

/// Get project milestones.
pub async fn get_milestones(
    client: &GitLabClient,
    project_id: &str,
    state: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/projects/{}/milestones",
        urlencoding::encode(project_id)
    );

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
    ];
    if !state.is_empty() && state != "all" {
        params.push(("state", state));
    }

    let milestones: Vec<Value> = client
        .get(&path, &params)
        .await?;

    if milestones.is_empty() {
        return Ok("No milestones found.".to_string());
    }

    let mut lines = vec![format!("**Milestones: {}**\n", milestones.len())];
    for m in &milestones {
        let title = m["title"].as_str().unwrap_or("?");
        let state = m["state"].as_str().unwrap_or("?");
        let due_date = m["due_date"].as_str().unwrap_or("–");

        let mut extra = Vec::new();
        if due_date != "–" {
            extra.push(format!("due: {due_date}"));
        }

        let desc = m["description"].as_str().unwrap_or("");
        let desc_short = if desc.len() > 80 {
            let truncated: String = desc.chars().take(80).collect();
            format!("{truncated}...")
        } else {
            desc.to_string()
        };

        let desc_part = if desc_short.is_empty() {
            String::new()
        } else {
            format!(" — {desc_short}")
        };

        let extra_str = if extra.is_empty() {
            String::new()
        } else {
            format!(" ({})", extra.join(", "))
        };

        lines.push(format!(
            "- **{title}** [{state}]{extra_str}{desc_part}"
        ));
    }

    Ok(lines.join("\n"))
}
