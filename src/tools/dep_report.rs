//! Weekly dependency-vulnerability report: the `audit_dependencies` findings turned
//! into an email a lead can act on in a minute.
//!
//! A full audit of a group is hundreds of rows, and the same rows every week. What
//! a lead needs is what changed (new findings, fixed ones), which repositories to
//! fix first, and how long the rest has been open. That takes memory between runs:
//! each run writes a JSON snapshot, and the next run diffs against it.
//!
//! Everything here is pure: the CLI gathers the audit and the previous snapshot,
//! these functions decide and render.

use std::collections::{BTreeMap, BTreeSet};

use chrono::NaiveDate;
use serde::{Deserialize, Serialize};

use super::dependencies::{Audit, PackageFinding, VulnSeverity, upgrade_target};
use super::reports::htmlescape as esc;

/// Bumped when the snapshot layout changes incompatibly; an older snapshot is then
/// treated as absent (a baseline week) rather than misread.
pub(crate) const SNAPSHOT_SCHEMA: u32 = 1;

/// One vulnerable package version pinned in one repository.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub(crate) struct Item {
    pub repo: String,
    pub ecosystem: String,
    pub package: String,
    pub version: String,
    pub severity: String,
    /// CVE ids where known, else the advisory id; sorted.
    pub advisories: Vec<String>,
    pub runtime: bool,
    pub fix: String,
    /// First run that saw this item (YYYY-MM-DD).
    pub first_seen: String,
}

impl Item {
    fn key(&self) -> (&str, &str, &str, &str) {
        (&self.repo, &self.ecosystem, &self.package, &self.version)
    }

