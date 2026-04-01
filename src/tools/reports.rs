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

    let mut all_commits: Vec<(String, CommitInfo)> = Vec::new(); // (project_path, commit)
    let mut all_files: u64 = 0;

    let since_str = since.to_rfc3339();
    for (&pid, _stats) in &by_project {
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

        for commit in &user_commits {
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

fn htmlescape(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
}
