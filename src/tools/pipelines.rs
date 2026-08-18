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
    source: &str,
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
    if !source.is_empty() {
        params.push(("source", source));
    }

    let pipelines: Vec<Value> = client
        .get(&path, &params)
        .await
        ?;

    if pipelines.is_empty() {
        return Ok("No pipelines found.".to_string());
    }

    let filter_note = if source.is_empty() {
        String::new()
    } else {
        format!(" (source={source})")
    };
    let mut lines = vec![format!(
        "**Found: {} pipelines**{filter_note}\n",
        pipelines.len()
    )];

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

/// The first HTTP status code in a failure log, if one is stated.
///
/// Read from the **log** rather than the cluster signature: `normalize_signature`
/// masks runs of digits so that identical faults cluster together, which also erases
/// the codes — numeric markers tested against the signature could only ever fire on
/// the short-signature fallback path, and silently missed every other case.
///
/// A bare three-digit number is not evidence ("took 403 ms"), so a code counts only
/// when it sits next to a status word or carries its canonical reason phrase.
static HTTP_STATUS_RE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
    regex::Regex::new(
        r"(?i)(?:status[ _-]?code|statuscode|status|https?|response|returned|code)\D{0,4}(\d{3})\b|\b(\d{3})\s+(?:unauthorized|forbidden|not\s+found|unprocessable|too\s+many|bad\s+gateway|service\s+unavailable|gateway\s+time)",
    )
    .expect("HTTP_STATUS_RE is a valid regex")
});

fn http_status(hay: &str) -> Option<u16> {
    HTTP_STATUS_RE
        .captures_iter(hay)
        .filter_map(|c| c.get(1).or_else(|| c.get(2)))
        .filter_map(|m| m.as_str().parse::<u16>().ok())
        .find(|c| (400..600).contains(c))
}

