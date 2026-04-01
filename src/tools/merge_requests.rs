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
            let project = mr["references"]["full"].as_str()
                .unwrap_or("")
                .split('!')
                .next()
                .unwrap_or("?")
                .to_string();

            stats.push(MrStats { iid, title, author, hours_to_merge: hours, project });
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

    stats.sort_by(|a, b| b.hours_to_merge.partial_cmp(&a.hours_to_merge).unwrap());
    lines.push(String::new());
    lines.push("**Slowest MRs:**".to_string());
    for s in stats.iter().take(5) {
        let duration = if s.hours_to_merge > 24.0 {
            format!("{:.1}d", s.hours_to_merge / 24.0)
        } else {
            format!("{:.1}h", s.hours_to_merge)
        };
        lines.push(format!("- {}!{} {} (@{}) — {duration}", s.project, s.iid, s.title, s.author));
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