    fn sev(&self) -> VulnSeverity {
        severity_from_label(&self.severity)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct Snapshot {
    pub schema: u32,
    pub date: String,
    pub scope: String,
    /// Repositories actually scanned this run. A finding only counts as fixed when
    /// its repository was rescanned: a scan that failed is not a fix.
    pub scanned: Vec<String>,
    pub items: Vec<Item>,
    /// Email subject line, so the sender needs no logic of its own.
    pub subject: String,
}

fn severity_from_label(label: &str) -> VulnSeverity {
    match label {
        "CRITICAL" => VulnSeverity::Critical,
        "HIGH" => VulnSeverity::High,
        "MEDIUM" => VulnSeverity::Medium,
        "LOW" => VulnSeverity::Low,
        _ => VulnSeverity::Unrated,
    }
}

/// Explode package findings into per-repository items for this run.
pub(crate) fn items_from(findings: &[PackageFinding], today: &str) -> Vec<Item> {
    let mut items = Vec::new();
    for f in findings {
        let repos: BTreeSet<&str> = f
            .locations
            .iter()
            .map(|l| l.split_once(':').map_or(l.as_str(), |(r, _)| r))
            .collect();
        let advisories: Vec<String> = f
            .advisories
            .iter()
            .map(|a| a.cve.clone().unwrap_or_else(|| a.id.clone()))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        let fix = upgrade_target(&f.dep.version, &f.advisories).label();
        for repo in repos {
            items.push(Item {
                repo: repo.to_string(),
                ecosystem: f.dep.ecosystem.osv_name().to_string(),
                package: f.dep.name.clone(),
                version: f.dep.version.clone(),
                severity: f.severity().label().to_string(),
                advisories: advisories.clone(),
                runtime: f.runtime_repos.contains(repo),
                fix: fix.clone(),
                first_seen: today.to_string(),
            });
        }
    }
    items.sort_by(|a, b| {
        (a.sev(), !a.runtime, &a.repo, &a.package).cmp(&(b.sev(), !b.runtime, &b.repo, &b.package))
    });
    items
}

/// What changed since the previous run.
#[derive(Debug, Default)]
pub(crate) struct Diff {
    /// No usable previous snapshot: this run is the baseline.
    pub baseline: bool,
    pub previous_date: Option<String>,
    /// Items not seen last time.
    pub new: Vec<Item>,
    /// Items seen before that picked up advisories since (newly disclosed).
    pub new_advisories: Vec<(Item, Vec<String>)>,
    /// Items gone from a rescanned repository, with the version now pinned there
    /// if that one is still vulnerable.
    pub fixed: Vec<(Item, Option<String>)>,
    /// Items from repositories that were not rescanned: carried over, not fixed.
    pub carried: Vec<Item>,
    /// Every open item this run, with `first_seen` carried forward.
    pub open: Vec<Item>,
}

/// Diff this run against the previous snapshot. The previous snapshot only counts
/// when it is the same schema and scope; otherwise this run is a baseline.
pub(crate) fn diff(
    previous: Option<&Snapshot>,
    mut current: Vec<Item>,
    scope: &str,
    scanned: &BTreeSet<String>,
) -> Diff {
    let prev = previous.filter(|p| p.schema == SNAPSHOT_SCHEMA && p.scope == scope);
    let Some(prev) = prev else {
        return Diff {
            baseline: true,
            open: current,
            ..Default::default()
        };
    };
    let before: BTreeMap<_, &Item> = prev.items.iter().map(|i| (i.key(), i)).collect();
    let mut d = Diff {
        previous_date: Some(prev.date.clone()),
        ..Default::default()
    };
    for item in &mut current {
        match before.get(&item.key()).copied() {
            None => d.new.push(item.clone()),
            Some(old) => {
                item.first_seen = old.first_seen.clone();
                let added: Vec<String> = item
                    .advisories
                    .iter()
                    .filter(|a| !old.advisories.contains(a))
                    .cloned()
                    .collect();
                if !added.is_empty() {
                    d.new_advisories.push((item.clone(), added));
                }
            }
        }
    }
    let now: BTreeSet<_> = current.iter().map(Item::key).collect();
    for old in &prev.items {
        if now.contains(&old.key()) {
            continue;
        }
        if scanned.contains(&old.repo) {
            let still = current
                .iter()
                .find(|c| {
                    c.repo == old.repo && c.ecosystem == old.ecosystem && c.package == old.package
                })
                .map(|c| c.version.clone());
            d.fixed.push((old.clone(), still));
        } else {
            d.carried.push(old.clone());
        }
    }
    current.extend(d.carried.iter().cloned());
    d.open = current;
    d
}

/// Repositories whose path marks them as tests, scripts or templates: real findings,
/// but nothing there serves users, so they rank below services.
pub(crate) fn is_tooling_repo(repo: &str) -> bool {
    repo.split('/').any(|seg| {
        let s = seg.to_ascii_lowercase();
        s == "qa"
            || s.contains("test")
            || s.contains("script")
            || s.contains("template")
            || s.contains("boilerplate")
            || s.contains("example")
            || s.contains("demo")
            || s.contains("sandbox")
    })
}

fn weight(s: VulnSeverity) -> f64 {
    match s {
        VulnSeverity::Critical => 10.0,
        VulnSeverity::High => 4.0,
        VulnSeverity::Medium => 1.0,
        VulnSeverity::Low => 0.25,
        VulnSeverity::Unrated => 0.5,
    }
}

/// One repository's open findings, scored for "fix first".
#[derive(Debug)]
pub(crate) struct RepoRank<'a> {
    pub repo: &'a str,
    pub tooling: bool,
    pub score: f64,
    pub items: Vec<&'a Item>,
}

/// Rank repositories by exposure: runtime findings weighted by severity, dev-only
/// findings at a quarter, tooling repositories at 30%. Transparent on purpose — the
/// email states the rule.
pub(crate) fn rank_repos(open: &[Item]) -> Vec<RepoRank<'_>> {
    let mut by_repo: BTreeMap<&str, Vec<&Item>> = BTreeMap::new();
    for i in open {
        by_repo.entry(i.repo.as_str()).or_default().push(i);
    }
    let mut ranks: Vec<RepoRank> = by_repo
        .into_iter()
        .map(|(repo, mut items)| {
            let tooling = is_tooling_repo(repo);
            let raw: f64 = items
                .iter()
                .map(|i| weight(i.sev()) * if i.runtime { 1.0 } else { 0.25 })
                .sum();
            items.sort_by(|a, b| {
                (a.sev(), !a.runtime, &a.package).cmp(&(b.sev(), !b.runtime, &b.package))
            });
            RepoRank {
                repo,
                tooling,
                score: if tooling { raw * 0.3 } else { raw },
                items,
            }
        })
        .collect();
    ranks.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.repo.cmp(b.repo)));
    ranks
}

fn is_urgent(i: &Item) -> bool {
    i.runtime && i.sev() <= VulnSeverity::High
}

/// The subject line: the change first, then the standing total.
pub(crate) fn subject(scope_name: &str, d: &Diff) -> String {
    let urgent_open = d.open.iter().filter(|i| is_urgent(i)).count();
    let urgent_new = d.new.iter().filter(|i| is_urgent(i)).count()
        + d.new_advisories
            .iter()
            .filter(|(i, _)| is_urgent(i))
            .count();
    let change = if d.baseline {
        "baseline".to_string()
    } else if urgent_new > 0 {
        format!("{urgent_new} new critical/high")
    } else {
        "nothing new critical/high".to_string()
    };
    let fixed = if d.fixed.is_empty() {
        String::new()
    } else {
        format!(" · {} fixed", d.fixed.len())
    };
    format!(
        "Dependency CVEs — {scope_name}: {change} · {urgent_open} open runtime critical/high{fixed}"
    )
}

