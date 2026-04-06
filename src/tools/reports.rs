//! HTML report generation for developer daily activity.

use crate::client::GitLabClient;
use crate::error::{Error, Result};
use crate::tools::commits;
use serde_json::Value;
use std::collections::BTreeMap;

/// Generate a full HTML daily report for a developer.
pub async fn generate_dev_report(
    client: &GitLabClient,
    username: &str,
    hours: u32,
    project_filter: &str,
) -> Result<String> {
    // 1. Resolve user
    let users: Vec<Value> = client
        .get("/users", &[("username", username)])
        .await
        ?;

    let user = users.first().ok_or_else(|| Error::NotFound(format!("User @{username} not found")))?;
    let user_id = user["id"].as_u64().ok_or(Error::Other("User has no ID".into()))?;
    let display_name = user["name"].as_str().unwrap_or(username);

    // 2. Fetch events
    let since = chrono::Utc::now() - chrono::Duration::hours(hours as i64);
    let events = commits::fetch_user_events(client, user_id, since.timestamp()).await?;

    // 3. Collect project IDs and resolve names
    let mut project_ids: std::collections::BTreeSet<u64> = std::collections::BTreeSet::new();
    for e in &events {
        if let Some(pid) = e["project_id"].as_u64() {
            project_ids.insert(pid);
        }
    }
    let project_names = commits::resolve_project_names(client, &project_ids).await;

    // 4. Aggregate events by project
    struct ProjectStats {
        pushes: u64,
        commits: u64,
        merges: u64,
        mr_opened: u64,
        mr_merged: u64,
        mr_approved: u64,
    }
    let mut by_project: BTreeMap<u64, ProjectStats> = BTreeMap::new();
    let mut _total_commits: u64 = 0;
    let mut total_additions: u64 = 0;
    let mut total_deletions: u64 = 0;
    let mut _total_mr_opened: u64 = 0;
    let mut total_mr_merged: u64 = 0;

    for event in &events {
        let pid = event["project_id"].as_u64().unwrap_or(0);
        let action = event["action_name"].as_str().unwrap_or("");
        let target_type = event["target_type"].as_str().unwrap_or("");
        let stats = by_project.entry(pid).or_insert(ProjectStats {
            pushes: 0, commits: 0, merges: 0, mr_opened: 0, mr_merged: 0, mr_approved: 0,
        });

        if action == "pushed to" || action == "pushed new" {
            let raw = event["push_data"]["commit_count"].as_u64().unwrap_or(1);
            let is_merge = raw > 20;
            stats.pushes += 1;
            stats.commits += if is_merge { 1 } else { raw };
            if is_merge { stats.merges += 1; }
            _total_commits += if is_merge { 1 } else { raw };
        } else if target_type == "MergeRequest" {
            match action {
                "opened" => { stats.mr_opened += 1; _total_mr_opened += 1; }
                "accepted" => { stats.mr_merged += 1; total_mr_merged += 1; }
                "approved" => { stats.mr_approved += 1; }
                _ => {}
            }
        }
    }

    // 5. For each active project, fetch recent commits by this author and their diffs
    #[allow(dead_code)]
    struct CommitInfo {
        sha: String,
        short_sha: String,
        title: String,
        time: String,
        files: Vec<FileInfo>,
        additions: u64,
        deletions: u64,
    }
    struct FileInfo {
        path: String,
        additions: u64,
        deletions: u64,
        is_new: bool,
        lang: String,
    }

    const MAX_COMMITS: usize = 50;
    const MAX_PROJECTS: usize = 10;

    let mut all_commits: Vec<(String, CommitInfo)> = Vec::new(); // (project_path, commit)
    let mut all_files: u64 = 0;
    let mut projects_processed: usize = 0;

    let since_str = since.to_rfc3339();
    for (&pid, _stats) in &by_project {
        if projects_processed >= MAX_PROJECTS {
            break;
        }
        let proj_path = project_names.get(&pid).cloned().unwrap_or_else(|| pid.to_string());

        if !project_filter.is_empty() && !proj_path.contains(project_filter) {
            continue;
        }

        // Fetch commits
        let encoded = urlencoding::encode(&proj_path);
        let commits_data: Vec<Value> = client
            .get(
                &format!("/projects/{encoded}/repository/commits"),
                &[("since", since_str.as_str()), ("per_page", "20")],
            )
            .await
            .unwrap_or_default();

        let author_lower = display_name.to_lowercase();
        let user_commits: Vec<&Value> = commits_data.iter().filter(|c| {
            let name = c["author_name"].as_str().unwrap_or("").to_lowercase();
            let email = c["author_email"].as_str().unwrap_or("").to_lowercase();
            name.contains(&username.to_lowercase()) || name.contains(&author_lower)
                || email.contains(&username.to_lowercase())
        }).collect();

        projects_processed += 1;

        for commit in &user_commits {
            if all_commits.len() >= MAX_COMMITS {
                break;
            }
            let sha = commit["id"].as_str().unwrap_or("").to_string();
            let short_sha = commit["short_id"].as_str().unwrap_or("?").to_string();
            let title = commit["title"].as_str().unwrap_or("?").to_string();
            let time = commit["created_at"].as_str().unwrap_or("?").to_string();
            let time_short = if time.len() > 16 {
                time[11..16].to_string()
            } else {
                time.clone()
            };

            // Fetch diff
            let diffs: Vec<Value> = client
                .get(
                    &format!("/projects/{encoded}/repository/commits/{sha}/diff"),
                    &[],
                )
                .await
                .unwrap_or_default();

            let mut files = Vec::new();
            let mut c_add: u64 = 0;
            let mut c_del: u64 = 0;

            for diff in &diffs {
                let path = diff["new_path"].as_str().unwrap_or("?").to_string();
                let diff_text = diff["diff"].as_str().unwrap_or("");
                let is_new = diff["new_file"].as_bool().unwrap_or(false);
                let lang = commits::detect_language(&path).to_string();

                let mut add: u64 = 0;
                let mut del: u64 = 0;
                for line in diff_text.lines() {
                    if line.starts_with('+') && !line.starts_with("+++") { add += 1; }
                    if line.starts_with('-') && !line.starts_with("---") { del += 1; }
                }
                c_add += add;
                c_del += del;
                all_files += 1;

                files.push(FileInfo { path, additions: add, deletions: del, is_new, lang });
            }

            total_additions += c_add;
            total_deletions += c_del;

            all_commits.push((proj_path.clone(), CommitInfo {
                sha, short_sha, title, time: time_short, files,
                additions: c_add, deletions: c_del,
            }));
        }
    }

    // 6. Fetch open MRs by author
    let mrs: Vec<Value> = client
        .get("/merge_requests", &[("author_username", username), ("state", "opened"), ("per_page", "50"), ("scope", "all")])
        .await
        .unwrap_or_default();

    // Group MRs by target branch
    let mut mrs_by_target: BTreeMap<String, Vec<&Value>> = BTreeMap::new();
    let mut draft_mrs: Vec<&Value> = Vec::new();
    for mr in &mrs {
        if mr["draft"].as_bool().unwrap_or(false) {
            draft_mrs.push(mr);
        } else {
            let target = mr["target_branch"].as_str().unwrap_or("?").to_string();
            mrs_by_target.entry(target).or_default().push(mr);
        }
    }

    // 7. Build HTML
    let date_str = chrono::Utc::now().format("%A, %d %B %Y").to_string();
    let period_label = if hours <= 24 { "Today".to_string() } else { format!("Last {}h", hours) };

    let mut html = format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{display_name} — {date_str}</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',sans-serif;background:#0f0f13;color:#e0e0e0;line-height:1.6}}
.c{{max-width:860px;margin:0 auto;padding:20px}}
.hdr{{background:linear-gradient(135deg,#1a1a2e 0%,#16213e 50%,#0f3460 100%);padding:28px;border-radius:14px;margin-bottom:18px;border:1px solid #1e2a3a}}
.hdr h1{{font-size:22px;color:#fff}} .hdr .sub{{color:#7a8ba5;font-size:12px;margin-top:2px}}
.stats{{display:flex;gap:14px;margin-top:16px;flex-wrap:wrap}}
.stat{{background:rgba(255,255,255,.04);border:1px solid #1e2a3a;border-radius:8px;padding:10px 14px;min-width:80px;text-align:center}}
.stat b{{font-size:20px;display:block;color:#fff}} .stat small{{font-size:9px;text-transform:uppercase;color:#5a6a7a;letter-spacing:.5px}}
.card{{background:#161620;border:1px solid #1e2030;border-radius:10px;padding:20px;margin-bottom:14px}}
.card h2{{font-size:13px;color:#8892a5;text-transform:uppercase;letter-spacing:.8px;border-bottom:1px solid #1e2030;padding-bottom:8px;margin-bottom:14px}}
.commit{{margin-bottom:16px;padding-bottom:14px;border-bottom:1px solid #1a1a2a}}.commit:last-child{{border:none;margin-bottom:0;padding-bottom:0}}
.cm-head{{display:flex;align-items:center;gap:8px;flex-wrap:wrap}}
.sha{{font-family:monospace;background:#1a1a2e;border:1px solid #252540;padding:2px 7px;border-radius:4px;font-size:11px;color:#64b5f6}}
.cm-msg{{font-weight:600;color:#e0e0e0;font-size:13px}}.cm-time{{font-size:11px;color:#555;margin-left:auto}}
.badge{{display:inline-block;padding:1px 7px;border-radius:3px;font-size:10px;font-weight:600}}
.b-lang{{background:#1a1a3e;color:#7c8cf5;border:1px solid #2a2a50}}
.b-new{{background:#0a2a1a;color:#4caf50;border:1px solid #1a4a2a}}
.b-open{{background:#0a2a1a;color:#4caf50;border:1px solid #1a4a2a}}
.b-draft{{background:#2a1a00;color:#ff9800;border:1px solid #4a3000}}
.b-rc{{background:#1a0a2a;color:#ab47bc;border:1px solid #2a1a4a}}
.file{{display:flex;align-items:center;gap:8px;padding:3px 0;font-size:12px}}
.fp{{font-family:monospace;color:#9aa0b0;font-size:11px}}.add{{color:#4caf50}}.del{{color:#ef5350}}
.fs{{font-family:monospace;font-size:11px}}
.proj-tag{{font-family:monospace;font-size:10px;color:#64b5f6;background:#0d1b2a;border:1px solid #1e2a3a;padding:1px 6px;border-radius:4px}}
table{{width:100%;border-collapse:collapse;font-size:12px}}
th{{text-align:left;padding:6px 10px;color:#5a6a7a;font-size:10px;text-transform:uppercase;letter-spacing:.5px;border-bottom:1px solid #1e2030}}
td{{padding:6px 10px;border-bottom:1px solid #151520}}
.alert{{background:#1a0a0a;border:1px solid #3a1a1a;border-radius:8px;padding:12px 16px;margin-top:12px;font-size:12px;color:#ef9a9a}}.alert b{{color:#ef5350}}
.foot{{text-align:center;padding:24px;color:#3a3a4a;font-size:10px}}
.foot a{{color:#4a4a6a;text-decoration:none}}
.grp-title{{font-size:11px;color:#5a6a7a;text-transform:uppercase;letter-spacing:.5px;padding:8px 0 4px}}
</style>
</head>
<body>
<div class="c">

<div class="hdr">
  <h1>{display_name}</h1>
  <div class="sub">@{username} &middot; {period_label} &middot; {date_str}</div>
  <div class="stats">
    <div class="stat"><b>{}</b><small>Commits</small></div>
    <div class="stat"><b>+{} / -{}</b><small>Lines</small></div>
    <div class="stat"><b>{}</b><small>Files</small></div>
    <div class="stat"><b>{}</b><small>Open MRs</small></div>
    <div class="stat"><b>{}</b><small>Merged</small></div>
    <div class="stat"><b>{}</b><small>Projects</small></div>
  </div>
</div>
"#,
        all_commits.len(), total_additions, total_deletions, all_files,
        mrs.len(), total_mr_merged, project_ids.len()
    );

    // Commits card
    if !all_commits.is_empty() {
        html.push_str("<div class=\"card\">\n  <h2>Commits</h2>\n");
        for (proj_path, c) in &all_commits {
            let short_proj = proj_path.rsplit('/').take(2).collect::<Vec<_>>().into_iter().rev().collect::<Vec<_>>().join("/");
            html.push_str(&format!(
                r#"  <div class="commit">
    <div class="cm-head">
      <span class="sha">{}</span>
      <span class="cm-msg">{}</span>
      <span class="proj-tag">{}</span>
      <span class="cm-time">{}</span>
    </div>
    <div style="margin:6px 0">
"#,
                c.short_sha,
                htmlescape(&c.title),
                short_proj,
                c.time,
            ));
            for f in &c.files {
                let new_badge = if f.is_new { r#" <span class="badge b-new">NEW</span>"# } else { "" };
                html.push_str(&format!(
                    r#"      <div class="file"><span class="fp">{}</span><span class="fs"><span class="add">+{}</span> <span class="del">-{}</span></span><span class="badge b-lang">{}</span>{}</div>
"#,
                    htmlescape(&f.path), f.additions, f.deletions, f.lang, new_badge
                ));
            }
            html.push_str("    </div>\n  </div>\n");
        }
        html.push_str("</div>\n");
    }

    // MRs card
    if !mrs.is_empty() {
        html.push_str(&format!("<div class=\"card\">\n  <h2>Open Merge Requests &middot; {}</h2>\n", mrs.len()));

        for (target, target_mrs) in &mrs_by_target {
            let badge_class = if target.contains("RC") || target.contains("rc") { "b-rc" } else { "b-open" };
            html.push_str(&format!("  <div class=\"grp-title\">{} ({})</div>\n  <table>\n", target, target_mrs.len()));
            for mr in target_mrs {
                let iid = mr["iid"].as_u64().unwrap_or(0);
                let title = mr["title"].as_str().unwrap_or("?");
                html.push_str(&format!(
                    "    <tr><td style=\"width:60px\">!{}</td><td>{}</td><td style=\"width:60px\"><span class=\"badge {}\">{}</span></td></tr>\n",
                    iid, htmlescape(title), badge_class, target
                ));
            }
            html.push_str("  </table>\n");
        }

        if !draft_mrs.is_empty() {
            html.push_str(&format!("  <div class=\"grp-title\">Drafts ({})</div>\n  <table>\n", draft_mrs.len()));
            for mr in &draft_mrs {
                let iid = mr["iid"].as_u64().unwrap_or(0);
                let title = mr["title"].as_str().unwrap_or("?");
                html.push_str(&format!(
                    "    <tr><td style=\"width:60px\">!{}</td><td>{}</td><td style=\"width:60px\"><span class=\"badge b-draft\">Draft</span></td></tr>\n",
                    iid, htmlescape(title)
                ));
            }
            html.push_str("  </table>\n");
        }

        if mrs.len() > 5 {
            html.push_str(&format!(
                "  <div class=\"alert\"><b>{} open MRs, {} merged.</b> Review bottleneck &mdash; consider assigning reviewers.</div>\n",
                mrs.len(), total_mr_merged
            ));
        }

        html.push_str("</div>\n");
    }

    // Footer
    html.push_str(&format!(
        r#"<div class="foot">made with &lt;3 by Alex Velesnitski &middot; gl-mcp + Claude &middot; {date_str}</div>

</div>
</body>
</html>"#
    ));

    Ok(html)
}

/// Generate a complete HTML team performance report for a project.
pub async fn generate_team_report(
    client: &GitLabClient,
    project_id: &str,
    usernames: &[&str],
    days: u32,
) -> Result<String> {
    use futures::future::join_all;

    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let encoded_project = urlencoding::encode(project_id);
    let mr_path = format!("/projects/{encoded_project}/merge_requests");

    // ── Per-developer stats (same structure as compare_developers) ──

    struct DevStats {
        username: String,
        mrs_merged: u64,
        mrs_reviewed: u64,
        reviewed_authors: BTreeMap<String, u64>,
        approvals_given: u64,
        avg_merge_hours: f64,
        mr_comments: u64,
        commits: u64,
        additions: u64,
        deletions: u64,
        files_changed: u64,
        mr_sizes: (u64, u64, u64),
    }

    let dev_futures: Vec<_> = usernames.iter().map(|&username| {
        let client = client.clone();
        let since = since.clone();
        let mr_path = mr_path.clone();
        let encoded_project = encoded_project.clone();
        async move {
            // Merged MRs by this author
            let merged_mrs: Vec<Value> = client.get(&mr_path, &[
                ("author_username", username),
                ("state", "merged"),
                ("created_after", &since),
                ("per_page", "100"),
            ]).await.unwrap_or_default();

            // MRs where this user is reviewer (merged)
            let reviewed_mrs: Vec<Value> = client.get(&mr_path, &[
                ("reviewer_username", username),
                ("state", "merged"),
                ("created_after", &since),
                ("per_page", "100"),
            ]).await.unwrap_or_default();

            // Calc merge time + LOC/files from merged MRs
            let mut merge_hours: Vec<f64> = Vec::new();
            let mut total_additions: u64 = 0;
            let mut total_deletions: u64 = 0;
            let mut total_files: u64 = 0;
            let mut mr_small: u64 = 0;
            let mut mr_medium: u64 = 0;
            let mut mr_large: u64 = 0;

            for mr in &merged_mrs {
                let created = mr["created_at"].as_str().unwrap_or("");
                let merged = mr["merged_at"].as_str().unwrap_or("");
                let created_dt = chrono::DateTime::parse_from_rfc3339(created).ok();
                let merged_dt = chrono::DateTime::parse_from_rfc3339(merged).ok();
                if let (Some(c), Some(m)) = (created_dt, merged_dt) {
                    merge_hours.push((m - c).num_minutes() as f64 / 60.0);
                }

                let mr_iid = mr["iid"].as_u64().unwrap_or(0);
                if mr_iid > 0 {
                    let changes: std::result::Result<Value, _> = client
                        .get(
                            &format!("/projects/{encoded_project}/merge_requests/{mr_iid}/changes"),
                            &[("access_raw_diffs", "true")],
                        )
                        .await;
                    if let Ok(detail) = changes {
                        if let Some(files) = detail["changes"].as_array() {
                            let file_count = files.len() as u64;
                            total_files += file_count;
                            match file_count {
                                0..=9 => mr_small += 1,
                                10..=50 => mr_medium += 1,
                                _ => mr_large += 1,
                            }
                            for f in files {
                                let diff = f["diff"].as_str().unwrap_or("");
                                for line in diff.lines() {
                                    if line.starts_with('+') && !line.starts_with("+++") {
                                        total_additions += 1;
                                    } else if line.starts_with('-') && !line.starts_with("---") {
                                        total_deletions += 1;
                                    }
                                }
                            }
                        }
                    }
                }
            }

            let avg_merge = if merge_hours.is_empty() {
                0.0
            } else {
                merge_hours.iter().sum::<f64>() / merge_hours.len() as f64
            };

            // Review matrix: who did this user review
            let mut reviewed_authors: BTreeMap<String, u64> = BTreeMap::new();
            for mr in &reviewed_mrs {
                let author = mr["author"]["username"].as_str().unwrap_or("?").to_string();
                *reviewed_authors.entry(author).or_insert(0) += 1;
            }

            // Events for approvals, comments, commits
            let cache_key = format!("user:{username}");
            let users: Vec<Value> = client
                .get_cached(&cache_key, "/users", &[("username", username)], 60)
                .await
                .unwrap_or_default();

            let user_id = users.first().and_then(|u| u["id"].as_u64()).unwrap_or(0);
            let since_ts = (chrono::Utc::now() - chrono::Duration::days(days as i64)).timestamp();
            let events = if user_id > 0 {
                commits::fetch_user_events(&client, user_id, since_ts).await.unwrap_or_default()
            } else {
                Vec::new()
            };

            let project_info: Option<Value> = client
                .get_cached(
                    &format!("project_info:{encoded_project}"),
                    &format!("/projects/{encoded_project}"),
                    &[("simple", "true")],
                    60,
                )
                .await
                .ok();
            let project_numeric_id = project_info.as_ref().and_then(|p| p["id"].as_u64());

            let mut approvals = 0u64;
            let mut comments = 0u64;
            let mut dev_commits = 0u64;

            for event in &events {
                let event_pid = event["project_id"].as_u64();
                if project_numeric_id.is_some() && event_pid != project_numeric_id {
                    continue;
                }
                let action = event["action_name"].as_str().unwrap_or("");
                let target_type = event["target_type"].as_str().unwrap_or("");
                match (action, target_type) {
                    ("approved", "MergeRequest") => approvals += 1,
                    ("commented on", "MergeRequest") => comments += 1,
                    ("pushed to", _) | ("pushed new", _) => {
                        let raw = event["push_data"]["commit_count"].as_u64().unwrap_or(1);
                        dev_commits += if raw > 20 { 1 } else { raw };
                    }
                    _ => {}
                }
            }

            DevStats {
                username: username.to_string(),
                mrs_merged: merged_mrs.len() as u64,
                mrs_reviewed: reviewed_mrs.len() as u64,
                reviewed_authors,
                approvals_given: approvals,
                avg_merge_hours: avg_merge,
                mr_comments: comments,
                commits: dev_commits,
                additions: total_additions,
                deletions: total_deletions,
                files_changed: total_files,
                mr_sizes: (mr_small, mr_medium, mr_large),
            }
        }
    }).collect();

    let dev_results = join_all(dev_futures).await;

    // ── MR turnaround data ──

    let turnaround_mrs: Vec<Value> = client.get(&mr_path, &[
        ("state", "merged"),
        ("created_after", &since),
        ("per_page", "50"),
        ("order_by", "updated_at"),
        ("sort", "desc"),
    ]).await.unwrap_or_default();

    struct TurnaroundMr {
        iid: u64,
        title: String,
        author: String,
        hours: f64,
    }
    let mut turnaround_stats: Vec<TurnaroundMr> = Vec::new();
    for mr in &turnaround_mrs {
        let created = mr["created_at"].as_str().unwrap_or("");
        let merged = mr["merged_at"].as_str().unwrap_or("");
        if let (Some(c), Some(m)) = (
            chrono::DateTime::parse_from_rfc3339(created).ok(),
            chrono::DateTime::parse_from_rfc3339(merged).ok(),
        ) {
            turnaround_stats.push(TurnaroundMr {
                iid: mr["iid"].as_u64().unwrap_or(0),
                title: mr["title"].as_str().unwrap_or("?").to_string(),
                author: mr["author"]["username"].as_str().unwrap_or("?").to_string(),
                hours: (m - c).num_minutes() as f64 / 60.0,
            });
        }
    }

    // ── Resolve project display name ──

    let project_name = client
        .get_cached::<Value>(
            &format!("project_info:{encoded_project}"),
            &format!("/projects/{encoded_project}"),
            &[("simple", "true")],
            60,
        )
        .await
        .ok()
        .and_then(|p| p["path_with_namespace"].as_str().map(|s| s.to_string()))
        .unwrap_or_else(|| project_id.to_string());

    // ── Compute summary metrics ──

    let total_mrs_merged: u64 = dev_results.iter().map(|d| d.mrs_merged).sum();
    let total_loc: u64 = dev_results.iter().map(|d| d.additions + d.deletions).sum();
    let reviewers_active = dev_results.iter().filter(|d| d.mrs_reviewed > 0).count();
    let inactive_count = dev_results.iter().filter(|d| d.commits == 0 && d.mrs_merged == 0 && d.mrs_reviewed == 0).count();
    let date_str = chrono::Utc::now().format("%A, %d %B %Y").to_string();

    // Review bus factor
    let bus_factor = if reviewers_active == 0 { 0 } else { reviewers_active };

    // ── Build HTML ──

    let mut html = format!(r#"<!DOCTYPE html>
<html lang="en">
<head>
<meta charset="UTF-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Team Report — {project_name} — {date_str}</title>
<style>
*{{margin:0;padding:0;box-sizing:border-box}}
body{{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#0d1117;color:#c9d1d9;padding:32px;line-height:1.6}}
h1{{color:#58a6ff;margin-bottom:8px;font-size:24px}}
h2{{color:#58a6ff;margin:36px 0 16px;font-size:18px;border-bottom:1px solid #21262d;padding-bottom:8px}}
.sub{{color:#8b949e;margin-bottom:24px;font-size:14px}}
.grid{{display:grid;grid-template-columns:repeat(auto-fit,minmax(180px,1fr));gap:14px;margin:16px 0}}
.card{{background:#161b22;border:1px solid #21262d;border-radius:8px;padding:18px}}
.card-t{{color:#8b949e;font-size:11px;text-transform:uppercase;letter-spacing:1px;margin-bottom:6px}}
.card-v{{font-size:28px;font-weight:700}}
.card-s{{color:#8b949e;font-size:12px;margin-top:4px}}
.g{{color:#3fb950}}.r{{color:#f85149}}.y{{color:#d29922}}.b{{color:#58a6ff}}
table{{width:100%;border-collapse:collapse;margin:12px 0}}
th{{background:#161b22;color:#8b949e;text-align:left;padding:10px 14px;font-size:11px;text-transform:uppercase;letter-spacing:.5px;border-bottom:2px solid #21262d}}
td{{padding:10px 14px;border-bottom:1px solid #21262d;font-size:14px}}
.issue{{background:#161b22;border:1px solid #21262d;border-radius:6px;padding:14px 18px;margin:8px 0}}
.issue b{{font-weight:600}}.issue .m{{color:#8b949e;font-size:13px;margin-top:4px}}
.risk{{border-left:3px solid #f85149}}.warn{{border-left:3px solid #d29922}}
footer{{margin-top:48px;padding-top:16px;border-top:1px solid #21262d;color:#484f58;font-size:12px}}
</style>
</head>
<body>

<h1>Team Performance Report</h1>
<div class="sub">{project_name} &middot; Last {days} days &middot; {date_str}</div>

<!-- Summary Cards -->
<div class="grid">
  <div class="card"><div class="card-t">Developers Active</div><div class="card-v b">{}</div><div class="card-s">{} total, {} inactive</div></div>
  <div class="card"><div class="card-t">MRs Merged</div><div class="card-v g">{total_mrs_merged}</div></div>
  <div class="card"><div class="card-t">Total LOC</div><div class="card-v">{total_loc}</div><div class="card-s">additions + deletions</div></div>
  <div class="card"><div class="card-t">Review Bus Factor</div><div class="card-v{}">{bus_factor}</div><div class="card-s">devs with reviews &gt; 0</div></div>
</div>
"#,
        dev_results.len() - inactive_count,
        dev_results.len(),
        inactive_count,
        if bus_factor <= 1 { " r" } else { "" },
    );

    // ── Developer Comparison Table ──

    html.push_str("<h2>Developer Comparison</h2>\n<table>\n<tr><th>Developer</th><th>Commits</th><th>LOC +</th><th>LOC &minus;</th><th>Files</th><th>MRs Merged</th><th>Reviews</th><th>Approvals</th><th>Avg Merge</th><th>Comments</th></tr>\n");

    for d in &dev_results {
        let merge_time = if d.mrs_merged == 0 {
            "&ndash;".to_string()
        } else {
            format!("{:.1}h", d.avg_merge_hours)
        };
        html.push_str(&format!(
            "<tr><td><b>@{}</b></td><td>{}</td><td class=\"g\">+{}</td><td class=\"r\">-{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
            htmlescape(&d.username),
            d.commits,
            d.additions,
            d.deletions,
            d.files_changed,
            d.mrs_merged,
            d.mrs_reviewed,
            d.approvals_given,
            merge_time,
            d.mr_comments,
        ));
    }
    html.push_str("</table>\n");

    // ── Review Matrix ──

    let has_reviews = dev_results.iter().any(|d| !d.reviewed_authors.is_empty());
    if has_reviews {
        html.push_str("<h2>Review Matrix</h2>\n<p class=\"sub\">Who reviewed whose MRs (count)</p>\n<table>\n<tr><th>Reviewer \\ Author</th>");
        for d in &dev_results {
            html.push_str(&format!("<th>@{}</th>", htmlescape(&d.username)));
        }
        html.push_str("</tr>\n");

        for reviewer in &dev_results {
            html.push_str(&format!("<tr><td><b>@{}</b></td>", htmlescape(&reviewer.username)));
            for author in &dev_results {
                let count = reviewer.reviewed_authors.get(&author.username).unwrap_or(&0);
                let cell = if *count == 0 { "&ndash;".to_string() } else { format!("<b>{count}</b>") };
                html.push_str(&format!("<td>{cell}</td>"));
            }
            html.push_str("</tr>\n");
        }
        html.push_str("</table>\n");
    }

    // ── MR Size Distribution ──

    let total_small: u64 = dev_results.iter().map(|d| d.mr_sizes.0).sum();
    let total_medium: u64 = dev_results.iter().map(|d| d.mr_sizes.1).sum();
    let total_large: u64 = dev_results.iter().map(|d| d.mr_sizes.2).sum();
    let total_sized = total_small + total_medium + total_large;

    html.push_str("<h2>MR Size Distribution</h2>\n");
    if total_sized > 0 {
        html.push_str("<div class=\"grid\">\n");
        html.push_str(&format!(
            "  <div class=\"card\"><div class=\"card-t\">Small (&lt;10 files)</div><div class=\"card-v g\">{total_small}</div><div class=\"card-s\">{:.0}%</div></div>\n",
            total_small as f64 / total_sized as f64 * 100.0
        ));
        html.push_str(&format!(
            "  <div class=\"card\"><div class=\"card-t\">Medium (10–50 files)</div><div class=\"card-v y\">{total_medium}</div><div class=\"card-s\">{:.0}%</div></div>\n",
            total_medium as f64 / total_sized as f64 * 100.0
        ));
        html.push_str(&format!(
            "  <div class=\"card\"><div class=\"card-t\">Large (&gt;50 files)</div><div class=\"card-v r\">{total_large}</div><div class=\"card-s\">{:.0}%</div></div>\n",
            total_large as f64 / total_sized as f64 * 100.0
        ));
        html.push_str("</div>\n");

        // Per-developer breakdown
        html.push_str("<table>\n<tr><th>Developer</th><th>Small</th><th>Medium</th><th>Large</th></tr>\n");
        for d in &dev_results {
            html.push_str(&format!(
                "<tr><td>@{}</td><td class=\"g\">{}</td><td class=\"y\">{}</td><td class=\"r\">{}</td></tr>\n",
                htmlescape(&d.username), d.mr_sizes.0, d.mr_sizes.1, d.mr_sizes.2,
            ));
        }
        html.push_str("</table>\n");
    } else {
        html.push_str("<p class=\"sub\">No merged MRs with size data in this period.</p>\n");
    }

    // ── MR Turnaround ──

    html.push_str("<h2>MR Turnaround</h2>\n");
    if !turnaround_stats.is_empty() {
        let total_hours: f64 = turnaround_stats.iter().map(|t| t.hours).sum();
        let avg_hours = total_hours / turnaround_stats.len() as f64;
        let median_hours = {
            let mut sorted: Vec<f64> = turnaround_stats.iter().map(|t| t.hours).collect();
            sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            sorted[sorted.len() / 2]
        };

        html.push_str("<div class=\"grid\">\n");
        html.push_str(&format!(
            "  <div class=\"card\"><div class=\"card-t\">Average</div><div class=\"card-v\">{:.1}h</div></div>\n",
            avg_hours
        ));
        html.push_str(&format!(
            "  <div class=\"card\"><div class=\"card-t\">Median</div><div class=\"card-v\">{:.1}h</div></div>\n",
            median_hours
        ));
        html.push_str(&format!(
            "  <div class=\"card\"><div class=\"card-t\">MRs Analyzed</div><div class=\"card-v\">{}</div></div>\n",
            turnaround_stats.len()
        ));
        html.push_str("</div>\n");

        // Slowest MRs
        let mut sorted_ta = turnaround_stats;
        sorted_ta.sort_by(|a, b| b.hours.partial_cmp(&a.hours).unwrap_or(std::cmp::Ordering::Equal));
        html.push_str("<h2>Slowest MRs</h2>\n<table>\n<tr><th>MR</th><th>Title</th><th>Author</th><th>Time to Merge</th></tr>\n");
        for t in sorted_ta.iter().take(5) {
            let duration = if t.hours > 24.0 {
                format!("{:.1}d", t.hours / 24.0)
            } else {
                format!("{:.1}h", t.hours)
            };
            html.push_str(&format!(
                "<tr><td>!{}</td><td>{}</td><td>@{}</td><td class=\"{}\">{}</td></tr>\n",
                t.iid,
                htmlescape(&t.title),
                htmlescape(&t.author),
                if t.hours > 48.0 { "r" } else if t.hours > 24.0 { "y" } else { "" },
                duration,
            ));
        }
        html.push_str("</table>\n");
    } else {
        html.push_str("<p class=\"sub\">No merged MRs with turnaround data in this period.</p>\n");
    }

    // ── Code Quality Placeholder ──

    html.push_str("<h2>Code Quality</h2>\n");
    html.push_str("<div class=\"issue warn\"><b>Not yet analyzed.</b><div class=\"m\">Use <code>validate_mr_changes</code> on individual MRs to check for code quality issues (large files, missing tests, debug statements).</div></div>\n");

    // ── Process Issues (auto-detected) ──

    html.push_str("<h2>Process Issues</h2>\n");

    let mut issues_found = 0;

    // Bus factor = 1
    if bus_factor <= 1 {
        html.push_str("<div class=\"issue risk\"><b>Review bus factor = 1</b><div class=\"m\">Only 1 (or 0) developer is actively reviewing MRs. Knowledge is concentrated in a single person.</div></div>\n");
        issues_found += 1;
    }

    // Zero review participation
    for d in &dev_results {
        if d.mrs_reviewed == 0 && d.commits > 10 {
            html.push_str(&format!(
                "<div class=\"issue risk\"><b>@{} — no review participation</b><div class=\"m\">{} commits but 0 reviews given. Consider requiring cross-reviews.</div></div>\n",
                htmlescape(&d.username), d.commits,
            ));
            issues_found += 1;
        }
    }

    // Large MRs
    for d in &dev_results {
        if d.mrs_merged > 0 {
            let avg_files_per_mr = d.files_changed as f64 / d.mrs_merged as f64;
            if avg_files_per_mr > 50.0 {
                html.push_str(&format!(
                    "<div class=\"issue warn\"><b>@{} — MRs too large</b><div class=\"m\">Average {:.0} files/MR. Break down into smaller, reviewable chunks.</div></div>\n",
                    htmlescape(&d.username), avg_files_per_mr,
                ));
                issues_found += 1;
            }
        }
    }

    // Zero MR comments across all
    let total_comments: u64 = dev_results.iter().map(|d| d.mr_comments).sum();
    if total_comments == 0 && total_mrs_merged > 0 {
        html.push_str("<div class=\"issue warn\"><b>Zero MR comments</b><div class=\"m\">No one left comments on merge requests in this period. Reviews may be rubber-stamped.</div></div>\n");
        issues_found += 1;
    }

    // Inactive members
    if inactive_count > 0 {
        let inactive_names: Vec<&str> = dev_results.iter()
            .filter(|d| d.commits == 0 && d.mrs_merged == 0 && d.mrs_reviewed == 0)
            .map(|d| d.username.as_str())
            .collect();
        html.push_str(&format!(
            "<div class=\"issue warn\"><b>{} inactive member(s)</b><div class=\"m\">No commits, MRs, or reviews: {}. May be on leave or assigned to other projects.</div></div>\n",
            inactive_count,
            inactive_names.iter().map(|n| format!("@{n}")).collect::<Vec<_>>().join(", "),
        ));
        issues_found += 1;
    }

    if issues_found == 0 {
        html.push_str("<div class=\"issue\" style=\"border-left:3px solid #3fb950\"><b>No issues detected</b><div class=\"m\">Team processes look healthy for this period.</div></div>\n");
    }

    // ── Footer ──

    html.push_str(&format!(
        r#"
<footer>made with &lt;3 by Alex Velesnitski &middot; gl-mcp + Claude &middot; {date_str}</footer>

</body>
</html>"#
    ));

    Ok(html)
}

fn htmlescape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}
