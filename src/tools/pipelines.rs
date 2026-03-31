//! GitLab CI/CD pipeline tools.

use crate::client::GitLabClient;
use serde_json::Value;

/// List pipelines for a project.
pub async fn list_pipelines(
    client: &GitLabClient,
    project_id: &str,
    status: &str,
    ref_name: &str,
    per_page: u32,
) -> Result<String, String> {
    let per_page_str = per_page.to_string();
    let path = format!(
        "/projects/{}/pipelines",
        urlencoding::encode(project_id)
    );

    let mut params: Vec<(&str, &str)> = vec![
        ("per_page", &per_page_str),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ];
    if !status.is_empty() {
        params.push(("status", status));
    }
    if !ref_name.is_empty() {
        params.push(("ref", ref_name));
    }

    let pipelines: Vec<Value> = client
        .get(&path, &params)
        .await
        .map_err(|e| e.to_string())?;

    if pipelines.is_empty() {
        return Ok("No pipelines found.".to_string());
    }

    let mut lines = vec![format!("**Found: {} pipelines**\n", pipelines.len())];

    for p in &pipelines {
        let id = p["id"].as_u64().unwrap_or(0);
        let status = p["status"].as_str().unwrap_or("?");
        let ref_name = p["ref"].as_str().unwrap_or("?");
        let source = p["source"].as_str().unwrap_or("?");
        let created = p["created_at"].as_str().unwrap_or("?");
        let web_url = p["web_url"].as_str().unwrap_or("");

        let status_icon = match status {
            "success" => "✅",
            "failed" => "❌",
            "running" => "🔄",
            "pending" => "⏳",
            "canceled" => "⛔",
            _ => "❓",
        };

        lines.push(format!(
            "- {status_icon} **#{id}** [{status}] ref: {ref_name} ({source}) — {created}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Get pipeline details with jobs.
pub async fn get_pipeline(
    client: &GitLabClient,
    project_id: &str,
    pipeline_id: u64,
) -> Result<String, String> {
    let encoded = urlencoding::encode(project_id);
    let path = format!("/projects/{encoded}/pipelines/{pipeline_id}");
    let p: Value = client.get(&path, &[]).await.map_err(|e| e.to_string())?;

    let status = p["status"].as_str().unwrap_or("?");
    let ref_name = p["ref"].as_str().unwrap_or("?");
    let source = p["source"].as_str().unwrap_or("?");
    let created = p["created_at"].as_str().unwrap_or("?");
    let finished = p["finished_at"].as_str().unwrap_or("running");
    let duration = p["duration"].as_f64().unwrap_or(0.0);
    let web_url = p["web_url"].as_str().unwrap_or("");
    let user = p["user"]["username"].as_str().unwrap_or("?");

    let duration_str = if duration >= 60.0 {
        format!("{}m {}s", duration as u64 / 60, duration as u64 % 60)
    } else {
        format!("{:.0}s", duration)
    };

    let mut parts = vec![
        format!("# Pipeline #{pipeline_id}"),
        String::new(),
        format!("**Status:** {status}"),
        format!("**Ref:** {ref_name}"),
        format!("**Source:** {source}"),
        format!("**Triggered by:** @{user}"),
        format!("**Duration:** {duration_str}"),
        format!("**Created:** {created}"),
        format!("**Finished:** {finished}"),
        format!("**URL:** {web_url}"),
    ];

    // Fetch jobs
    let jobs_path = format!("/projects/{encoded}/pipelines/{pipeline_id}/jobs");
    let jobs: Vec<Value> = client
        .get(&jobs_path, &[("per_page", "100")])
        .await
        .map_err(|e| e.to_string())?;

    if !jobs.is_empty() {
        // Group by stage
        let mut stages: std::collections::BTreeMap<String, Vec<&Value>> =
            std::collections::BTreeMap::new();
        for job in &jobs {
            let stage = job["stage"].as_str().unwrap_or("unknown").to_string();
            stages.entry(stage).or_default().push(job);
        }

        parts.push(String::new());
        parts.push(format!("## Jobs ({})", jobs.len()));

        for (stage, stage_jobs) in &stages {
            parts.push(format!("\n### {stage}"));
            for job in stage_jobs {
                let name = job["name"].as_str().unwrap_or("?");
                let status = job["status"].as_str().unwrap_or("?");
                let duration = job["duration"].as_f64().unwrap_or(0.0);

                let icon = match status {
                    "success" => "✅",
                    "failed" => "❌",
                    "running" => "🔄",
                    "pending" => "⏳",
                    "canceled" => "⛔",
                    "skipped" => "⏭️",
                    "manual" => "👆",
                    _ => "❓",
                };

                parts.push(format!("- {icon} **{name}** [{status}] {duration:.0}s"));
            }
        }
    }

    Ok(parts.join("\n"))
}
