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
                let job_id = job["id"].as_u64().unwrap_or(0);
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

                // Include the numeric job id so get_job_log can be called directly.
                let mut line = format!("- {icon} **{name}** [{status}] (job {job_id}) {duration:.0}s");
                if status == "failed" {
                    if let Some(reason) = job["failure_reason"].as_str() {
                        if !reason.is_empty() {
                            line.push_str(&format!(" — {reason}"));
                        }
                    }
                }
                parts.push(line);
            }
        }
    }

    Ok(parts.join("\n"))
}

/// Remove ANSI escape sequences and carriage returns that GitLab embeds in CI
/// job traces, so the log is readable (no `\x1b[0K`/`\x1b[32;1m` noise) and
/// fewer tokens are spent on control codes.
fn strip_ansi(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => {
                if chars.peek() == Some(&'[') {
                    chars.next(); // consume '['
                    // CSI: param/intermediate bytes, then a final byte in @..~
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if ('\u{40}'..='\u{7e}').contains(&nc) {
                            break;
                        }
                    }
                } else {
                    chars.next(); // drop the byte following a lone ESC
                }
            }
            '\r' => {} // drop carriage returns (progress-line overwrites)
            _ => out.push(c),
        }
    }
    out
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
    let failure_reason = job["failure_reason"].as_str().unwrap_or("");
    let meta = if status == "failed" && !failure_reason.is_empty() {
        format!("**Stage:** {stage} | **Status:** {status} ({failure_reason}) | **Duration:** {duration:.0}s")
    } else {
        format!("**Stage:** {stage} | **Status:** {status} | **Duration:** {duration:.0}s")
    };

    // The trace endpoint returns plain text, not JSON — use get_text (get::<String>
    // would try to JSON-deserialize the trace and fail with a parse error).
    // Strip the ANSI colour/erase codes GitLab embeds before processing.
    let log_text = strip_ansi(
        &client
            .get_text(&format!("/projects/{encoded}/jobs/{job_id}/trace"), &[])
            .await?,
    );

    if log_text.trim().is_empty() {
        return Ok(format!(
            "## Job #{job_id}: {name}\n{meta}\n\n*(log is empty — the job may be pending/created, or its trace was erased)*"
        ));
    }

    // Tail: take last N lines
    let lines: Vec<&str> = log_text.lines().collect();
    let start = if lines.len() > tail { lines.len() - tail } else { 0 };
    let tail_lines = &lines[start..];

    let mut parts = vec![
        format!("## Job #{job_id}: {name}"),
        meta,
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

/// Create a new CI/CD variable for a project. NEVER echoes the value back.
#[allow(clippy::too_many_arguments)]
pub async fn set_ci_variable(
    client: &GitLabClient,
    project_id: &str,
    key: &str,
    value: &str,
    protected: bool,
    masked: bool,
    environment_scope: &str,
    variable_type: &str,
) -> Result<String> {
    let valid_types = ["env_var", "file"];
    if !valid_types.contains(&variable_type) {
        return Ok(format!(
            "**Error:** Invalid variable_type '{variable_type}'. Use 'env_var' or 'file'."
        ));
    }

    let path = format!(
        "/projects/{}/variables",
        urlencoding::encode(project_id)
    );

    let body = serde_json::json!({
        "key": key,
        "value": value,
        "protected": protected,
        "masked": masked,
        "environment_scope": environment_scope,
        "variable_type": variable_type,
    });

    let _: Value = client.post(&path, &body).await?;

    Ok(format!(
        "CI variable `{key}` set on **{project_id}** (masked: {masked}, protected: {protected}, scope: {environment_scope}, type: {variable_type}). Value not shown."
    ))
}

/// Update an existing CI/CD variable. NEVER echoes the value back.
#[allow(clippy::too_many_arguments)]
pub async fn update_ci_variable(
    client: &GitLabClient,
    project_id: &str,
    key: &str,
    value: &str,
    protected: Option<bool>,
    masked: Option<bool>,
    environment_scope: Option<&str>,
    variable_type: Option<&str>,
) -> Result<String> {
    if let Some(vt) = variable_type {
        let valid_types = ["env_var", "file"];
        if !valid_types.contains(&vt) {
            return Ok(format!(
                "**Error:** Invalid variable_type '{vt}'. Use 'env_var' or 'file'."
            ));
        }
    }

    let path = format!(
        "/projects/{}/variables/{}",
        urlencoding::encode(project_id),
        urlencoding::encode(key)
    );

    let mut body = serde_json::json!({ "value": value });
    if let Some(p) = protected {
        body["protected"] = serde_json::json!(p);
    }
    if let Some(m) = masked {
        body["masked"] = serde_json::json!(m);
    }
    if let Some(env) = environment_scope {
        body["environment_scope"] = serde_json::json!(env);
    }
    if let Some(vt) = variable_type {
        body["variable_type"] = serde_json::json!(vt);
    }

    let _: Value = client.put(&path, &body).await?;

    Ok(format!(
        "CI variable `{key}` updated on **{project_id}**. Value not shown."
    ))
}

/// Delete a CI/CD variable.
pub async fn delete_ci_variable(
    client: &GitLabClient,
    project_id: &str,
    key: &str,
) -> Result<String> {
    let path = format!(
        "/projects/{}/variables/{}",
        urlencoding::encode(project_id),
        urlencoding::encode(key)
    );

    client.delete(&path).await?;

    Ok(format!(
        "CI variable `{key}` deleted from **{project_id}**."
    ))
}

/// Get CI/CD variables for a project (keys and metadata only, never values).
pub async fn get_ci_variables(
    client: &GitLabClient,
    project_id: &str,
) -> Result<String> {
    let path = format!(
        "/projects/{}/variables",
        urlencoding::encode(project_id)
    );

    let variables: Vec<Value> = client
        .get(&path, &[("per_page", "100")])
        .await?;

    if variables.is_empty() {
        return Ok("No CI/CD variables found.".to_string());
    }

    let mut lines = vec![format!("**CI/CD Variables: {}**\n", variables.len())];
    lines.push("| Key | Masked | Protected | Environment |".to_string());
    lines.push("|-----|--------|-----------|-------------|".to_string());

    for v in &variables {
        let key = v["key"].as_str().unwrap_or("?");
        let masked = if v["masked"].as_bool().unwrap_or(false) { "yes" } else { "no" };
        let protected = if v["protected"].as_bool().unwrap_or(false) { "yes" } else { "no" };
        let env_scope = v["environment_scope"].as_str().unwrap_or("*");

        lines.push(format!("| {key} | {masked} | {protected} | {env_scope} |"));
    }

    Ok(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::strip_ansi;

    #[test]
    fn strips_csi_color_and_erase_codes() {
        // Real GitLab-trace shape: color codes + erase-line + carriage return.
        let raw = "\u{1b}[0K\u{1b}[32;1m$ docker login\u{1b}[0;m\r\nError: unauthorized";
        assert_eq!(strip_ansi(raw), "$ docker login\nError: unauthorized");
    }

    #[test]
    fn leaves_plain_text_untouched() {
        assert_eq!(strip_ansi("plain log line"), "plain log line");
        // A lone ESC drops only the following byte, not real content.
        assert_eq!(strip_ansi("a\u{1b}Xb"), "ab");
    }
}
