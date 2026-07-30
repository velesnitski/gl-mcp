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
/// Repos/pipelines inspected concurrently by `analyze_pipeline_failures`.
const ANALYZE_CONCURRENCY: usize = 8;

/// UUIDs, long numbers and hashes are collapsed so the *same* failure clusters
/// together instead of splitting per run.
static VOLATILE_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        // Note: the numeric arm is deliberately NOT \b-anchored — durations and
        // sizes arrive glued to units ("1234ms", "512MB"), and a word-boundary
        // form silently fails to mask them, so the same fault would not cluster.
        r"(?i)[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}|\b[0-9a-f]{7,}\b|\d{2,}",
    )
    .unwrap()
});

/// Sources meaning "something automated or an operator ran this" — the product
/// triggering infra work, a schedule, an API call, a Run-pipeline click.
///
/// The rest (`push`, `merge_request_event`) is development CI. Keeping them apart
/// is the whole point: a template repo whose MR pipelines are all red looks like a
/// total outage while production provisioning is perfectly healthy.
fn is_automated_source(source: &str) -> bool {
    matches!(
        source,
        "trigger" | "api" | "schedule" | "pipeline" | "web" | "external"
    )
}

/// Collapse a raw error line into a stable, clusterable signature.
///
/// Masking volatile IDs is what makes repeats cluster, but on a message that is
/// mostly numbers it eats the whole thing and yields a useless `error: …`. When
/// the masked form retains almost no words, keep the original text instead —
/// a slightly over-specific cluster beats an empty one.
fn normalize_signature(s: &str) -> String {
    let collapsed = s.split_whitespace().collect::<Vec<_>>().join(" ");
    let masked = VOLATILE_RE.replace_all(&collapsed, "…");
    let words_left = masked.chars().filter(|c| c.is_alphabetic()).count();
    let chosen: &str = if words_left < 8 { &collapsed } else { &masked };
    let mut out: String = chosen.chars().take(140).collect();
    if chosen.chars().count() > 140 {
        out.push('…');
    }
    out
}

/// Best-effort root-cause line from a job log.
///
/// Prefers the first *specific* error over the generic `ERROR: Job failed: exit
/// code N` trailer, which every failed job ends with and which says nothing.
fn error_signature(log: &str) -> Option<String> {
    let clean = strip_ansi(log);
    let mut fallback: Option<String> = None;
    for line in clean.lines() {
        let lower = line.to_ascii_lowercase();
        let Some(pos) = ["error:", "fatal:", "panic:"]
            .iter()
            .filter_map(|m| lower.find(m))
            .min()
        else {
            continue;
        };
        let sig = normalize_signature(line[pos..].trim());
        if sig.is_empty() {
            continue;
        }
        if sig.to_ascii_lowercase().starts_with("error: job failed") {
            fallback.get_or_insert(sig);
            continue;
        }
        return Some(sig);
    }
    fallback
}

/// Triage a failure as retryable or not. Deliberately conservative: anything not
/// clearly transient is **not** advertised as safe to retry.
fn failure_class(signature: &str, failure_reason: &str) -> &'static str {
    let r = failure_reason.to_ascii_lowercase();
    if r.contains("runner_system_failure")
        || r.contains("stuck_or_timeout")
        || r.contains("scheduler_failure")
        || r.contains("api_failure")
    {
        return "transient";
    }
    let s = signature.to_ascii_lowercase();
    const TRANSIENT: &[&str] = &[
        "timeout", "timed out", "connection reset", "temporarily unavailable", "rate limit",
        "too many requests", "tls handshake", "no space left", "i/o timeout", "unexpected eof",
        "could not resolve host", "connection refused", "502", "503", "504", "deadline exceeded",
    ];
    const CONFIG: &[&str] = &[
        "missing", "must be set", "not found", "no such file", "invalid", "unauthorized",
        "forbidden", "permission denied", "undefined", "undeclared", "does not exist",
        "unknown variable", "parse error", "syntax", "already exists", "conflict",
        "not set", "required", "unsupported",
    ];
    if TRANSIENT.iter().any(|m| s.contains(m)) {
        return "transient";
    }
    if CONFIG.iter().any(|m| s.contains(m)) {
        return "config";
    }
    "unknown"
}

