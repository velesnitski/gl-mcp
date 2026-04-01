//! GitLab merge request tools.

use crate::client::GitLabClient;
use crate::error::Result;
use serde_json::Value;
use std::collections::BTreeMap;

/// List merge requests.
pub async fn list_merge_requests(
    client: &GitLabClient,
    project_id: &str,
    state: &str,
    author: &str,
    scope: &str,
    created_after: &str,
    opened_before: &str,
    group_id: &str,
    per_page: u32,
) -> Result<String> {
    let per_page_str = per_page.to_string();
    let path = if !group_id.is_empty() {
        format!("/groups/{}/merge_requests", urlencoding::encode(group_id))
    } else if project_id.is_empty() {
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
    if !created_after.is_empty() {
        params.push(("created_after", created_after));
    }
    if !opened_before.is_empty() {
        params.push(("created_before", opened_before));
    }

    let mrs: Vec<Value> = client.get(&path, &params).await?;

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

        let pipeline_status = mr["head_pipeline"]["status"].as_str().or(mr["pipeline"]["status"].as_str()).unwrap_or("none");
        let ci = if pipeline_status != "none" { format!(" [CI: {pipeline_status}]") } else { String::new() };

        let reviewers: Vec<&str> = mr["reviewers"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v["username"].as_str()).collect())
            .unwrap_or_default();
        let rev_str = if reviewers.is_empty() { String::new() } else { format!(" reviewers: {}", reviewers.iter().map(|r| format!("@{r}")).collect::<Vec<_>>().join(", ")) };

        lines.push(format!(
            "- **{project}** [{state}]{draft}{ci} {title} ({source} → {target}) by @{author} ({created_short}){rev_str}"
        ));
    }

    Ok(lines.join("\n"))
}

/// Smart MR title from branch name: "feature/PROJ-123-add-oauth" → "PROJ-123: Add OAuth"
fn title_from_branch(branch: &str) -> String {
    // Strip known prefixes
    let stripped = branch
        .trim_start_matches("feature/")
        .trim_start_matches("feat/")
        .trim_start_matches("hotfix/")
        .trim_start_matches("bugfix/")
        .trim_start_matches("fix/")
        .trim_start_matches("chore/")
        .trim_start_matches("docs/")
        .trim_start_matches("test/")
        .trim_start_matches("ci/")
        .trim_start_matches("refactor/");

    // Check for ticket prefix like PROJ-123
    let re = regex::Regex::new(r"^([A-Z]+-\d+)-?(.*)$").unwrap();
    if let Some(caps) = re.captures(stripped) {
        let ticket = &caps[1];
        let rest = caps[2].replace('-', " ").replace('_', " ");
        let rest = rest.trim();
        if rest.is_empty() {
            return ticket.to_string();
        }
        // Capitalize first letter
        let mut chars = rest.chars();
        let first = chars.next().unwrap().to_uppercase().to_string();
        return format!("{ticket}: {first}{}", chars.as_str());
    }

    // No ticket — just humanize
    let humanized = stripped.replace('-', " ").replace('_', " ");
    let humanized = humanized.trim();
    if humanized.is_empty() {
        return branch.to_string();
    }
    let mut chars = humanized.chars();
    let first = chars.next().unwrap().to_uppercase().to_string();
    format!("{first}{}", chars.as_str())
}