/// Whole weeks between two YYYY-MM-DD dates.
fn weeks_between(from: &str, to: &str) -> Option<i64> {
    let a = NaiveDate::parse_from_str(from, "%Y-%m-%d").ok()?;
    let b = NaiveDate::parse_from_str(to, "%Y-%m-%d").ok()?;
    Some((b - a).num_days() / 7)
}

// ─── HTML ───
// Inline styles throughout: Gmail strips <style> blocks, and this is read in mail.

const SEV_COLORS: [(&str, &str); 5] = [
    ("CRITICAL", "#b91c1c"),
    ("HIGH", "#c2410c"),
    ("MEDIUM", "#a16207"),
    ("LOW", "#4b5563"),
    ("UNRATED", "#6b7280"),
];

fn badge(sev: &str) -> String {
    let color = SEV_COLORS
        .iter()
        .find(|(s, _)| *s == sev)
        .map_or("#6b7280", |(_, c)| c);
    format!(
        r#"<span style="display:inline-block;padding:1px 6px;border-radius:4px;background:{color};color:#fff;font-size:11px;font-weight:600;letter-spacing:.02em">{}</span>"#,
        esc(sev)
    )
}

const TH: &str = "text-align:left;padding:6px 8px;border-bottom:2px solid #e5e7eb;font-size:12px;color:#6b7280;font-weight:600";
const TD: &str =
    "padding:6px 8px;border-bottom:1px solid #f3f4f6;font-size:13px;vertical-align:top";
const H2: &str = "font-size:17px;margin:28px 0 8px;color:#111827";
const MUTED: &str = "color:#6b7280;font-size:12px";

fn short_repo(repo: &str, scope_group: &str) -> String {
    repo.strip_prefix(scope_group)
        .map(|r| r.trim_start_matches('/'))
        .filter(|r| !r.is_empty())
        .unwrap_or(repo)
        .to_string()
}

fn advisories_text(ids: &[String], limit: usize) -> String {
    if ids.len() > limit {
        format!("{} +{} more", ids[..limit].join(", "), ids.len() - limit)
    } else {
        ids.join(", ")
    }
}

fn package_cell(i: &Item) -> String {
    format!(
        r#"<b>{}</b>@{} <span style="{MUTED}">{}{}</span>"#,
        esc(&i.package),
        esc(&i.version),
        esc(&i.ecosystem),
        if i.runtime { "" } else { " · dev" }
    )
}

fn kpi(label: &str, value: String, color: &str, note: &str) -> String {
    format!(
        r#"<td style="padding:10px 12px;border:1px solid #e5e7eb;border-radius:6px;width:25%"><div style="{MUTED}">{label}</div><div style="font-size:24px;font-weight:700;color:{color}">{value}</div><div style="{MUTED}">{note}</div></td>"#
    )
}

/// Inputs to the email besides the diff.
pub(crate) struct ReportMeta<'a> {
    pub group: &'a str,
    pub today: &'a str,
    pub repos_scanned: usize,
    pub total_repos: usize,
    pub unique_deps: usize,
    pub no_lockfile: Vec<String>,
    pub unsupported: Vec<String>,
    pub declared_only: Vec<String>,
    pub warnings: &'a str,
}

impl<'a> ReportMeta<'a> {
    pub(crate) fn from_audit(audit: &'a Audit, group: &'a str, today: &'a str) -> Self {
        let mut no_lockfile = Vec::new();
        let mut unsupported = Vec::new();
        let mut declared_only = Vec::new();
        for s in &audit.scans {
            if s.lockfiles.is_empty() && s.unsupported.is_empty() {
                no_lockfile.push(s.path.clone());
            }
            unsupported.extend(s.unsupported.iter().map(|_| s.path.clone()));
            declared_only.extend(s.declared_only.iter().map(|_| s.path.clone()));
        }
        ReportMeta {
            group,
            today,
            repos_scanned: audit.scans.len(),
            total_repos: audit.total_repos,
            unique_deps: audit.unique_deps,
            no_lockfile,
            unsupported,
            declared_only,
            warnings: &audit.warnings,
        }
    }
}

fn list_short(names: &[String], group: &str, limit: usize) -> String {
    let set: BTreeSet<String> = names.iter().map(|n| short_repo(n, group)).collect();
    let v: Vec<String> = set.into_iter().collect();
    let shown: Vec<String> = v.iter().take(limit).map(|n| esc(n)).collect();
    if v.len() > limit {
        format!("{} and {} more", shown.join(", "), v.len() - limit)
    } else {
        shown.join(", ")
    }
}

