//! GitLab CI/CD pipeline tools.

use crate::client::GitLabClient;
use crate::error::Result;
use serde_json::Value;

/// List pipelines for a project.
pub async fn list_pipelines(
    client: &GitLabClient,
    project_id: &str,
    status: &str,
    ref_name: &str,
    per_page: u32,
) -> Result<String> {
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
        ?;

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
        let _web_url = p["web_url"].as_str().unwrap_or("");

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
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let path = format!("/projects/{encoded}/pipelines/{pipeline_id}");
    let p: Value = client.get(&path, &[]).await?;

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
        ?;

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

/// Get CI job log (trace).
pub async fn get_job_log(
    client: &GitLabClient,
    project_id: &str,
    job_id: u64,
    tail: usize,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    // Get job metadata first
    let job: serde_json::Value = client
        .get(&format!("/projects/{encoded}/jobs/{job_id}"), &[])
        .await
        ?;

    let name = job["name"].as_str().unwrap_or("?");
    let status = job["status"].as_str().unwrap_or("?");
    let stage = job["stage"].as_str().unwrap_or("?");
    let duration = job["duration"].as_f64().unwrap_or(0.0);

    // Get log text (plain text, not JSON)
    let _log_url = format!("{}/api/v4/projects/{encoded}/jobs/{job_id}/trace", "");
    // Use get_json which returns Value, but trace returns plain text
    // We need raw text — use the client's get method differently
    let log_text: String = client
        .get(&format!("/projects/{encoded}/jobs/{job_id}/trace"), &[])
        .await
        ?;

    // Tail: take last N lines
    let lines: Vec<&str> = log_text.lines().collect();
    let start = if lines.len() > tail { lines.len() - tail } else { 0 };
    let tail_lines = &lines[start..];

    let mut parts = vec![
        format!("## Job #{job_id}: {name}"),
        format!("**Stage:** {stage} | **Status:** {status} | **Duration:** {duration:.0}s"),
    ];

    if start > 0 {
        parts.push(format!("*...{start} lines skipped, showing last {tail}*"));
    }
    parts.push(String::new());
    parts.push("```".to_string());
    parts.push(tail_lines.join("\n"));
    parts.push("```".to_string());

    Ok(parts.join("\n"))
}

/// List pipelines for a merge request.
pub async fn get_mr_pipelines(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
) -> Result<String> {
    let path = format!(
        "/projects/{}/merge_requests/{}/pipelines",
        urlencoding::encode(project_id),
        mr_iid
    );

    let pipelines: Vec<Value> = client
        .get(&path, &[])
        .await?;

    if pipelines.is_empty() {
        return Ok(format!("No pipelines found for MR !{mr_iid}."));
    }

    let mut lines = vec![format!("**MR !{mr_iid} — {} pipelines**\n", pipelines.len())];

    for p in &pipelines {
        let id = p["id"].as_u64().unwrap_or(0);
        let status = p["status"].as_str().unwrap_or("?");
        let ref_name = p["ref"].as_str().unwrap_or("?");
        let sha = p["sha"].as_str().unwrap_or("?");
        let sha_short = if sha.len() > 8 { &sha[..8] } else { sha };
        let created = p["created_at"].as_str().unwrap_or("?");

        let status_icon = match status {
            "success" => "✅",
            "failed" => "❌",
            "running" => "🔄",
            "pending" => "⏳",
            "canceled" => "⛔",
            _ => "❓",
        };

        lines.push(format!(
            "- {status_icon} **#{id}** [{status}] ref: {ref_name} sha: {sha_short} — {created}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Retry a pipeline.
pub async fn retry_pipeline(
    client: &GitLabClient,
    project_id: &str,
    pipeline_id: u64,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let p: serde_json::Value = client
        .post(
            &format!("/projects/{encoded}/pipelines/{pipeline_id}/retry"),
            &serde_json::json!({}),
        )
        .await
        ?;

    let status = p["status"].as_str().unwrap_or("?");
    let web_url = p["web_url"].as_str().unwrap_or("");
    Ok(format!(
        "Pipeline #{pipeline_id} retried. **Status:** {status}\n**URL:** {web_url}"
    ))
}

/// Cancel a pipeline.
pub async fn cancel_pipeline(
    client: &GitLabClient,
    project_id: &str,
    pipeline_id: u64,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let p: serde_json::Value = client
        .post(
            &format!("/projects/{encoded}/pipelines/{pipeline_id}/cancel"),
            &serde_json::json!({}),
        )
        .await
        ?;

    let status = p["status"].as_str().unwrap_or("?");
    Ok(format!("Pipeline #{pipeline_id} canceled. **Status:** {status}"))
}