/// Create a merge request with smart defaults.
pub async fn create_merge_request(
    client: &GitLabClient,
    project_id: &str,
    source_branch: &str,
    target_branch: &str,
    title: &str,
    description: &str,
    labels: &str,
    assignee: &str,
    reviewers: &str,
    squash: bool,
    remove_source_branch: bool,
    draft: bool,
) -> Result<String> {
    let enc = urlencoding::encode(project_id);

    // 1. Resolve target branch if empty — use project default
    let actual_target = if target_branch.is_empty() {
        let proj: Value = client
            .get(&format!("/projects/{enc}"), &[])
            .await?;
        proj["default_branch"]
            .as_str()
            .unwrap_or("main")
            .to_string()
    } else {
        target_branch.to_string()
    };

    // 2. Check source branch exists
    let branches_path = format!("/projects/{enc}/repository/branches/{}", urlencoding::encode(source_branch));
    let branch_check: std::result::Result<Value, _> = client.get(&branches_path, &[]).await;
    if branch_check.is_err() {
        return Ok(format!("**Error:** Source branch `{source_branch}` not found in project `{project_id}`."));
    }

    // 3. Check for existing open MR with same source→target
    let existing: Vec<Value> = client
        .get(
            &format!("/projects/{enc}/merge_requests"),
            &[
                ("source_branch", source_branch),
                ("target_branch", &actual_target),
                ("state", "opened"),
            ],
        )
        .await?;
    if !existing.is_empty() {
        let mr = &existing[0];
        let iid = mr["iid"].as_u64().unwrap_or(0);
        let url = mr["web_url"].as_str().unwrap_or("?");
        return Ok(format!(
            "**MR already exists:** !{iid} ({source_branch} → {actual_target})\n**URL:** {url}"
        ));
    }

    // 4. Auto-generate title from branch if not provided
    let actual_title = if title.is_empty() {
        title_from_branch(source_branch)
    } else {
        title.to_string()
    };

    // 5. Auto-generate description from commits between source and target
    let actual_description = if description.is_empty() {
        let compare: Value = client
            .get(
                &format!("/projects/{enc}/repository/compare"),
                &[("from", actual_target.as_str()), ("to", source_branch)],
            )
            .await
            .unwrap_or(Value::Null);

        let commits = compare["commits"].as_array();
        if let Some(commits) = commits {
            if commits.is_empty() {
                String::new()
            } else {
                let mut lines = vec!["## Commits".to_string(), String::new()];
                for c in commits.iter().rev() {
                    let short = c["short_id"].as_str().unwrap_or("?");
                    let msg = c["title"].as_str().unwrap_or("?");
                    lines.push(format!("- `{short}` {msg}"));
                }
                lines.join("\n")
            }
        } else {
            String::new()
        }
    } else {
        description.to_string()
    };

    // 6. Resolve assignee and reviewer user IDs
    let mut body = serde_json::json!({
        "source_branch": source_branch,
        "target_branch": actual_target,
        "title": if draft { format!("Draft: {actual_title}") } else { actual_title.clone() },
        "squash": squash,
        "remove_source_branch": remove_source_branch,
    });

    if !actual_description.is_empty() {
        body["description"] = Value::String(actual_description);
    }

    if !labels.is_empty() {
        body["labels"] = Value::String(labels.to_string());
    }

    if !assignee.is_empty() {
        // Resolve username to ID
        let users: Vec<Value> = client
            .get("/users", &[("username", assignee)])
            .await
            .unwrap_or_default();
        if let Some(user) = users.first() {
            if let Some(id) = user["id"].as_u64() {
                body["assignee_id"] = Value::Number(id.into());
            }
        }
    }

    if !reviewers.is_empty() {
        let mut reviewer_ids = Vec::new();
        for username in reviewers.split(',').map(|s| s.trim()).filter(|s| !s.is_empty()) {
            let users: Vec<Value> = client
                .get("/users", &[("username", username)])
                .await
                .unwrap_or_default();
            if let Some(user) = users.first() {
                if let Some(id) = user["id"].as_u64() {
                    reviewer_ids.push(Value::Number(id.into()));
                }
            }
        }
        if !reviewer_ids.is_empty() {
            body["reviewer_ids"] = Value::Array(reviewer_ids);
        }
    }

    // 7. Create the MR
    let mr: Value = client
        .post(&format!("/projects/{enc}/merge_requests"), &body)
        .await?;

    let iid = mr["iid"].as_u64().unwrap_or(0);
    let web_url = mr["web_url"].as_str().unwrap_or("?");
    let state = mr["state"].as_str().unwrap_or("?");
    let mr_title = mr["title"].as_str().unwrap_or("?");
    let src = mr["source_branch"].as_str().unwrap_or("?");
    let tgt = mr["target_branch"].as_str().unwrap_or("?");

    // 8. Get diff stats for summary
    let changes: std::result::Result<Value, _> = client
        .get(
            &format!("/projects/{enc}/merge_requests/{iid}/changes"),
            &[("access_raw_diffs", "false")],
        )
        .await;

    let diff_stats = if let Ok(ch) = changes {
        let files = ch["changes"].as_array().map(|a| a.len()).unwrap_or(0);
        let adds: i64 = ch["changes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| f["diff"].as_str())
                    .flat_map(|d| d.lines())
                    .filter(|l| l.starts_with('+') && !l.starts_with("+++"))
                    .count() as i64
            })
            .unwrap_or(0);
        let dels: i64 = ch["changes"]
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|f| f["diff"].as_str())
                    .flat_map(|d| d.lines())
                    .filter(|l| l.starts_with('-') && !l.starts_with("---"))
                    .count() as i64
            })
            .unwrap_or(0);
        format!("{files} files (+{adds}, -{dels})")
    } else {
        "unknown".to_string()
    };

    Ok(format!(
        "**Created: !{iid}** [{state}] {mr_title}\n\
         **Branch:** {src} → {tgt}\n\
         **Changes:** {diff_stats}\n\
         **URL:** {web_url}"
    ))
}