/// Triage a failure. Deliberately conservative: anything not clearly transient is
/// **not** advertised as safe to retry.
///
/// Classified against the **whole log**, not just the cluster signature: the
/// decisive evidence ("already exists", `status-code=404`) is usually a few lines
/// below the `Error:` header that names the cluster.
///
/// `state` is separate from `config` on purpose. Both mean "do not retry", but the
/// remediation is unrelated — state drift is reconciled (import, state rm, delete
/// ordering), configuration is edited. Reporting drift as config sends people to
/// the wrong fix.
fn failure_class(signature: &str, failure_reason: &str, log: &str) -> &'static str {
    let r = failure_reason.to_ascii_lowercase();
    if r.contains("runner_system_failure")
        || r.contains("stuck_or_timeout")
        || r.contains("scheduler_failure")
        || r.contains("api_failure")
    {
        return "transient";
    }
    let s = signature.to_ascii_lowercase();
    let hay = format!("{s} {}", log.to_ascii_lowercase());

    // State drift: the provider's view and reality disagree.
    const STATE: &[&str] = &[
        "already exists", "already managed", "duplicate key", "state lock",
        "resource already", "currently in use",
    ];
    if STATE.iter().any(|m| hay.contains(m)) {
        return "state";
    }
    // A delete/destroy that 404s: the object is already gone — also drift, and the
    // desired end state is in fact reached.
    let removing = ["deleting", "destroying", "destroy", "removing"]
        .iter()
        .any(|m| hay.contains(m));
    let missing = ["not found", "status-code=404", "404", "no longer exists"]
        .iter()
        .any(|m| hay.contains(m));
    if removing && missing {
        return "state";
    }

    // A status code states the verdict that the surrounding prose often leaves out.
    // Checked before the word lists so that "403 Forbidden" is read as permission
    // rather than matching some unrelated word elsewhere in the log.
    if let Some(code) = http_status(&hay) {
        match code {
            408 | 425 | 429 | 500 | 502 | 503 | 504 => return "transient",
            400 | 401 | 403 | 404 | 405 | 409 | 422 => return "config",
            _ => {}
        }
    }
    const TRANSIENT: &[&str] = &[
        "timeout", "timed out", "connection reset", "temporarily unavailable", "rate limit",
        "too many requests", "tls handshake", "no space left", "i/o timeout", "unexpected eof",
        "could not resolve host", "connection refused", "deadline exceeded",
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
        /// created→finished; the only measure bridge pipelines always report.
        wall: Option<f64>,
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
                    wall: wall_clock_secs(
                        p["created_at"].as_str().unwrap_or(""),
                        p["finished_at"].as_str().unwrap_or(""),
                    ),
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
    // Wall clock, not job time: it answers "how long until this was done" and it
    // exists for bridge pipelines, whose `duration` is null.
    let mut durs: Vec<f64> = runs
        .iter()
        .filter(|r| r.automated && r.status == "success")
        .filter_map(|r| r.wall.or(if r.duration > 0.0 { Some(r.duration) } else { None }))
        .collect();
    durs.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    // Bridge/child pipelines report a null duration, so this can legitimately be
    // empty — say so rather than printing a confident "0s".
    let median = match durs.get(durs.len() / 2) {
        Some(d) => format!("{} wall clock", human_secs(*d)),
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
            let (reason, sig, log_tail) = match failed {
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
                    // Keep a bounded tail for classification — the decisive
                    // evidence sits below the Error: header that names the cluster.
                    let tail: String = log.chars().rev().take(4000).collect::<String>()
                        .chars().rev().collect();
                    (reason.clone(), error_signature(&log), tail)
                }
                None => (String::new(), None, String::new()),
            };
            (r, reason, sig, log_tail)
        });
        for (r, reason, sig, log_tail) in futures::future::join_all(futs).await {
            let signature = sig.unwrap_or_else(|| {
                if reason.is_empty() {
                    "(no error line found in log)".to_string()
                } else {
                    format!("({reason})")
                }
            });
            let class = failure_class(&signature, &reason, &log_tail).to_string();
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
            "state" => "🧭",
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

    let state: usize = rows
        .iter()
        .filter(|((c, _), _)| c == "state")
        .map(|(_, (n, _, _))| n)
        .sum();

    out.push(String::new());
    out.push("## Retry guidance".to_string());
    out.push(format!(
        "- 🔁 **{transient} transient** — infrastructure/network noise; retrying is likely to succeed."
    ));
    out.push(format!(
        "- 🧭 **{state} state drift** — **do not retry**: the recorded state and reality disagree (object already exists, or a delete found nothing). Reconcile instead — import the existing object, drop it from state, or fix delete ordering."
    ));
    out.push(format!(
        "- 🛠 **{config} config** — **do not retry**: the same run will fail identically until the configuration is fixed."
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

/// Seconds between two RFC-3339 timestamps.
///
/// GitLab's `duration` counts only job execution, so a run that waited an hour for
/// a runner reports the same number as one that started instantly. For provisioning
/// the customer-facing figure is created→finished, and bridge/child pipelines report
/// a null `duration` entirely — wall clock is the only measure that always exists.
fn wall_clock_secs(created: &str, finished: &str) -> Option<f64> {
    let c = chrono::DateTime::parse_from_rfc3339(created).ok()?;
    let f = chrono::DateTime::parse_from_rfc3339(finished).ok()?;
    let secs = (f - c).num_seconds();
    (secs >= 0).then_some(secs as f64)
}

/// Human duration: `1h 37m` / `4m 12s` / `26s`.
fn human_secs(s: f64) -> String {
    let s = s as u64;
    if s >= 3600 {
        format!("{}h {}m", s / 3600, (s % 3600) / 60)
    } else if s >= 60 {
        format!("{}m {}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
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
        format!("**Duration:** {duration_str} (job time)"),
    ];

    // Wall clock and queue time. `duration` counts execution only, so a run that
    // waited hours for a runner looks identical to one that started at once —
    // misleading wherever the question is "how long until this was done".
    if let Some(wall) = wall_clock_secs(created, finished) {
        let queued = p["queued_duration"].as_f64().unwrap_or(0.0);
        let mut line = format!("**Wall clock:** {} (created → finished)", human_secs(wall));
        if queued >= 1.0 {
            line.push_str(&format!(" · **queued {}**", human_secs(queued)));
        }
        // Long idle with little execution is the signature of runner starvation.
        if wall > 300.0 && duration > 0.0 && wall > duration * 10.0 {
            line.push_str(&format!(
                " ⚠️ only {duration:.0}s executing — the rest was waiting"
            ));
        }
        parts.push(line);
    }

    parts.extend([
        format!("**Created:** {created}"),
        format!("**Finished:** {finished}"),
        format!("**URL:** {web_url}"),
    ]);

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

    // Downstream (multi-project) pipelines. A provisioning run commonly spans
    // several projects via bridge jobs, so the failure that matters is often two
    // hops below the pipeline you're looking at; without this the parent is just
    // red with no explanation.
    //
    // `trigger_jobs` superseded `bridges` in GitLab 19.2 — try the current route
    // first and fall back, so this works across instance versions.
    let mut bridges: Vec<Value> = client
        .get(&format!("{path}/trigger_jobs"), &[("per_page", "100")])
        .await
        .unwrap_or_default();
    if bridges.is_empty() {
        bridges = client
            .get(&format!("{path}/bridges"), &[("per_page", "100")])
            .await
            .unwrap_or_default();
    }
    if !bridges.is_empty() {
        parts.push(String::new());
        parts.push(format!("## Downstream pipelines ({})", bridges.len()));
        for b in &bridges {
            let name = b["name"].as_str().unwrap_or("?");
            let b_status = b["status"].as_str().unwrap_or("?");
            let d = &b["downstream_pipeline"];
            match d["id"].as_u64() {
                Some(did) => {
                    let d_status = d["status"].as_str().unwrap_or("?");
                    let d_url = d["web_url"].as_str().unwrap_or("");
                    let icon = if d_status == "failed" { "❌" } else { "•" };
                    parts.push(format!(
                        "- {icon} **{name}** [{b_status}] → pipeline #{did} [{d_status}] {d_url}"
                    ));
                }
                // A bridge with no downstream never spawned one — usually the
                // trigger itself failed, which is worth seeing.
                None => parts.push(format!(
                    "- ⚠️ **{name}** [{b_status}] → no downstream pipeline created"
                )),
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
/// Lines of `log` matching `re`, 1-indexed, keeping at most `limit` — the **last**
/// matches, since the decisive error in a CI log sits near the end.
///
/// Returns the total number of matches alongside the kept ones, so a truncated
/// result can say how much it left out instead of looking complete.
fn grep_lines<'a>(log: &'a str, re: &regex::Regex, limit: usize) -> (usize, Vec<(usize, &'a str)>) {
    let hits: Vec<(usize, &str)> = log
        .lines()
        .enumerate()
        .filter(|(_, l)| re.is_match(l))
        .map(|(i, l)| (i + 1, l))
        .collect();
    let total = hits.len();
    let start = total.saturating_sub(limit);
    (total, hits[start..].to_vec())
}

pub async fn get_job_log(
    client: &GitLabClient,
    project_id: &str,
    job_id: u64,
    tail: usize,
    pattern: &str,
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

    // Pattern search scans the whole log, not the tail window — reaching the lines
    // the tail cuts off is the entire point of searching.
    if !pattern.is_empty() {
        let re = match regex::RegexBuilder::new(pattern)
            .case_insensitive(true)
            .build()
        {
            Ok(re) => re,
            Err(e) => {
                return Ok(format!(
                    "## Job #{job_id}: {name}\n{meta}\n\n**Invalid pattern** `{pattern}`: {e}"
                ));
            }
        };
        let total_lines = log_text.lines().count();
        let (total, hits) = grep_lines(&log_text, &re, tail);
        if total == 0 {
            return Ok(format!(
                "## Job #{job_id}: {name}\n{meta}\n\n*No line matches `{pattern}` ({total_lines} lines searched).*"
            ));
        }
        let note = if hits.len() < total {
            format!("*`{pattern}`: {total} matching lines of {total_lines}, showing last {}*", hits.len())
        } else {
            format!("*`{pattern}`: {total} matching lines of {total_lines}*")
        };
        let mut parts = vec![format!("## Job #{job_id}: {name}"), meta, note, String::new()];
        parts.push("```".to_string());
        for (n, l) in hits {
            parts.push(format!("{n}: {l}"));
        }
        parts.push("```".to_string());
        return Ok(parts.join("\n"));
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
        error_signature, failure_class, grep_lines, http_status, human_secs,
        is_automated_source, normalize_signature, render_variable, strip_ansi, wall_clock_secs,
    };

    #[test]
    fn http_status_needs_context_not_a_bare_number() {
        // Status words, in the forms CI logs actually use.
        assert_eq!(http_status("Error: status code 401 from provider"), Some(401));
        assert_eq!(http_status("HTTP 503 while calling the API"), Some(503));
        assert_eq!(http_status("request returned 422"), Some(422));
        assert_eq!(http_status("status-code=404"), Some(404));
        // Canonical reason phrase carries a code with no status word at all.
        assert_eq!(http_status("got 403 Forbidden from the registry"), Some(403));
        // A bare number is not a status: durations and counts must not be read as one.
        assert_eq!(http_status("provisioning took 403 ms"), None);
        assert_eq!(http_status("wrote 5024 bytes"), None);
        // 2xx/3xx are not failures.
        assert_eq!(http_status("HTTP 200 OK"), None);
    }

    #[test]
    fn classifies_by_http_status() {
        // Auth/permission/validation: retrying repeats the identical failure.
        assert_eq!(failure_class("", "script_failure", "HTTP 401 unauthorized"), "config");
        assert_eq!(failure_class("", "script_failure", "status code 403"), "config");
        assert_eq!(failure_class("", "script_failure", "response 422 unprocessable"), "config");
        // Server-side and throttling: worth another attempt.
        assert_eq!(failure_class("", "script_failure", "HTTP 429 too many requests"), "transient");
        assert_eq!(failure_class("", "script_failure", "status 503 service unavailable"), "transient");
    }

    #[test]
    fn numeric_codes_survive_signature_masking() {
        // Regression: signatures are normalized before classification, and
        // normalization masks digit runs — so a code tested against the signature
        // is already gone. It has to be read from the log.
        let sig = normalize_signature("Error: upstream call failed with a bad gateway response");
        assert!(!sig.contains("502"), "precondition: signature carries no code");
        assert_eq!(
            failure_class(&sig, "script_failure", "server responded: HTTP 502 bad gateway\n"),
            "transient"
        );
    }

    #[test]
    fn state_drift_outranks_a_404_status() {
        // A delete that 404s has reached its desired end state — reconcile, don't
        // treat it as broken configuration.
        assert_eq!(
            failure_class("", "script_failure", "Error deleting folder: status-code=404 not found"),
            "state"
        );
    }

    #[test]
    fn grep_lines_numbers_matches_and_keeps_the_last_ones() {
        let log = "start\nWARN one\nmiddle\nWARN two\nWARN three\ndone";
        let re = regex::RegexBuilder::new("warn")
            .case_insensitive(true)
            .build()
            .unwrap();

        let (total, hits) = grep_lines(log, &re, 10);
        assert_eq!(total, 3);
        // 1-indexed line numbers, so they line up with an editor.
        assert_eq!(hits, vec![(2, "WARN one"), (4, "WARN two"), (5, "WARN three")]);

        // Over the cap: keep the LAST matches — the decisive error is near the end.
        let (total, hits) = grep_lines(log, &re, 2);
        assert_eq!(total, 3, "total still reports everything found");
        assert_eq!(hits, vec![(4, "WARN two"), (5, "WARN three")]);

        // No match is not an error.
        let none = regex::Regex::new("nothing-here").unwrap();
        assert_eq!(grep_lines(log, &none, 5), (0, vec![]));
    }

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
        assert_eq!(failure_class(&sig, "script_failure", ""), "config");
    }

    #[test]
    fn generic_trailer_is_only_a_fallback() {
        let log = "some output\nERROR: Job failed: exit code 137";
        let sig = error_signature(log).expect("signature");
        assert!(sig.to_lowercase().contains("job failed"));
    }

    #[test]
    fn transient_failures_are_classified_retryable() {
        assert_eq!(failure_class("", "runner_system_failure", ""), "transient");
        assert_eq!(failure_class("", "stuck_or_timeout_failure", ""), "transient");
        assert_eq!(
            failure_class("Error: dial tcp: i/o timeout", "script_failure", ""),
            "transient"
        );
        assert_eq!(
            failure_class("Error: 429 too many requests", "script_failure", ""),
            "transient"
        );
    }

    #[test]
    fn unrecognized_failures_are_not_advertised_as_retryable() {
        assert_eq!(failure_class("Error: something novel exploded", "script_failure", ""), "unknown");
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
                failure_class(sig, "script_failure", ""),
                "config",
                "should be config: {sig}"
            );
        }
    }

    /// Both halves of a real state-drift loop: a create that collides with an
    /// existing object, and a delete that finds nothing. Same "don't retry"
    /// verdict as config, but a different fix — so they get their own class.
    #[test]
    fn state_drift_is_distinguished_from_config() {
        let create_collision = "Error: Error creating project secret folder\n\
            Unsuccessful response [POST /api/v1/folders] [status-code=400] \
            [message=\"Folder with name 'abc' already exists in path '/x/y'\"]";
        assert_eq!(
            failure_class("Error: Error creating project secret folder", "script_failure", create_collision),
            "state"
        );

        let delete_missing = "module.x.thing: Destroying... [id=1]\n\
            Error: Error deleting secret folder\n\
            Unsuccessful response [DELETE /api/v2/folders/1] [status-code=404] \
            [message=\"Folder with path '/x/y' not found\"]";
        assert_eq!(
            failure_class("Error: Error deleting secret folder", "script_failure", delete_missing),
            "state"
        );

        // A genuine config error must NOT be swept into the state bucket.
        assert_eq!(
            failure_class("Error: Missing Hypervisor API Endpoint", "script_failure", "endpoint must be set"),
            "config"
        );
    }

    #[test]
    fn wall_clock_and_human_duration() {
        // The shape that motivated this: long elapsed time, seconds of work.
        let wall = wall_clock_secs("2026-05-27T10:28:57Z", "2026-05-27T12:06:16Z").unwrap();
        assert_eq!(human_secs(wall), "1h 37m");
        assert_eq!(human_secs(26.0), "26s");
        assert_eq!(human_secs(252.0), "4m 12s");
        // Unparseable or reversed timestamps yield nothing rather than a bogus number.
        assert!(wall_clock_secs("", "").is_none());
        assert!(wall_clock_secs("2026-05-27T12:00:00Z", "2026-05-27T10:00:00Z").is_none());
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