/// Analyze pipeline health and cluster failures for a project or a whole group.
///
/// Separates **automated/operator runs** (the real signal) from development CI,
/// then pulls the log of each recent automated failure, extracts a root-cause
/// signature, clusters identical causes and triages each as transient (safe to
/// retry) or config/state (retrying just burns cycles).
pub async fn analyze_pipeline_failures(
    client: &GitLabClient,
    project_id: &str,
    group_path: &str,
    days: u32,
    max_logs: usize,
) -> Result<String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    // Resolve the scope to a concrete project list.
    let projects: Vec<(u64, String)> = if !group_path.is_empty() {
        let enc = urlencoding::encode(group_path);
        let list: Vec<Value> = client
            .get_all_pages(
                &format!("/groups/{enc}/projects"),
                &[
                    ("include_subgroups", "true"),
                    ("archived", "false"),
                    ("order_by", "last_activity_at"),
                    ("sort", "desc"),
                ],
                3,
            )
            .await?;
        list.iter()
            .filter_map(|p| {
                Some((
                    p["id"].as_u64()?,
                    p["path_with_namespace"].as_str()?.to_string(),
                ))
            })
            .collect()
    } else {
        let enc = urlencoding::encode(project_id);
        let p: Value = client.get(&format!("/projects/{enc}"), &[]).await?;
        vec![(
            p["id"].as_u64().unwrap_or_default(),
            p["path_with_namespace"]
                .as_str()
                .unwrap_or(project_id)
                .to_string(),
        )]
    };

    if projects.is_empty() {
        return Ok(format!("No projects found for '{group_path}{project_id}'."));
    }

    struct Run {
        project: String,
        project_id: u64,
        id: u64,
        status: String,
        automated: bool,
        source: String,
        duration: f64,
        web_url: String,
    }

    // Fetch each project's recent pipelines, bounded concurrency.
    let mut runs: Vec<Run> = Vec::new();
    for chunk in projects.chunks(ANALYZE_CONCURRENCY) {
        let futs = chunk.iter().map(|(pid, path)| {
            let since = since.clone();
            async move {
                let list: Vec<Value> = client
                    .get(
                        &format!("/projects/{pid}/pipelines"),
                        &[("updated_after", since.as_str()), ("per_page", "100")],
                    )
                    .await
                    .unwrap_or_default();
                (*pid, path.clone(), list)
            }
        });
        for (pid, path, list) in futures::future::join_all(futs).await {
            for p in &list {
                let source = p["source"].as_str().unwrap_or("unknown").to_string();
                runs.push(Run {
                    project: path.clone(),
                    project_id: pid,
                    id: p["id"].as_u64().unwrap_or(0),
                    status: p["status"].as_str().unwrap_or("?").to_string(),
                    automated: is_automated_source(&source),
                    source,
                    duration: p["duration"].as_f64().unwrap_or(0.0),
                    web_url: p["web_url"].as_str().unwrap_or("").to_string(),
                });
            }
        }
    }

    if runs.is_empty() {
        return Ok(format!(
            "No pipelines in the last {days}d for {} project(s).",
            projects.len()
        ));
    }

    let tally = |auto: bool, st: &str| {
        runs.iter()
            .filter(|r| r.automated == auto && r.status == st)
            .count()
    };
    let (a_total, d_total) = (
        runs.iter().filter(|r| r.automated).count(),
        runs.iter().filter(|r| !r.automated).count(),
    );
    let (a_ok, a_fail) = (tally(true, "success"), tally(true, "failed"));
    let (_d_ok, d_fail) = (tally(false, "success"), tally(false, "failed"));
    let finished = a_ok + a_fail;
    let rate = if finished > 0 {
        a_ok as f64 / finished as f64 * 100.0
    } else {
        0.0
    };
    let mut durs: Vec<f64> = runs
        .iter()
        .filter(|r| r.automated && r.status == "success" && r.duration > 0.0)
        .map(|r| r.duration)
        .collect();
    durs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Bridge/child pipelines report a null duration, so this can legitimately be
    // empty — say so rather than printing a confident "0s".
    let median = match durs.get(durs.len() / 2) {
        Some(d) => format!("{d:.0}s"),
        None => "n/a".to_string(),
    };

    let scope = if group_path.is_empty() { project_id } else { group_path };
    let mut out = vec![
        format!("# Pipeline failure analysis: `{scope}` (last {days}d)"),
        String::new(),
        format!(
            "**Automated / operator runs — the production signal:** {a_total} runs · ✅ {a_ok} · ❌ {a_fail} · success rate **{rate:.0}%** · median {median}"
        ),
        format!(
            "**Development CI (push / merge_request):** {d_total} runs · ❌ {d_fail} — *excluded from the numbers above; branch/MR failures are not customer impact*"
        ),
        String::new(),
    ];

    // Source mix — makes the signal/noise split auditable rather than asserted.
    let mut by_source: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
    for r in &runs {
        *by_source.entry(r.source.as_str()).or_default() += 1;
    }
    out.push(format!(
        "_Sources: {}_",
        by_source
            .iter()
            .map(|(s, n)| format!("{s} {n}"))
            .collect::<Vec<_>>()
            .join(" · ")
    ));
    out.push(String::new());

    // Cluster the automated failures by root cause.
    let mut failures: Vec<&Run> = runs
        .iter()
        .filter(|r| r.automated && r.status == "failed")
        .collect();
    failures.sort_by(|a, b| b.id.cmp(&a.id));
    let analyzed: Vec<&&Run> = failures.iter().take(max_logs).collect();

    if failures.is_empty() {
        out.push("No automated-run failures in the window. ✅".to_string());
        return Ok(out.join("\n"));
    }

    // (signature, class) -> (count, example run)
    let mut clusters: std::collections::BTreeMap<(String, String), (usize, String, String)> =
        std::collections::BTreeMap::new();

    for chunk in analyzed.chunks(ANALYZE_CONCURRENCY) {
        let futs = chunk.iter().map(|r| async move {
            let jobs: Vec<Value> = client
                .get(
                    &format!("/projects/{}/pipelines/{}/jobs", r.project_id, r.id),
                    &[("per_page", "100")],
                )
                .await
                .unwrap_or_default();
            let failed = jobs
                .iter()
                .find(|j| j["status"].as_str() == Some("failed"))
                .cloned();
            let (reason, sig) = match failed {
                Some(j) => {
                    let reason = j["failure_reason"].as_str().unwrap_or("").to_string();
                    let job_id = j["id"].as_u64().unwrap_or(0);
                    let log = client
                        .get_text(
                            &format!("/projects/{}/jobs/{job_id}/trace", r.project_id),
                            &[],
                        )
                        .await
                        .unwrap_or_default();
                    (reason.clone(), error_signature(&log))
                }
                None => (String::new(), None),
            };
            (r, reason, sig)
        });
        for (r, reason, sig) in futures::future::join_all(futs).await {
            let signature = sig.unwrap_or_else(|| {
                if reason.is_empty() {
                    "(no error line found in log)".to_string()
                } else {
                    format!("({reason})")
                }
            });
            let class = failure_class(&signature, &reason).to_string();
            let e = clusters
                .entry((class, signature))
                .or_insert((0, r.project.clone(), r.web_url.clone()));
            e.0 += 1;
        }
    }

    out.push(format!(
        "## Failure clusters ({} of {a_fail} automated failures analyzed)",
        analyzed.len()
    ));
    out.push(String::new());
    out.push("| # | Class | Root cause | Example |".to_string());
    out.push("|---|-------|-----------|---------|".to_string());
    let mut rows: Vec<_> = clusters.iter().collect();
    rows.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));
    for ((class, sig), (count, project, url)) in &rows {
        let icon = match class.as_str() {
            "transient" => "🔁",
            "config" => "🛠",
            _ => "❔",
        };
        let link = if url.is_empty() {
            project.clone()
        } else {
            format!("[{project}]({url})")
        };
        out.push(format!("| {count} | {icon} {class} | `{sig}` | {link} |"));
    }

    let transient: usize = rows
        .iter()
        .filter(|((c, _), _)| c == "transient")
        .map(|(_, (n, _, _))| n)
        .sum();
    let config: usize = rows
        .iter()
        .filter(|((c, _), _)| c == "config")
        .map(|(_, (n, _, _))| n)
        .sum();

    out.push(String::new());
    out.push("## Retry guidance".to_string());
    out.push(format!(
        "- 🔁 **{transient} transient** — infrastructure/network noise; retrying is likely to succeed."
    ));
    out.push(format!(
        "- 🛠 **{config} config/state** — **do not retry**: the same run will fail identically until the cause is fixed."
    ));
    out.push(
        "- ❔ **unknown** — inspect with `get_job_log` before deciding; not classified as retryable."
            .to_string(),
    );
    out.push(String::new());
    out.push(
        "> ⚠️ Before retrying infra pipelines, check the plan for **destroy** operations — a partial apply can leave state inconsistent, and a blind retry can make it worse."
            .to_string(),
    );

    Ok(out.join("\n"))
}