/// Get a single merge request with details.
pub async fn get_merge_request(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
    include_notes: bool,
) -> Result<String> {
    let path = format!(
        "/projects/{}/merge_requests/{}",
        urlencoding::encode(project_id),
        mr_iid
    );
    let mr: Value = client.get(&path, &[]).await?;

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

/// Get MR review turnaround stats for a project or group.
pub async fn get_mr_turnaround(
    client: &GitLabClient,
    project_id: &str,
    group_id: &str,
    days: u32,
) -> Result<String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let path = if !group_id.is_empty() {
        format!("/groups/{}/merge_requests", urlencoding::encode(group_id))
    } else if !project_id.is_empty() {
        format!("/projects/{}/merge_requests", urlencoding::encode(project_id))
    } else {
        "/merge_requests".to_string()
    };

    let mrs: Vec<Value> = client.get(&path, &[
        ("state", "merged"),
        ("created_after", &since),
        ("per_page", "50"),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ]).await?;

    if mrs.is_empty() {
        return Ok("No merged MRs found in this period.".to_string());
    }

    struct MrStats {
        iid: u64,
        title: String,
        author: String,
        merged_by: String,
        hours_to_merge: f64,
        project: String,
    }

    let mut stats: Vec<MrStats> = Vec::new();

    for mr in &mrs {
        let created = mr["created_at"].as_str().unwrap_or("");
        let merged = mr["merged_at"].as_str().unwrap_or("");

        let created_dt = chrono::DateTime::parse_from_rfc3339(created).ok();
        let merged_dt = chrono::DateTime::parse_from_rfc3339(merged).ok();

        if let (Some(c), Some(m)) = (created_dt, merged_dt) {
            let hours = (m - c).num_minutes() as f64 / 60.0;
            let iid = mr["iid"].as_u64().unwrap_or(0);
            let title = mr["title"].as_str().unwrap_or("?").to_string();
            let author = mr["author"]["username"].as_str().unwrap_or("?").to_string();
            let merged_by = mr["merged_by"]["username"].as_str().unwrap_or("?").to_string();
            let project = mr["references"]["full"].as_str()
                .unwrap_or("")
                .split('!')
                .next()
                .unwrap_or("?")
                .to_string();

            stats.push(MrStats { iid, title, author, merged_by, hours_to_merge: hours, project });
        }
    }

    if stats.is_empty() {
        return Ok("No MRs with valid timestamps found.".to_string());
    }

    let total: f64 = stats.iter().map(|s| s.hours_to_merge).sum();
    let avg = total / stats.len() as f64;
    let median = {
        let mut sorted: Vec<f64> = stats.iter().map(|s| s.hours_to_merge).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        sorted[sorted.len() / 2]
    };
    let max = stats.iter().map(|s| s.hours_to_merge).fold(0.0f64, f64::max);
    let min = stats.iter().map(|s| s.hours_to_merge).fold(f64::MAX, f64::min);

    let mut by_author: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in &stats {
        by_author.entry(s.author.clone()).or_default().push(s.hours_to_merge);
    }

    let scope = if !group_id.is_empty() { group_id } else { project_id };
    let mut lines = vec![
        format!("**MR Turnaround: {scope}** (last {days} days, {} MRs)\n", stats.len()),
        format!("| Metric | Value |"),
        format!("|--------|-------|"),
        format!("| Average | {:.1}h |", avg),
        format!("| Median | {:.1}h |", median),
        format!("| Fastest | {:.1}h |", min),
        format!("| Slowest | {:.1}h |", max),
        String::new(),
        format!("**By author:**"),
    ];

    for (author, times) in &by_author {
        let author_avg: f64 = times.iter().sum::<f64>() / times.len() as f64;
        lines.push(format!("- @{author}: {} MRs, avg {:.1}h", times.len(), author_avg));
    }

    let mut by_merger: BTreeMap<String, Vec<f64>> = BTreeMap::new();
    for s in &stats {
        by_merger.entry(s.merged_by.clone()).or_default().push(s.hours_to_merge);
    }
    lines.push(String::new());
    lines.push("**By merger (who merged):**".to_string());
    for (merger, times) in &by_merger {
        let merger_avg: f64 = times.iter().sum::<f64>() / times.len() as f64;
        lines.push(format!("- @{merger}: {} MRs merged, avg {:.1}h", times.len(), merger_avg));
    }

    stats.sort_by(|a, b| b.hours_to_merge.partial_cmp(&a.hours_to_merge).unwrap());
    lines.push(String::new());
    lines.push("**Slowest MRs:**".to_string());
    for s in stats.iter().take(5) {
        let duration = if s.hours_to_merge > 24.0 {
            format!("{:.1}d", s.hours_to_merge / 24.0)
        } else {
            format!("{:.1}h", s.hours_to_merge)
        };
        lines.push(format!("- {}!{} {} (@{}, merged by @{}) — {duration}", s.project, s.iid, s.title, s.author, s.merged_by));
    }

    Ok(lines.join("\n"))
}

