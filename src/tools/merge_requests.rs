//! GitLab merge request tools.

use crate::client::GitLabClient;
use serde_json::Value;

/// List merge requests.
pub async fn list_merge_requests(
    client: &GitLabClient,
    project_id: &str,
    state: &str,
    author: &str,
    scope: &str,
    per_page: u32,
) -> Result<String, String> {
    let per_page_str = per_page.to_string();
    let path = if project_id.is_empty() {
        "/merge_requests".to_string()
    } else {
        format!(
            "/projects/{}/merge_requests",
            urlencoding::encode(project_id)
        )
    };

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ];
    if !state.is_empty() {
        params.push(("state", state));
    }
    if !author.is_empty() {
        params.push(("author_username", author));
    }
    if !scope.is_empty() {
        params.push(("scope", scope));
    }

    let mrs: Vec<Value> = client.get(&path, &params).await.map_err(|e| e.to_string())?;

    if mrs.is_empty() {
        return Ok("No merge requests found.".to_string());
    }

    let mut lines = vec![format!("**Found: {} merge requests**\n", mrs.len())];

    for mr in &mrs {
        let iid = mr["iid"].as_u64().unwrap_or(0);
        let title = mr["title"].as_str().unwrap_or("?");
        let state = mr["state"].as_str().unwrap_or("?");
        let author = mr["author"]["username"].as_str().unwrap_or("?");
        let source = mr["source_branch"].as_str().unwrap_or("?");
        let target = mr["target_branch"].as_str().unwrap_or("?");
        let fallback = format!("!{iid}");
        let project = mr["references"]["full"]
            .as_str()
            .unwrap_or(&fallback);

        let draft = if mr["draft"].as_bool().unwrap_or(false) {
            " [DRAFT]"
        } else {
            ""
        };

        let created = mr["created_at"].as_str().unwrap_or("?");
        let created_short = if created.len() > 10 { &created[..10] } else { created };

        lines.push(format!(
            "- **{project}** [{state}]{draft} {title} ({source} → {target}) by @{author} ({created_short})"
        ));
    }

    Ok(lines.join("\n"))
}

/// Get a single merge request with details.
pub async fn get_merge_request(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
    include_notes: bool,
) -> Result<String, String> {
    let path = format!(
        "/projects/{}/merge_requests/{}",
        urlencoding::encode(project_id),
        mr_iid
    );
    let mr: Value = client.get(&path, &[]).await.map_err(|e| e.to_string())?;

    let title = mr["title"].as_str().unwrap_or("?");
    let state = mr["state"].as_str().unwrap_or("?");
    let desc = mr["description"].as_str().unwrap_or("");
    let author = mr["author"]["username"].as_str().unwrap_or("?");
    let source = mr["source_branch"].as_str().unwrap_or("?");
    let target = mr["target_branch"].as_str().unwrap_or("?");
    let web_url = mr["web_url"].as_str().unwrap_or("");
    let created = mr["created_at"].as_str().unwrap_or("?");
    let updated = mr["updated_at"].as_str().unwrap_or("?");
    let pipeline_status = mr["pipeline"]["status"].as_str().unwrap_or("none");
    let merge_status = mr["detailed_merge_status"].as_str().unwrap_or("?");
    let changes = mr["changes_count"].as_str().unwrap_or("?");

    let reviewers: Vec<&str> = mr["reviewers"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v["username"].as_str()).collect())
        .unwrap_or_default();

    let labels: Vec<&str> = mr["labels"]
        .as_array()
        .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
        .unwrap_or_default();

    let mut parts = vec![
        format!("# !{mr_iid}: {title}"),
        String::new(),
        format!("**State:** {state}"),
        format!("**Branch:** {source} → {target}"),
        format!("**Author:** @{author}"),
        format!("**Pipeline:** {pipeline_status}"),
        format!("**Merge status:** {merge_status}"),
        format!("**Changes:** {changes} files"),
        format!("**Created:** {created}"),
        format!("**Updated:** {updated}"),
        format!("**URL:** {web_url}"),
    ];

    if !reviewers.is_empty() {
        parts.push(format!(
            "**Reviewers:** {}",
            reviewers.iter().map(|r| format!("@{r}")).collect::<Vec<_>>().join(", ")
        ));
    }

    if !labels.is_empty() {
        parts.push(format!("**Labels:** {}", labels.join(", ")));
    }

    if !desc.is_empty() {
        parts.push(String::new());
        parts.push(format!("## Description\n{desc}"));
    }

    if include_notes {
        let notes_path = format!(
            "/projects/{}/merge_requests/{}/notes",
            urlencoding::encode(project_id),
            mr_iid
        );
        let notes: Vec<Value> = client
            .get(&notes_path, &[("per_page", "50"), ("sort", "asc")])
            .await
            .map_err(|e| e.to_string())?;

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
