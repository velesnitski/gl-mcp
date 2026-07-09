//! Documentation-quality audit tools.
//!
//! `audit_readmes` scans a group (with subgroups) and classifies each project's
//! README as missing / small / non-English (Cyrillic) / ok. All fetching and
//! classification happens here, so the caller gets a compact table instead of
//! N READMEs — the same server-side-scan shape as `get_ai_adoption`.

use crate::client::GitLabClient;
use crate::error::Result;
use serde_json::Value;

/// How concurrently we fetch per-repo READMEs. Bounded to be polite to the API
/// (the client retries 429s, but we'd rather not provoke them at 150 repos).
const CONCURRENCY: usize = 12;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Missing,
    Cyrillic,
    Small,
    Ok,
}

struct RepoReadme {
    path: String,
    web_url: String,
    default_branch: String,
    verdict: Verdict,
    size: usize,
    cyr_pct: u8,
    fname: String,
}

/// Share of alphabetic characters that are Cyrillic (0–100). Used as a
/// language proxy — a README that is mostly Russian scores high.
fn cyrillic_pct(text: &str) -> u8 {
    let (mut cyr, mut lat) = (0usize, 0usize);
    for c in text.chars() {
        if ('\u{0400}'..='\u{04FF}').contains(&c) {
            cyr += 1;
        } else if c.is_ascii_alphabetic() {
            lat += 1;
        }
    }
    let total = cyr + lat;
    if total == 0 {
        0
    } else {
        (cyr * 100 / total) as u8
    }
}

/// Classify one project's README. Two calls at most: list the root tree to find
/// a README* file (any variant/case), then fetch its raw bytes.
async fn classify_repo(
    client: &GitLabClient,
    project: &Value,
    small_bytes: usize,
    cyr_threshold: u8,
) -> RepoReadme {
    let id = project["id"].as_u64().unwrap_or(0);
    let path = project["path_with_namespace"].as_str().unwrap_or("?").to_string();
    let web_url = project["web_url"].as_str().unwrap_or("").to_string();
    let branch = project["default_branch"].as_str().unwrap_or("").to_string();
    let ref_name = if branch.is_empty() { "HEAD" } else { branch.as_str() };

    let mk = |verdict, size, cyr_pct, fname: &str| RepoReadme {
        path: path.clone(),
        web_url: web_url.clone(),
        default_branch: branch.clone(),
        verdict,
        size,
        cyr_pct,
        fname: fname.to_string(),
    };

    // Empty repo (no default branch) → treat as missing README.
    if branch.is_empty() {
        return mk(Verdict::Missing, 0, 0, "-");
    }

    let tree: Vec<Value> = client
        .get(
            &format!("/projects/{id}/repository/tree"),
            &[("ref", ref_name), ("per_page", "100")],
        )
        .await
        .unwrap_or_default();

    let readme = tree.iter().find(|e| {
        e["type"].as_str() == Some("blob")
            && e["name"]
                .as_str()
                .map(|n| n.to_ascii_lowercase().starts_with("readme"))
                .unwrap_or(false)
    });

    let Some(readme) = readme else {
        return mk(Verdict::Missing, 0, 0, "-");
    };
    let fname = readme["name"].as_str().unwrap_or("README").to_string();

    let content = client
        .get_text(
            &format!(
                "/projects/{id}/repository/files/{}/raw",
                urlencoding::encode(&fname)
            ),
            &[("ref", ref_name)],
        )
        .await
        .unwrap_or_default();

    let size = content.len();
    let cyr = cyrillic_pct(&content);

    // Priority: missing > cyrillic (the flagged language issue, even if short) >
    // small stub > ok. A large mostly-Russian README is Cyrillic, not Ok.
    let verdict = if size == 0 {
        Verdict::Missing
    } else if cyr >= cyr_threshold {
        Verdict::Cyrillic
    } else if size < small_bytes {
        Verdict::Small
    } else {
        Verdict::Ok
    };
    mk(verdict, size, cyr, &fname)
}