/// Compact MR dashboard for a group: open count, avg age, reviewer bottlenecks.
pub async fn get_mr_dashboard(
    client: &GitLabClient,
    group_id: &str,
) -> Result<String> {
    let path = format!("/groups/{}/merge_requests", urlencoding::encode(group_id));

    let mrs: Vec<Value> = client.get(&path, &[
        ("state", "opened"),
        ("per_page", "100"),
        ("order_by", "created_at"),
        ("sort", "asc"),
    ]).await?;

    if mrs.is_empty() {
        return Ok(format!("No open MRs in group {group_id}."));
    }

    let now = chrono::Utc::now();

    struct MrInfo {
        iid: u64,
        project: String,
        title: String,
        #[allow(dead_code)]
        author: String,
        age_hours: f64,
        reviewers: Vec<String>,
        draft: bool,
    }

    let mut infos: Vec<MrInfo> = Vec::new();
    let mut reviewer_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut _author_counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut project_counts: BTreeMap<String, u32> = BTreeMap::new();

    for mr in &mrs {
        let created = mr["created_at"].as_str().unwrap_or("");
        let age_hours = chrono::DateTime::parse_from_rfc3339(created)
            .ok()
            .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_hours() as f64)
            .unwrap_or(0.0);

        let author = mr["author"]["username"].as_str().unwrap_or("?").to_string();
        let project = mr["references"]["full"].as_str()
            .unwrap_or("")
            .split('!')
            .next()
            .unwrap_or("?")
            .to_string();
        let draft = mr["draft"].as_bool().unwrap_or(false);
        let iid = mr["iid"].as_u64().unwrap_or(0);
        let title = mr["title"].as_str().unwrap_or("?").to_string();

        let reviewers: Vec<String> = mr["reviewers"]
            .as_array()
            .map(|a| a.iter().filter_map(|v| v["username"].as_str().map(String::from)).collect())
            .unwrap_or_default();

        *_author_counts.entry(author.clone()).or_default() += 1;
        *project_counts.entry(project.clone()).or_default() += 1;
        for r in &reviewers {
            *reviewer_counts.entry(r.clone()).or_default() += 1;
        }

        infos.push(MrInfo { iid, project, title, author, age_hours, reviewers, draft });
    }

    let total = infos.len();
    let drafts = infos.iter().filter(|m| m.draft).count();
    let avg_age = infos.iter().map(|m| m.age_hours).sum::<f64>() / total as f64;
    let stale_count = infos.iter().filter(|m| m.age_hours > 168.0).count();
    let no_reviewer = infos.iter().filter(|m| m.reviewers.is_empty()).count();

    let mut lines = vec![
        format!("**MR Dashboard: {group_id}** ({total} open)\n"),
        format!("| Metric | Value |"),
        format!("|--------|-------|"),
        format!("| Open MRs | {total} |"),
        format!("| Drafts | {drafts} |"),
        format!("| Avg age | {:.0}h ({:.1}d) |", avg_age, avg_age / 24.0),
        format!("| Stale (>7d) | {stale_count} |"),
        format!("| No reviewer | {no_reviewer} |"),
    ];

    if !reviewer_counts.is_empty() {
        lines.push(String::new());
        lines.push("**Reviewer load:**".to_string());
        let mut sorted_reviewers: Vec<_> = reviewer_counts.iter().collect();
        sorted_reviewers.sort_by(|a, b| b.1.cmp(a.1));
        for (reviewer, count) in sorted_reviewers {
            lines.push(format!("- @{reviewer}: {count} MRs to review"));
        }
    }

    lines.push(String::new());
    lines.push("**By project:**".to_string());
    let mut sorted_projects: Vec<_> = project_counts.iter().collect();
    sorted_projects.sort_by(|a, b| b.1.cmp(a.1));
    for (project, count) in sorted_projects {
        lines.push(format!("- {project}: {count} open"));
    }

    if stale_count > 0 {
        lines.push(String::new());
        lines.push("**Stale MRs (>7 days):**".to_string());
        for m in infos.iter().filter(|m| m.age_hours > 168.0) {
            let age_days = m.age_hours / 24.0;
            let rev = if m.reviewers.is_empty() { "no reviewer".to_string() } else { m.reviewers.iter().map(|r| format!("@{r}")).collect::<Vec<_>>().join(", ") };
            lines.push(format!("- {}!{} {} ({:.0}d old, {rev})", m.project, m.iid, m.title, age_days));
        }
    }

    Ok(lines.join("\n"))
}