/// Key substrings whose values are **never** rendered. Checked first, so a key
/// like `<vendor>_CLIENT_ID` is denied even though it ends in `_ID`.
const SECRETISH: &[&str] = &[
    "SECRET", "TOKEN", "PASSWORD", "PASSWD", "PASS", "KEY", "CREDENTIAL", "PRIVATE", "AUTH",
    "CERT", "SALT", "SIGNATURE", "WEBHOOK", "DSN", "CLIENT_ID", "SESSION", "COOKIE",
];

/// Render one pipeline variable, **default-deny** on the value.
///
/// Trigger variables routinely carry credentials next to harmless identifiers, so
/// a value is shown only when all three hold: the key is not secret-ish, it looks
/// like a plain identifier, and the value itself is short and boring. Everything
/// else shows the key with `<redacted>` — the key name alone is useful for
/// debugging and low-risk, the value is not.
fn render_variable(key: &str, value: &str) -> String {
    let k = key.to_ascii_uppercase();
    let secretish = SECRETISH.iter().any(|m| k.contains(m));
    let identifier_shaped = k.ends_with("_UUID")
        || k.ends_with("_ID")
        || k.ends_with("_TYPE")
        || k.ends_with("_ACTION")
        || k.ends_with("_REGION")
        || k.ends_with("_NAME")
        || k == "ENVIRONMENT"
        || k == "REGION"
        || k == "TIER";
    let value_is_boring = value.len() <= 80
        && !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | ':' | '/'));

    if !secretish && identifier_shaped && value_is_boring {
        format!("- `{key}` = `{value}`")
    } else {
        format!("- `{key}` = `<redacted>`")
    }
}

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

    // Trigger variables. For `trigger`/`api` pipelines these are often the only
    // link from a CI failure back to the business object it was acting on (which
    // org, which network), so they turn "pipeline failed" into "customer X's
    // provisioning failed". Values are default-deny redacted — see
    // `render_variable`. Needs elevated scope; a 403/404 just omits the section.
    let vars: Vec<Value> = client
        .get(&format!("{path}/variables"), &[])
        .await
        .unwrap_or_default();
    if !vars.is_empty() {
        parts.push(String::new());
        parts.push(format!("## Trigger variables ({})", vars.len()));
        parts.push("_Values shown only for plain identifiers; everything else redacted._".into());
        for v in &vars {
            let key = v["key"].as_str().unwrap_or("?");
            let value = v["value"].as_str().unwrap_or("");
            parts.push(render_variable(key, value));
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
    use super::{
        error_signature, failure_class, is_automated_source, normalize_signature, render_variable,
        strip_ansi,
    };

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

    // ── Trigger-variable redaction (security-critical: default-deny) ──

    #[test]
    fn identifier_values_are_shown() {
        assert!(render_variable("ORG_UUID", "b1e2c3d4-0000-4a5b-8c9d-1234567890ab")
            .contains("b1e2c3d4"));
        assert!(render_variable("NODE_TYPE", "gateway").contains("`gateway`"));
        assert!(render_variable("ENVIRONMENT", "prod").contains("`prod`"));
    }

    #[test]
    fn secretish_keys_are_always_redacted() {
        // Every one of these is short and identifier-shaped, so only the
        // secret-ish check can save them.
        for key in [
            "VAULT_CLIENT_SECRET",
            "VAULT_CLIENT_ID", // half a credential pair — denied despite _ID
            "CI_JOB_TOKEN",
            "TF_VAR_api_key",
            "DB_PASSWORD",
            "PRIVATE_KEY",
            "SESSION_ID",
        ] {
            let out = render_variable(key, "abc123");
            assert!(out.contains("<redacted>"), "{key} leaked: {out}");
            assert!(!out.contains("abc123"), "{key} leaked its value: {out}");
        }
    }

    #[test]
    fn unknown_or_odd_shaped_values_are_redacted() {
        // Not identifier-shaped → redacted even though the key looks harmless.
        assert!(render_variable("SOMETHING", "whatever").contains("<redacted>"));
        // Identifier-shaped key but a long/odd value → still redacted.
        let long = "x".repeat(200);
        assert!(render_variable("ORG_UUID", &long).contains("<redacted>"));
        assert!(render_variable("ORG_UUID", "has spaces and $ymbols").contains("<redacted>"));
    }

    // ── Failure triage ──

    #[test]
    fn terraform_config_error_is_extracted_and_not_retryable() {
        let log = "\
2026-01-01T00:00:00Z 01O Plan: 5 to add, 0 to change, 3 to destroy.
2026-01-01T00:00:00Z 01E │ Error: Missing required configuration
2026-01-01T00:00:00Z 01E │ The 'server_url' must be set (via EXAMPLE_SERVER_URL).
2026-01-01T00:00:00Z 00O ERROR: Job failed: exit code 1";
        let sig = error_signature(log).expect("signature");
        assert!(sig.starts_with("Error: Missing required configuration"), "got: {sig}");
        // The generic trailer must not win over the specific cause.
        assert!(!sig.to_lowercase().contains("job failed"));
        assert_eq!(failure_class(&sig, "script_failure"), "config");
    }

    #[test]
    fn generic_trailer_is_only_a_fallback() {
        let log = "some output\nERROR: Job failed: exit code 137";
        let sig = error_signature(log).expect("signature");
        assert!(sig.to_lowercase().contains("job failed"));
    }

    #[test]
    fn transient_failures_are_classified_retryable() {
        assert_eq!(failure_class("", "runner_system_failure"), "transient");
        assert_eq!(failure_class("", "stuck_or_timeout_failure"), "transient");
        assert_eq!(
            failure_class("Error: dial tcp: i/o timeout", "script_failure"),
            "transient"
        );
        assert_eq!(
            failure_class("Error: 429 too many requests", "script_failure"),
            "transient"
        );
    }

    #[test]
    fn unrecognized_failures_are_not_advertised_as_retryable() {
        assert_eq!(failure_class("Error: something novel exploded", "script_failure"), "unknown");
    }

    /// Volatile IDs collapse so repeated instances of one fault cluster together.
    #[test]
    fn signatures_cluster_across_runs() {
        let a = normalize_signature("Error: node 7f3a1b2c-0000-4111-8222-333344445555 failed after 1234ms");
        let b = normalize_signature("Error: node 9c8d7e6f-1111-4222-8333-444455556666 failed after 987ms");
        assert_eq!(a, b, "same fault should normalize identically");
    }

    /// Real clusters that landed in "unknown" on the first live run.
    #[test]
    fn real_world_config_errors_are_recognized() {
        for sig in [
            "Error: Missing Hypervisor API Endpoint",
            "Error: Reference to undeclared resource",
            "Error: ttl must be set to 1 when `proxied` is true",
            "Error: Not Found for url: https://example/api/v3/secrets/raw",
        ] {
            assert_eq!(
                failure_class(sig, "script_failure"),
                "config",
                "should be config: {sig}"
            );
        }
    }

    /// A message that is mostly digits must not be masked into oblivion —
    /// the first live run produced a useless `error: …` cluster this way.
    #[test]
    fn numeric_heavy_messages_keep_their_text() {
        let sig = normalize_signature("error: 5432109876");
        assert_ne!(sig, "error: …", "masker ate the whole message");
        assert!(sig.contains("5432109876"), "got: {sig}");
        // A wordy message still masks its volatile parts.
        let wordy = normalize_signature("Error: node 12345678 unreachable after retry");
        assert!(wordy.contains('…') && wordy.contains("unreachable"), "got: {wordy}");
    }

    #[test]
    fn source_classification_splits_production_from_dev_ci() {
        for s in ["trigger", "api", "schedule", "web", "pipeline"] {
            assert!(is_automated_source(s), "{s} should count as automated");
        }
        for s in ["push", "merge_request_event"] {
            assert!(!is_automated_source(s), "{s} should count as dev CI");
        }
    }
}