/// Audit README coverage/quality across a group (including subgroups).
pub async fn audit_readmes(
    client: &GitLabClient,
    group_path: &str,
    small_bytes: usize,
    cyr_threshold: u8,
    include_ok: bool,
) -> Result<String> {
    let encoded = urlencoding::encode(group_path);
    let projects: Vec<Value> = client
        .get_all_pages(
            &format!("/groups/{encoded}/projects"),
            &[
                ("include_subgroups", "true"),
                ("archived", "false"),
                ("order_by", "path"),
                ("sort", "asc"),
            ],
            20,
        )
        .await?;

    if projects.is_empty() {
        return Ok(format!("No (non-archived) projects found in group `{group_path}`."));
    }

    // Fetch in bounded-concurrency chunks.
    let mut results: Vec<RepoReadme> = Vec::with_capacity(projects.len());
    for chunk in projects.chunks(CONCURRENCY) {
        let futs = chunk
            .iter()
            .map(|p| classify_repo(client, p, small_bytes, cyr_threshold));
        results.extend(futures::future::join_all(futs).await);
    }

    let count = |v: Verdict| results.iter().filter(|r| r.verdict == v).count();
    let (missing, cyrillic, small, ok) = (
        count(Verdict::Missing),
        count(Verdict::Cyrillic),
        count(Verdict::Small),
        count(Verdict::Ok),
    );

    let mut out = vec![
        format!("# README audit: {group_path}"),
        String::new(),
        format!(
            "{} projects scanned · **{missing} missing · {cyrillic} Russian/Cyrillic · {small} small (<{small_bytes}B)** · {ok} ok",
            results.len()
        ),
    ];

    let section = |out: &mut Vec<String>, title: &str, v: Verdict| {
        let mut rows: Vec<&RepoReadme> = results.iter().filter(|r| r.verdict == v).collect();
        if rows.is_empty() {
            return;
        }
        // Worst first: missing has no size; others by size asc (smallest = worst).
        rows.sort_by(|a, b| a.size.cmp(&b.size));
        out.push(String::new());
        out.push(format!("## {title} ({})", rows.len()));
        out.push(String::new());
        out.push("| Repo | README | Size | Cyrillic |".to_string());
        out.push("|------|--------|------|----------|".to_string());
        for r in rows {
            let readme_link = if r.web_url.is_empty() || r.fname == "-" {
                r.fname.clone()
            } else {
                format!(
                    "[{}]({}/-/blob/{}/{})",
                    r.fname, r.web_url, r.default_branch, r.fname
                )
            };
            let size = if r.verdict == Verdict::Missing {
                "–".to_string()
            } else {
                format!("{}B", r.size)
            };
            out.push(format!(
                "| [{}]({}) | {readme_link} | {size} | {}% |",
                r.path, r.web_url, r.cyr_pct
            ));
        }
    };

    section(&mut out, "Missing README", Verdict::Missing);
    section(&mut out, "Russian / Cyrillic README", Verdict::Cyrillic);
    section(&mut out, "Small / stub README", Verdict::Small);
    if include_ok {
        section(&mut out, "OK", Verdict::Ok);
    }

    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::cyrillic_pct;

    #[test]
    fn cyrillic_detection() {
        assert_eq!(cyrillic_pct("Just plain English README text."), 0);
        assert_eq!(cyrillic_pct("Полностью русский текст"), 100);
        // Mixed but mostly Russian prose → flagged high.
        assert!(cyrillic_pct("VPN сервер конфигурация нод") >= 70);
        // Predominantly-English README with one short Russian note stays low.
        let mostly_english = "# Backend service\n\nInstall with cargo build and run the \
            server. Configure the database connection and the redis cache. \
            See docs for deployment. Примечание: смотри конфиг.";
        assert!(cyrillic_pct(mostly_english) < 20, "got {}", cyrillic_pct(mostly_english));
        assert_eq!(cyrillic_pct("```\n1234 5678\n```"), 0); // no letters
    }
}