/// Get MR review depth: comments/discussions per MR before merge.
pub async fn get_mr_review_depth(
    client: &GitLabClient,
    project_id: &str,
    group_id: &str,
    days: u32,
) -> Result<String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let path = if !group_id.is_empty() {
        format!("/groups/{}/merge_requests", urlencoding::encode(group_id))
    } else if !project_id.is_empty() {
        format!("/projects/{}/merge_requests", urlencoding::encode(project_id))
    } else {
        "/merge_requests".to_string()
    };

    let mrs: Vec<Value> = client.get(&path, &[
        ("state", "merged"),
        ("created_after", &since),
        ("per_page", "30"),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ]).await?;

    if mrs.is_empty() {
        return Ok("No merged MRs found in this period.".to_string());
    }

    struct ReviewInfo {
        iid: u64,
        project: String,
        title: String,
        author: String,
        discussions: u64,
        user_notes: u64,
    }

    let mut infos: Vec<ReviewInfo> = Vec::new();

    for mr in &mrs {
        let iid = mr["iid"].as_u64().unwrap_or(0);
        let user_notes = mr["user_notes_count"].as_u64().unwrap_or(0);
        let title = mr["title"].as_str().unwrap_or("?").to_string();
        let author = mr["author"]["username"].as_str().unwrap_or("?").to_string();
        let project = mr["references"]["full"].as_str()
            .unwrap_or("")
            .split('!')
            .next()
            .unwrap_or("?")
            .to_string();

        // Fetch discussions count from API
        let proj_path = mr["source_project_id"].as_u64().unwrap_or(0);
        let disc_path = format!("/projects/{}/merge_requests/{}/discussions", proj_path, iid);
        let discussions: Vec<Value> = client.get(&disc_path, &[("per_page", "100")]).await.unwrap_or_default();
        let non_system = discussions.iter().filter(|d| {
            d["notes"].as_array()
                .map(|notes| notes.iter().any(|n| !n["system"].as_bool().unwrap_or(true)))
                .unwrap_or(false)
        }).count() as u64;

        infos.push(ReviewInfo { iid, project, title, author, discussions: non_system, user_notes });
    }

    let total_notes: u64 = infos.iter().map(|i| i.user_notes).sum();
    let total_discussions: u64 = infos.iter().map(|i| i.discussions).sum();
    let avg_notes = total_notes as f64 / infos.len() as f64;
    let avg_disc = total_discussions as f64 / infos.len() as f64;
    let zero_review = infos.iter().filter(|i| i.user_notes == 0).count();

    let scope = if !group_id.is_empty() { group_id } else { project_id };
    let mut lines = vec![
        format!("**Review Depth: {scope}** (last {days}d, {} MRs)\n", infos.len()),
        format!("| Metric | Value |"),
        format!("|--------|-------|"),
        format!("| Avg comments/MR | {:.1} |", avg_notes),
        format!("| Avg discussions/MR | {:.1} |", avg_disc),
        format!("| Zero-comment MRs | {} ({:.0}%) |", zero_review, zero_review as f64 / infos.len() as f64 * 100.0),
    ];

    // Per-author depth
    let mut by_author: BTreeMap<String, (u64, u64, u64)> = BTreeMap::new();
    for i in &infos {
        let e = by_author.entry(i.author.clone()).or_default();
        e.0 += 1; // MRs
        e.1 += i.user_notes; // comments
        e.2 += i.discussions; // discussions
    }
    lines.push(String::new());
    lines.push("**By author:**".to_string());
    for (author, (mrs_count, notes, disc)) in &by_author {
        let avg = *notes as f64 / *mrs_count as f64;
        lines.push(format!("- @{author}: {mrs_count} MRs, {notes} comments ({avg:.1} avg), {disc} discussions"));
    }

    // Most-discussed MRs
    infos.sort_by(|a, b| b.discussions.cmp(&a.discussions));
    lines.push(String::new());
    lines.push("**Most discussed:**".to_string());
    for i in infos.iter().take(5) {
        lines.push(format!("- {}!{} {} (@{}) — {} discussions, {} comments", i.project, i.iid, i.title, i.author, i.discussions, i.user_notes));
    }

    Ok(lines.join("\n"))
}