/// The email. Pure: the date, the diff and the scan metadata are all inputs.
/// `full` adds the appendix of every open finding. The email body leaves it out:
/// Gmail clips bodies over ~102 KB, so the full list travels as the attachment.
pub(crate) fn render_email(meta: &ReportMeta, d: &Diff, full: bool) -> String {
    let g = meta.group;
    let urgent_open = d.open.iter().filter(|i| is_urgent(i)).count();
    let urgent_new: Vec<&Item> = d.new.iter().filter(|i| is_urgent(i)).collect();
    let ranks = rank_repos(&d.open);
    let repos_urgent = ranks
        .iter()
        .filter(|r| r.items.iter().any(|i| is_urgent(i)))
        .count();
    let packages: BTreeSet<(&str, &str, &str)> = d
        .open
        .iter()
        .map(|i| (i.ecosystem.as_str(), i.package.as_str(), i.version.as_str()))
        .collect();

    let mut h = String::new();
    h.push_str(&format!(
        r#"<!DOCTYPE html>
<html lang="en"><head><meta charset="UTF-8"><meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>Dependency vulnerabilities — {ge} — {today}</title></head>
<body style="margin:0;padding:16px;background:#ffffff;color:#111827;font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,Helvetica,Arial,sans-serif">
<div style="max-width:820px;margin:0 auto">
<h1 style="font-size:22px;margin:0 0 4px">Dependency vulnerabilities — {ge}</h1>
<div style="{MUTED}">{today} · {scanned} of {total} repos scanned · {deps} distinct packages · {since}</div>
"#,
        ge = esc(g),
        today = esc(meta.today),
        scanned = meta.repos_scanned,
        total = meta.total_repos,
        deps = meta.unique_deps,
        since = match (&d.previous_date, d.baseline) {
            (Some(p), false) => format!("compared with {}", esc(p)),
            _ => "first run — this week is the baseline".to_string(),
        },
    ));
    if !meta.warnings.trim().is_empty() {
        h.push_str(&format!(
            r#"<div style="margin-top:10px;padding:8px 10px;background:#fef3c7;border-radius:6px;font-size:13px">⚠ {}</div>"#,
            esc(meta.warnings.trim())
        ));
    }

    // KPI row
    let new_value = if d.baseline {
        "–".to_string()
    } else {
        urgent_new.len().to_string()
    };
    let fixed_value = if d.baseline {
        "–".to_string()
    } else {
        d.fixed.len().to_string()
    };
    h.push_str(r#"<table role="presentation" style="width:100%;border-collapse:separate;border-spacing:6px;margin:14px -6px 0"><tr>"#);
    h.push_str(&kpi(
        "Open runtime critical/high",
        urgent_open.to_string(),
        "#b91c1c",
        &format!("{} vulnerable packages in total", packages.len()),
    ));
    h.push_str(&kpi(
        "New critical/high",
        new_value,
        "#c2410c",
        "since last report",
    ));
    h.push_str(&kpi("Fixed", fixed_value, "#15803d", "upgraded or removed"));
    h.push_str(&kpi(
        "Repos to act on",
        repos_urgent.to_string(),
        "#1d4ed8",
        "with runtime critical/high",
    ));
    h.push_str("</tr></table>\n");

    // What changed
    if !d.baseline {
        h.push_str(&format!(
            r#"<h2 style="{H2}">New since {}</h2>"#,
            esc(d.previous_date.as_deref().unwrap_or(""))
        ));
        let mut new_rows: Vec<(&Item, Option<&Vec<String>>)> =
            d.new.iter().map(|i| (i, None)).collect();
        new_rows.extend(d.new_advisories.iter().map(|(i, a)| (i, Some(a))));
        new_rows.sort_by(|a, b| {
            (a.0.sev(), !a.0.runtime, &a.0.repo).cmp(&(b.0.sev(), !b.0.runtime, &b.0.repo))
        });
        if new_rows.is_empty() {
            h.push_str(r#"<p style="font-size:14px">Nothing new. 👍</p>"#);
        } else {
            h.push_str(&format!(
                r#"<table style="width:100%;border-collapse:collapse"><tr><th style="{TH}">Severity</th><th style="{TH}">Package</th><th style="{TH}">Repository</th><th style="{TH}">Why new</th><th style="{TH}">Fix</th></tr>"#
            ));
            for (i, added) in new_rows.iter().take(40) {
                let why = match added {
                    Some(a) => format!("newly disclosed: {}", esc(&advisories_text(a, 3))),
                    None => format!(
                        "newly pinned or disclosed: {}",
                        esc(&advisories_text(&i.advisories, 3))
                    ),
                };
                h.push_str(&format!(
                    r#"<tr><td style="{TD}">{}</td><td style="{TD}">{}</td><td style="{TD}">{}</td><td style="{TD}">{why}</td><td style="{TD}">{}</td></tr>"#,
                    badge(&i.severity),
                    package_cell(i),
                    esc(&short_repo(&i.repo, g)),
                    esc(&i.fix)
                ));
            }
            h.push_str("</table>");
            if new_rows.len() > 40 {
                h.push_str(&format!(
                    r#"<div style="{MUTED}">{} more in the attachment.</div>"#,
                    new_rows.len() - 40
                ));
            }
        }
        if !d.fixed.is_empty() {
            h.push_str(&format!(r#"<h2 style="{H2}">Fixed since {} ✅</h2><ul style="font-size:13px;padding-left:18px;margin:0">"#, esc(d.previous_date.as_deref().unwrap_or(""))));
            let mut by_repo: BTreeMap<&str, Vec<String>> = BTreeMap::new();
            for (i, still) in &d.fixed {
                let what = match still {
                    Some(v) => format!(
                        "{} {} → {} (still vulnerable)",
                        esc(&i.package),
                        esc(&i.version),
                        esc(v)
                    ),
                    None => format!("{} {}", esc(&i.package), esc(&i.version)),
                };
                by_repo.entry(i.repo.as_str()).or_default().push(what);
            }
            for (repo, list) in by_repo {
                h.push_str(&format!(
                    "<li><b>{}</b>: {}</li>",
                    esc(&short_repo(repo, g)),
                    list.join(", ")
                ));
            }
            h.push_str("</ul>");
        }
        if !d.carried.is_empty() {
            h.push_str(&format!(
                r#"<div style="{MUTED};margin-top:8px">{} findings carried over from repositories that could not be rescanned this week — not counted as fixed.</div>"#,
                d.carried.len()
            ));
        }
    }

    // Fix first
    h.push_str(&format!(
        r#"<h2 style="{H2}">Fix first — by repository</h2>"#
    ));
    h.push_str(&format!(
        r#"<div style="{MUTED};margin-bottom:8px">Ranked by exposure: runtime findings weighted by severity (critical 10, high 4, medium 1), dev-only at ¼, test/script/template repos at 30%.</div>"#
    ));
    for r in ranks
        .iter()
        .filter(|r| r.items.iter().any(|i| i.sev() <= VulnSeverity::High))
        .take(12)
    {
        let crit = r
            .items
            .iter()
            .filter(|i| i.sev() == VulnSeverity::Critical)
            .count();
        let high = r
            .items
            .iter()
            .filter(|i| i.sev() == VulnSeverity::High)
            .count();
        let tag = if r.tooling {
            r#" <span style="font-size:11px;color:#6b7280;border:1px solid #d1d5db;border-radius:4px;padding:0 4px">tooling</span>"#
        } else {
            ""
        };
        h.push_str(&format!(
            r#"<div style="border:1px solid #e5e7eb;border-radius:8px;padding:10px 12px;margin:8px 0"><div style="font-size:15px;font-weight:600">{}{tag} <span style="{MUTED};font-weight:400">· {crit} critical · {high} high · {} vulnerable packages</span></div><table style="width:100%;border-collapse:collapse;margin-top:6px">"#,
            esc(&short_repo(r.repo, g)),
            r.items.len()
        ));
        for i in r.items.iter().take(5) {
            let age = weeks_between(&i.first_seen, meta.today)
                .filter(|w| *w >= 1)
                .map(|w| format!(r#" <span style="{MUTED}">· open {w} wk</span>"#))
                .unwrap_or_default();
            h.push_str(&format!(
                r#"<tr><td style="{TD};width:80px">{}</td><td style="{TD}">{}{age}<div style="{MUTED}">{}</div></td><td style="{TD};white-space:nowrap">{}</td></tr>"#,
                badge(&i.severity),
                package_cell(i),
                esc(&advisories_text(&i.advisories, 3)),
                esc(&i.fix)
            ));
        }
        h.push_str("</table>");
        if r.items.len() > 5 {
            h.push_str(&format!(
                r#"<div style="{MUTED};margin-top:4px">+{} more in the attachment</div>"#,
                r.items.len() - 5
            ));
        }
        h.push_str("</div>");
    }

    // Coverage
    h.push_str(&format!(r#"<h2 style="{H2}">What this does not cover</h2><ul style="font-size:13px;padding-left:18px;margin:0">"#));
    if !meta.no_lockfile.is_empty() {
        h.push_str(&format!(
            "<li><b>{} repos have no lockfile</b>, so their dependencies cannot be audited: {}. Committing a lockfile brings a repo into the report.</li>",
            meta.no_lockfile.len(),
            list_short(&meta.no_lockfile, g, 6)
        ));
    }
    if !meta.unsupported.is_empty() {
        h.push_str(&format!(
            "<li><b>CocoaPods</b> ({}): no public advisory feed exists for it.</li>",
            list_short(&meta.unsupported, g, 6)
        ));
    }
    if !meta.declared_only.is_empty() {
        h.push_str(&format!(
            "<li><b>Gradle version catalogs</b> ({}): direct dependencies only; enable Gradle dependency locking to cover transitive ones.</li>",
            list_short(&meta.declared_only, g, 6)
        ));
    }
    h.push_str("<li>Only versions pinned in lockfiles are checked; code you wrote, container images and OS packages are out of scope.</li></ul>");

    // Appendix
    if !full {
        h.push_str(&format!(
            r#"<p style="font-size:13px;margin-top:20px">The full list of all {} open findings is in the attached report.</p>"#,
            d.open.len()
        ));
    }
    if full {
        h.push_str(&format!(r#"<h2 style="{H2}">All open findings</h2>"#));
        h.push_str(&format!(
        r#"<table style="width:100%;border-collapse:collapse"><tr><th style="{TH}">Severity</th><th style="{TH}">Repository</th><th style="{TH}">Package</th><th style="{TH}">Advisories</th><th style="{TH}">Fix</th></tr>"#
    ));
        let mut all: Vec<&Item> = d.open.iter().collect();
        all.sort_by(|a, b| {
            (a.sev(), !a.runtime, &a.repo, &a.package).cmp(&(
                b.sev(),
                !b.runtime,
                &b.repo,
                &b.package,
            ))
        });
        for i in &all {
            h.push_str(&format!(
            r#"<tr><td style="{TD}">{}</td><td style="{TD}">{}</td><td style="{TD}">{}</td><td style="{TD}">{}</td><td style="{TD}">{}</td></tr>"#,
            badge(&i.severity),
            esc(&short_repo(&i.repo, g)),
            package_cell(i),
            esc(&advisories_text(&i.advisories, 4)),
            esc(&i.fix)
        ));
        }
        h.push_str("</table>");
    }
    h.push_str(&format!(
        r#"<p style="{MUTED};margin-top:20px">Matched against OSV.dev (GitHub, RustSec, Go, PyPA and other advisory feeds). Advisories for one issue are merged; severity is the advisory's own rating, else its CVSS v3 score. "Fix" is the lowest version that resolves every advisory with a published fix — verify it against your changelog before upgrading. Generated by gl-mcp v{}.</p>
</div></body></html>
"#,
        env!("CARGO_PKG_VERSION")
    ));
    h
}

/// The weekly run: audit the group, diff against the previous snapshot (raw JSON,
/// if any), and return the email plus the snapshot to keep for next week.
pub async fn weekly_report(
    client: &crate::client::GitLabClient,
    group: &str,
    max_projects: usize,
    previous_json: Option<&str>,
    today: &str,
) -> crate::error::Result<WeeklyReport> {
    let audit = super::dependencies::collect_audit(client, "", group, max_projects).await?;
    // An unreadable previous snapshot means a baseline week, not a failed report.
    let previous: Option<Snapshot> = previous_json.and_then(|j| serde_json::from_str(j).ok());
    let scanned: BTreeSet<String> = audit.scans.iter().map(|s| s.path.clone()).collect();
    let d = diff(
        previous.as_ref(),
        items_from(&audit.findings, today),
        &audit.scope,
        &scanned,
    );
    let meta = ReportMeta::from_audit(&audit, group, today);
    let body = render_email(&meta, &d, false);
    let full = render_email(&meta, &d, true);
    let snapshot = Snapshot {
        schema: SNAPSHOT_SCHEMA,
        date: today.to_string(),
        scope: audit.scope.clone(),
        scanned: scanned.into_iter().collect(),
        subject: subject(group, &d),
        items: d.open,
    };
    let json = serde_json::to_string_pretty(&snapshot)
        .map_err(|e| crate::error::Error::other(format!("snapshot: {e}")))?;
    Ok(WeeklyReport {
        body,
        full,
        snapshot: json,
    })
}

pub struct WeeklyReport {
    /// Email body: everything but the full appendix.
    pub body: String,
    /// The same report with every open finding, for the attachment.
    pub full: String,
    pub snapshot: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(repo: &str, pkg: &str, ver: &str, sev: &str, runtime: bool, first_seen: &str) -> Item {
        Item {
            repo: repo.into(),
            ecosystem: "npm".into(),
            package: pkg.into(),
            version: ver.into(),
            severity: sev.into(),
            advisories: vec![format!("CVE-2099-{pkg}")],
            runtime,
            fix: "→ 9.9.9".into(),
            first_seen: first_seen.into(),
        }
    }

    fn snap(date: &str, items: Vec<Item>) -> Snapshot {
        Snapshot {
            schema: SNAPSHOT_SCHEMA,
            date: date.into(),
            scope: "group `acme`".into(),
            scanned: items.iter().map(|i| i.repo.clone()).collect(),
            items,
            subject: String::new(),
        }
    }

    fn scanned(repos: &[&str]) -> BTreeSet<String> {
        repos.iter().map(|r| r.to_string()).collect()
    }

    #[test]
    fn first_run_is_a_baseline() {
        let d = diff(
            None,
            vec![item(
                "acme/api",
                "lodash",
                "4.17.15",
                "HIGH",
                true,
                "2026-10-26",
            )],
            "group `acme`",
            &scanned(&["acme/api"]),
        );
        assert!(d.baseline);
        assert!(d.new.is_empty(), "a baseline reports nothing as new");
        assert_eq!(d.open.len(), 1);
    }

    #[test]
    fn another_scope_or_schema_is_a_baseline_too() {
        let mut prev = snap("2026-10-19", vec![]);
        prev.scope = "group `other`".into();
        assert!(diff(Some(&prev), vec![], "group `acme`", &scanned(&[])).baseline);
        let mut prev = snap("2026-10-19", vec![]);
        prev.schema = SNAPSHOT_SCHEMA + 1;
        assert!(diff(Some(&prev), vec![], "group `acme`", &scanned(&[])).baseline);
    }

    #[test]
    fn diff_finds_new_fixed_upgraded_and_carries_age() {
        let prev = snap(
            "2026-10-19",
            vec![
                item("acme/api", "lodash", "4.17.15", "HIGH", true, "2026-09-28"),
                item(
                    "acme/api",
                    "minimist",
                    "0.0.8",
                    "CRITICAL",
                    true,
                    "2026-10-05",
                ),
                item("acme/web", "axios", "0.19.2", "HIGH", true, "2026-10-12"),
                item("acme/gone", "qs", "6.5.2", "HIGH", true, "2026-10-12"),
            ],
        );
        let mut lodash = item("acme/api", "lodash", "4.17.15", "HIGH", true, "2026-10-26");
        lodash.advisories.push("CVE-2099-fresh".into());
        let current = vec![
            lodash,
            item("acme/web", "axios", "0.21.1", "HIGH", true, "2026-10-26"),
            item("acme/web", "vite", "5.2.0", "CRITICAL", true, "2026-10-26"),
        ];
        // acme/gone failed to scan this week.
        let d = diff(
            Some(&prev),
            current,
            "group `acme`",
            &scanned(&["acme/api", "acme/web"]),
        );
        assert!(!d.baseline);
        let new: Vec<_> = d
            .new
            .iter()
            .map(|i| format!("{}@{}", i.package, i.version))
            .collect();
        assert_eq!(new, vec!["axios@0.21.1", "vite@5.2.0"]);
        assert_eq!(d.new_advisories.len(), 1);
        assert_eq!(d.new_advisories[0].1, vec!["CVE-2099-fresh".to_string()]);
        let fixed: Vec<_> = d
            .fixed
            .iter()
            .map(|(i, still)| (i.package.as_str(), still.clone()))
            .collect();
        assert_eq!(
            fixed,
            vec![("minimist", None), ("axios", Some("0.21.1".to_string()))]
        );
        assert_eq!(
            d.carried.len(),
            1,
            "a repo that failed to scan is not fixed"
        );
        assert_eq!(d.carried[0].repo, "acme/gone");
        let lodash = d.open.iter().find(|i| i.package == "lodash").unwrap();
        assert_eq!(lodash.first_seen, "2026-09-28", "age survives across runs");
        assert_eq!(d.open.len(), 4, "3 current + 1 carried");
    }

    #[test]
    fn tooling_repos_rank_below_services() {
        assert!(is_tooling_repo("acme/qa/web-tests"));
        assert!(is_tooling_repo("acme/frontend/boilerplates"));
        assert!(is_tooling_repo("acme/tools/qa-scripts"));
        assert!(!is_tooling_repo("acme/backend/api"));
        let open = vec![
            item(
                "acme/qa/e2e-tests",
                "a",
                "1",
                "CRITICAL",
                true,
                "2026-10-26",
            ),
            item(
                "acme/qa/e2e-tests",
                "b",
                "1",
                "CRITICAL",
                true,
                "2026-10-26",
            ),
            item("acme/backend/api", "c", "1", "CRITICAL", true, "2026-10-26"),
            item("acme/web", "d", "1", "CRITICAL", false, "2026-10-26"),
        ];
        let order: Vec<&str> = rank_repos(&open).iter().map(|r| r.repo).collect();
        // api 10 > tests 2×10×0.3 = 6 > web dev-only 2.5
        assert_eq!(
            order,
            vec!["acme/backend/api", "acme/qa/e2e-tests", "acme/web"]
        );
    }

    #[test]
    fn subject_leads_with_the_change() {
        let d = diff(
            None,
            vec![item("acme/api", "x", "1", "HIGH", true, "2026-10-26")],
            "group `acme`",
            &scanned(&["acme/api"]),
        );
        assert_eq!(
            subject("acme", &d),
            "Dependency CVEs — acme: baseline · 1 open runtime critical/high"
        );
        let prev = snap(
            "2026-10-19",
            vec![item("acme/api", "old", "1", "HIGH", true, "2026-10-12")],
        );
        let d = diff(
            Some(&prev),
            vec![
                item("acme/api", "x", "1", "CRITICAL", true, "2026-10-26"),
                item("acme/api", "y", "1", "HIGH", false, "2026-10-26"),
            ],
            "group `acme`",
            &scanned(&["acme/api"]),
        );
        assert_eq!(
            subject("acme", &d),
            "Dependency CVEs — acme: 1 new critical/high · 1 open runtime critical/high · 1 fixed"
        );
    }

    fn meta(today: &'static str) -> ReportMeta<'static> {
        ReportMeta {
            group: "acme",
            today,
            repos_scanned: 4,
            total_repos: 4,
            unique_deps: 900,
            no_lockfile: vec!["acme/docs".into()],
            unsupported: vec!["acme/ios".into()],
            declared_only: vec!["acme/android".into()],
            warnings: "",
        }
    }

    #[test]
    fn golden_weekly_email() {
        let prev = snap(
            "2026-10-19",
            vec![
                item(
                    "acme/backend/api",
                    "lodash",
                    "4.17.15",
                    "HIGH",
                    true,
                    "2026-09-28",
                ),
                item(
                    "acme/backend/api",
                    "minimist",
                    "0.0.8",
                    "CRITICAL",
                    true,
                    "2026-10-05",
                ),
            ],
        );
        let current = vec![
            item(
                "acme/backend/api",
                "lodash",
                "4.17.15",
                "HIGH",
                true,
                "2026-10-26",
            ),
            item("acme/web", "vite", "5.2.0", "CRITICAL", true, "2026-10-26"),
            item(
                "acme/qa/e2e-tests",
                "pillow",
                "10.3.0",
                "CRITICAL",
                true,
                "2026-10-26",
            ),
            item("acme/web", "<script>", "1", "MEDIUM", false, "2026-10-26"),
        ];
        let d = diff(
            Some(&prev),
            current,
            "group `acme`",
            &scanned(&["acme/backend/api", "acme/web", "acme/qa/e2e-tests"]),
        );
        let html = render_email(&meta("2026-10-26"), &d, true);
        crate::golden::assert_golden("render/dependency_email.html", &html);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(
            !html.contains("<style"),
            "inline styles only: mail clients strip <style>"
        );
        assert!(!html.contains("<script>"), "package names are escaped");
        assert!(html.contains("open 4 wk"), "lodash first seen 4 weeks ago");
        assert!(html.contains("Fixed since 2026-10-19"));
        assert!(html.contains("<b>backend/api</b>: minimist 0.0.8"));
        let ranking =
            &html[html.find("Fix first").unwrap()..html.find("What this does not cover").unwrap()];
        assert!(
            ranking.find(">web ").unwrap() < ranking.find(">qa/e2e-tests").unwrap(),
            "services before tooling"
        );
    }

    #[test]
    fn baseline_email_has_no_change_section() {
        let d = diff(
            None,
            vec![item(
                "acme/web",
                "vite",
                "5.2.0",
                "CRITICAL",
                true,
                "2026-10-26",
            )],
            "group `acme`",
            &scanned(&["acme/web"]),
        );
        let html = render_email(&meta("2026-10-26"), &d, false);
        assert!(html.contains("All 1 open findings") || html.contains("all 1 open findings"));
        assert!(
            !html.contains("All open findings"),
            "the body leaves the appendix to the attachment"
        );
        assert!(html.contains("first run — this week is the baseline"));
        assert!(!html.contains("New since"));
    }

    #[test]
    fn snapshot_round_trips_through_json() {
        let s = snap(
            "2026-10-19",
            vec![item(
                "acme/api",
                "lodash",
                "4.17.15",
                "HIGH",
                true,
                "2026-09-28",
            )],
        );
        let back: Snapshot = serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        assert_eq!(back, s);
    }
}