/// Classify MRs by category (feature, hotfix, bugfix, chore) based on branch names and titles.
pub async fn get_mr_categories(
    client: &GitLabClient,
    project_id: &str,
    group_id: &str,
    state: &str,
    days: u32,
) -> Result<String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let path = if !group_id.is_empty() {
        format!("/groups/{}/merge_requests", urlencoding::encode(group_id))
    } else if !project_id.is_empty() {
        format!("/projects/{}/merge_requests", urlencoding::encode(project_id))
    } else {
        "/merge_requests".to_string()
    };

    let state_param = if state.is_empty() { "all" } else { state };
    let mrs: Vec<Value> = client.get(&path, &[
        ("state", state_param),
        ("created_after", &since),
        ("per_page", "100"),
        ("order_by", "created_at"),
        ("sort", "desc"),
    ]).await?;

    if mrs.is_empty() {
        return Ok("No MRs found in this period.".to_string());
    }

    fn classify(branch: &str, title: &str) -> &'static str {
        let b = branch.to_lowercase();
        let t = title.to_lowercase();
        if b.starts_with("hotfix/") || b.starts_with("hotfix-") || t.starts_with("hotfix") { return "hotfix"; }
        if b.starts_with("bugfix/") || b.starts_with("fix/") || t.starts_with("fix") { return "bugfix"; }
        if b.starts_with("feature/") || b.starts_with("feat/") || t.starts_with("feat") { return "feature"; }
        if b.starts_with("chore/") || b.starts_with("refactor/") || t.starts_with("chore") || t.starts_with("refactor") { return "chore"; }
        if b.starts_with("docs/") || t.starts_with("docs") { return "docs"; }
        if b.starts_with("test/") || t.starts_with("test") { return "test"; }
        if b.starts_with("ci/") || b.starts_with("devops/") { return "ci/devops"; }
        "other"
    }

    let mut by_category: BTreeMap<&str, Vec<(&str, &str, &str)>> = BTreeMap::new(); // category -> [(title, author, state)]
    let mut by_author_cat: BTreeMap<String, BTreeMap<&str, u32>> = BTreeMap::new();

    for mr in &mrs {
        let branch = mr["source_branch"].as_str().unwrap_or("");
        let title = mr["title"].as_str().unwrap_or("?");
        let author = mr["author"]["username"].as_str().unwrap_or("?");
        let mr_state = mr["state"].as_str().unwrap_or("?");
        let cat = classify(branch, title);
        by_category.entry(cat).or_default().push((title, author, mr_state));
        *by_author_cat.entry(author.to_string()).or_default().entry(cat).or_default() += 1;
    }

    let scope = if !group_id.is_empty() { group_id } else { project_id };
    let mut lines = vec![
        format!("**MR Categories: {scope}** (last {days}d, {} MRs, state: {state_param})\n", mrs.len()),
        "| Category | Count | % |".to_string(),
        "|----------|-------|---|".to_string(),
    ];

    let total = mrs.len() as f64;
    let mut sorted_cats: Vec<_> = by_category.iter().collect();
    sorted_cats.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (cat, items) in &sorted_cats {
        let pct = items.len() as f64 / total * 100.0;
        lines.push(format!("| {} | {} | {:.0}% |", cat, items.len(), pct));
    }

    // Author x Category matrix
    lines.push(String::new());
    lines.push("**By author:**".to_string());
    for (author, cats) in &by_author_cat {
        let parts: Vec<String> = cats.iter().map(|(cat, count)| format!("{cat}: {count}")).collect();
        lines.push(format!("- @{author}: {}", parts.join(", ")));
    }

    Ok(lines.join("\n"))
}

/// Decompose MR merge time into queue time (creation → first review) and review time (first review → merge).
pub async fn get_mr_timeline(
    client: &GitLabClient,
    project_id: &str,
    group_id: &str,
    days: u32,
) -> Result<String> {
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let path = if !group_id.is_empty() {
        format!("/groups/{}/merge_requests", urlencoding::encode(group_id))
    } else if !project_id.is_empty() {
        format!("/projects/{}/merge_requests", urlencoding::encode(project_id))
    } else {
        "/merge_requests".to_string()
    };

    let mrs: Vec<Value> = client.get(&path, &[
        ("state", "merged"),
        ("created_after", &since),
        ("per_page", "30"),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ]).await?;

    if mrs.is_empty() {
        return Ok("No merged MRs found in this period.".to_string());
    }

    struct Timeline {
        iid: u64,
        project: String,
        title: String,
        author: String,
        total_hours: f64,
        queue_hours: f64,   // creation → first non-author action
        review_hours: f64,  // first non-author action → merge
        had_review: bool,
    }

    let mut timelines: Vec<Timeline> = Vec::new();

    for mr in &mrs {
        let created_str = mr["created_at"].as_str().unwrap_or("");
        let merged_str = mr["merged_at"].as_str().unwrap_or("");
        let created = chrono::DateTime::parse_from_rfc3339(created_str).ok();
        let merged = chrono::DateTime::parse_from_rfc3339(merged_str).ok();
        let (Some(created_dt), Some(merged_dt)) = (created, merged) else { continue };

        let iid = mr["iid"].as_u64().unwrap_or(0);
        let title = mr["title"].as_str().unwrap_or("?").to_string();
        let author = mr["author"]["username"].as_str().unwrap_or("?").to_string();
        let project_id_num = mr["source_project_id"].as_u64().unwrap_or(0);
        let project = mr["references"]["full"].as_str()
            .unwrap_or("").split('!').next().unwrap_or("?").to_string();

        let total_hours = (merged_dt - created_dt).num_minutes() as f64 / 60.0;

        // Fetch notes to find first non-author action
        let notes_path = format!("/projects/{}/merge_requests/{}/notes", project_id_num, iid);
        let notes: Vec<Value> = client.get(&notes_path, &[("per_page", "50"), ("sort", "asc")]).await.unwrap_or_default();

        let first_review_ts = notes.iter().find_map(|n| {
            let note_author = n["author"]["username"].as_str().unwrap_or("");
            let is_system = n["system"].as_bool().unwrap_or(false);
            // Find first non-author activity (comment or system approval)
            if note_author != author || (is_system && n["body"].as_str().unwrap_or("").contains("approved")) {
                n["created_at"].as_str()
                    .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            } else {
                None
            }
        });

        let (queue_hours, review_hours, had_review) = if let Some(review_dt) = first_review_ts {
            let q = (review_dt - created_dt).num_minutes() as f64 / 60.0;
            let r = (merged_dt - review_dt).num_minutes() as f64 / 60.0;
            (q.max(0.0), r.max(0.0), true)
        } else {
            (total_hours, 0.0, false)
        };

        timelines.push(Timeline { iid, project, title, author, total_hours, queue_hours, review_hours, had_review });
    }

    if timelines.is_empty() {
        return Ok("No MRs with valid timestamps found.".to_string());
    }

    let count = timelines.len();
    let avg_total = timelines.iter().map(|t| t.total_hours).sum::<f64>() / count as f64;
    let avg_queue = timelines.iter().map(|t| t.queue_hours).sum::<f64>() / count as f64;
    let avg_review = timelines.iter().filter(|t| t.had_review).map(|t| t.review_hours).sum::<f64>() / timelines.iter().filter(|t| t.had_review).count().max(1) as f64;
    let no_review_count = timelines.iter().filter(|t| !t.had_review).count();

    let scope = if !group_id.is_empty() { group_id } else { project_id };
    let mut lines = vec![
        format!("**MR Timeline: {scope}** (last {days}d, {count} MRs)\n"),
        "| Phase | Avg Time |".to_string(),
        "|-------|----------|".to_string(),
        format!("| Total (creation → merge) | {:.1}h |", avg_total),
        format!("| Queue (creation → first review) | {:.1}h |", avg_queue),
        format!("| Review (first review → merge) | {:.1}h |", avg_review),
        format!("| No review activity | {} ({:.0}%) |", no_review_count, no_review_count as f64 / count as f64 * 100.0),
    ];

    // Longest queue times
    timelines.sort_by(|a, b| b.queue_hours.partial_cmp(&a.queue_hours).unwrap());
    lines.push(String::new());
    lines.push("**Longest queue (waiting for first review):**".to_string());
    for t in timelines.iter().take(5) {
        let queue_str = if t.queue_hours > 24.0 { format!("{:.1}d", t.queue_hours / 24.0) } else { format!("{:.1}h", t.queue_hours) };
        let review_note = if t.had_review { format!(", review: {:.1}h", t.review_hours) } else { ", no review".to_string() };
        lines.push(format!("- {}!{} {} (@{}) — queue: {queue_str}{review_note}", t.project, t.iid, t.title, t.author));
    }

    Ok(lines.join("\n"))
}

/// Cross-group MR dashboard: aggregates multiple groups.
pub async fn get_org_mr_dashboard(
    client: &GitLabClient,
    group_ids: &[&str],
) -> Result<String> {
    let now = chrono::Utc::now();

    struct GroupStats {
        name: String,
        open: u32,
        drafts: u32,
        stale: u32,
        no_reviewer: u32,
        avg_age_hours: f64,
        reviewers: BTreeMap<String, u32>,
    }

    let mut all_stats: Vec<GroupStats> = Vec::new();

    for &group_id in group_ids {
        let path = format!("/groups/{}/merge_requests", urlencoding::encode(group_id));
        let mrs: Vec<Value> = client.get(&path, &[
            ("state", "opened"),
            ("per_page", "100"),
        ]).await.unwrap_or_default();

        let mut gs = GroupStats {
            name: group_id.to_string(),
            open: mrs.len() as u32,
            drafts: 0, stale: 0, no_reviewer: 0,
            avg_age_hours: 0.0,
            reviewers: BTreeMap::new(),
        };

        let mut total_age = 0.0f64;
        for mr in &mrs {
            let created = mr["created_at"].as_str().unwrap_or("");
            let age = chrono::DateTime::parse_from_rfc3339(created)
                .ok()
                .map(|dt| (now - dt.with_timezone(&chrono::Utc)).num_hours() as f64)
                .unwrap_or(0.0);
            total_age += age;

            if mr["draft"].as_bool().unwrap_or(false) { gs.drafts += 1; }
            if age > 168.0 { gs.stale += 1; }

            let reviewers: Vec<String> = mr["reviewers"]
                .as_array()
                .map(|a| a.iter().filter_map(|v| v["username"].as_str().map(String::from)).collect())
                .unwrap_or_default();
            if reviewers.is_empty() { gs.no_reviewer += 1; }
            for r in reviewers {
                *gs.reviewers.entry(r).or_default() += 1;
            }
        }
        if gs.open > 0 { gs.avg_age_hours = total_age / gs.open as f64; }
        all_stats.push(gs);
    }

    let total_open: u32 = all_stats.iter().map(|s| s.open).sum();
    let total_stale: u32 = all_stats.iter().map(|s| s.stale).sum();
    let total_no_rev: u32 = all_stats.iter().map(|s| s.no_reviewer).sum();

    let mut lines = vec![
        format!("**Org MR Dashboard** ({} groups, {total_open} open)\n", all_stats.len()),
        format!("| Group | Open | Drafts | Stale (>7d) | No reviewer | Avg age |"),
        format!("|-------|------|--------|-------------|-------------|---------|"),
    ];

    for gs in &all_stats {
        lines.push(format!(
            "| {} | {} | {} | {} | {} | {:.0}h ({:.1}d) |",
            gs.name, gs.open, gs.drafts, gs.stale, gs.no_reviewer, gs.avg_age_hours, gs.avg_age_hours / 24.0
        ));
    }

    lines.push(format!(
        "| **Total** | **{total_open}** | | **{total_stale}** | **{total_no_rev}** | |"
    ));

    // Aggregate reviewer load
    let mut all_reviewers: BTreeMap<String, u32> = BTreeMap::new();
    for gs in &all_stats {
        for (r, c) in &gs.reviewers {
            *all_reviewers.entry(r.clone()).or_default() += c;
        }
    }
    if !all_reviewers.is_empty() {
        lines.push(String::new());
        lines.push("**Reviewer load (across all groups):**".to_string());
        let mut sorted: Vec<_> = all_reviewers.iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(a.1));
        for (r, c) in sorted {
            lines.push(format!("- @{r}: {c} MRs"));
        }
    }

    Ok(lines.join("\n"))
}
