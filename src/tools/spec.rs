//! Spec-drift audit: cross-reference a documented API/metadata spec (e.g. a
//! per-platform app-spec article from a knowledge base) against the actual
//! codebase.
//!
//! Three checks:
//! - **Route drift** — each endpoint the spec documents is searched in the repo
//!   and classified: cleanup-debt (spec flags it for removal but it's still
//!   wired in), stale-doc (flagged for removal and already gone), drift (listed
//!   as active but missing), or in-sync.
//! - **Version** — the spec's documented version vs the repo's latest tag.
//! - (needs-review) routes whose path is too generic to match reliably are
//!   surfaced rather than silently guessed.
//!
//! gl-mcp stays GitLab-only: the spec text is supplied by the caller (fetch the
//! article with whatever KB tool you use, pass its markdown in via `spec`).
//! This module does only the GitLab-side analysis.
//!
//! The per-route audit set ([`RouteAudit`]) is the natural unit a future local
//! "metadata map" would persist after the first run, so run-over-run diffs and
//! reverse-drift (code endpoints absent from the spec) can be layered on top
//! without re-deriving the heuristics here.

use crate::client::GitLabClient;
use crate::error::Result;
use futures::future::join_all;
use serde_json::Value;
use std::sync::LazyLock;

/// Inline HTML tags used for colour-coding in wiki tables.
static HTML_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"<[^>]+>").unwrap());

/// `{{api}}` / `{{cdn}}` style template tokens prefixed to documented routes.
static TEMPLATE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{\{[^}]*\}\}").unwrap());

/// Interpolation placeholders: `\(identifier)` or `(identifier)`.
static INTERP_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\\?\([^)]*\)").unwrap());

/// First path-like token in a cell: an absolute `/a/b` path, or a relative
/// `a/b` path with at least one slash. Bare single words don't match.
static PATH_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"/[A-Za-z0-9_][\w\-./]*|[A-Za-z0-9_]+(?:/[\w\-.]+)+").unwrap());

/// Documented version, requiring at least one dot (so "version 4" alone is ignored).
static VERSION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)version[:\s]+v?([0-9]+(?:\.[0-9]+)+)").unwrap());

/// First dotted-numeric run anywhere in a string (e.g. inside a tag like `release-4.9.10`).
static SEMVER_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[0-9]+(?:\.[0-9]+)+").unwrap());

/// Base64-ish secret blob (AES keys, encrypted tokens). Min length avoids
/// matching short obfuscated route segments.
static SECRET_B64_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Za-z0-9+/]{32,}={0,2}").unwrap());

/// UUID (app identifiers, device IDs).
static UUID_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"(?i)\b[0-9a-f]{8}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{4}-[0-9a-f]{12}\b")
        .unwrap()
});

/// Email addresses (service-account credentials).
static EMAIL_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"(?i)\b[a-z0-9._%+\-]+@[a-z0-9.\-]+\.[a-z]{2,}\b").unwrap());

/// A quoted, leading-slash path literal in source code, e.g. `return "/v3/user"`.
/// Group 1 is the raw path (interpolation normalized out later).
static LITERAL_PATH_RE: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r#"["'](/[A-Za-z0-9_][\w\-./{}()\\:]*)["']"#).unwrap()
});

/// `{identifier}` path-parameter placeholder.
static BRACE_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\{[^}]*\}").unwrap());

/// Seed queries to surface lines that define path literals when no routes file
/// is given. Each match's snippet is mined for `"/..."` literals.
const HARVEST_SEEDS: &[&str] = &["return \"/", "= \"/", ": \"/", "\"/v", "\"/api"];

/// Substrings (lowercased) that mark a documented route as deprecated / scheduled
/// for removal. Mix of English and the Russian annotations used in the spec wiki.
const DEPRECATION_MARKERS: &[&str] = &[
    "неиспольз",   // covers "не используется" and the misspelling "неиспользуеться"
    "не использ",
    "убрать",
    "почистить",
    "заглушка",
    "удалить",
    "moccasin",    // background-color used to grey-out deprecated rows
    "deprecated",
    "obsolete",
    "remove",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DocStatus {
    Active,
    Deprecated,
}

/// One route extracted from the spec.
#[derive(Debug, Clone)]
pub(crate) struct DocRoute {
    /// Human label from the left table cell, for report context.
    pub label: String,
    /// Normalized path: template token, query string and interpolation stripped.
    pub path: String,
    pub status: DocStatus,
    /// Code-search query, or None when the path is too generic to match reliably.
    pub query: Option<String>,
}

/// Parsed spec: documented version plus all routes (deduped by normalized path).
#[derive(Debug, Default)]
pub(crate) struct ParsedSpec {
    pub version: Option<String>,
    pub routes: Vec<DocRoute>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Verdict {
    /// Spec flags it for removal, but it's still wired in — the actionable backlog.
    CleanupDebt,
    /// Listed as active but missing from code — investigate (renamed or dropped).
    Drift,
    /// Flagged for removal and already gone — safe to delete the spec row.
    StaleDoc,
    /// Active and present.
    InSync,
    /// Path too generic to match reliably — surfaced for a human.
    NeedsReview,
}

/// Per-route audit result. This is the unit a local metadata map would persist.
#[derive(Debug)]
pub(crate) struct RouteAudit {
    pub route: DocRoute,
    /// (file, line) matches, capped.
    pub hits: Vec<(String, u64)>,
    pub verdict: Verdict,
}

impl Verdict {
    fn as_str(self) -> &'static str {
        match self {
            Verdict::CleanupDebt => "cleanup-debt",
            Verdict::Drift => "drift",
            Verdict::StaleDoc => "stale-doc",
            Verdict::InSync => "in-sync",
            Verdict::NeedsReview => "needs-review",
        }
    }
}

/// Classify a route from its documented status and whether code search found it.
pub(crate) fn classify(status: DocStatus, searchable: bool, in_code: bool) -> Verdict {
    if !searchable {
        return Verdict::NeedsReview;
    }
    match (status, in_code) {
        (DocStatus::Deprecated, true) => Verdict::CleanupDebt,
        (DocStatus::Deprecated, false) => Verdict::StaleDoc,
        (DocStatus::Active, false) => Verdict::Drift,
        (DocStatus::Active, true) => Verdict::InSync,
    }
}

/// Strip HTML tags, template tokens and interpolation from a cell, then extract
/// the first path-like token with its query string removed. Returns None when
/// the cell has no path (e.g. an absolute URL or prose).
pub(crate) fn normalize_path(cell: &str) -> Option<String> {
    // Skip absolute URLs — those live in LINKS sections, not the route surface.
    if cell.contains("://") {
        return None;
    }
    let no_html = HTML_RE.replace_all(cell, " ");
    let no_tpl = TEMPLATE_RE.replace_all(&no_html, " ");
    let no_interp = INTERP_RE.replace_all(&no_tpl, " ");
    let candidate = PATH_RE.find(&no_interp)?.as_str();
    // Drop query string and trailing punctuation/whitespace.
    let path = candidate.split('?').next().unwrap_or(candidate);
    let path = path.trim().trim_end_matches(['.', ',', '/', '\\', ')']);
    if path.is_empty() || path == "/" {
        None
    } else {
        Some(path.to_string())
    }
}

/// Build a code-search query for a normalized path, or None when it's too
/// generic to match reliably (avoids false "in sync" from common words).
/// Multi-segment paths use the last two segments — distinctive and robust to
/// prefix differences between doc and code.
pub(crate) fn route_search_query(path: &str) -> Option<String> {
    let segs: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
    match segs.as_slice() {
        [] => None,
        [one] => {
            // A single segment is reliable only when it's clearly distinctive.
            let distinctive =
                one.len() >= 6 && one.chars().any(|c| c == '-' || c == '_' || c.is_ascii_digit());
            distinctive.then(|| one.to_string())
        }
        _ => {
            let n = segs.len();
            Some(format!("{}/{}", segs[n - 2], segs[n - 1]))
        }
    }
}

/// Non-empty path segments.
fn path_segments(path: &str) -> Vec<&str> {
    path.split('/').filter(|s| !s.is_empty()).collect()
}

/// Match key for a path: its last two segments (or the single segment). Lets a
/// code path and a doc path match despite prefix differences.
pub(crate) fn path_key(path: &str) -> String {
    let s = path_segments(path);
    match s.as_slice() {
        [] => String::new(),
        [one] => one.to_string(),
        _ => format!("{}/{}", s[s.len() - 2], s[s.len() - 1]),
    }
}

/// First path segment (the API "namespace", e.g. `v3`).
fn first_segment(path: &str) -> Option<String> {
    path_segments(path).first().map(|s| s.to_string())
}

/// Normalize a path literal pulled from code: strip `\(interp)` / `{param}` and
/// query, trim trailing punctuation. Returns None if it isn't a usable path.
pub(crate) fn normalize_code_path(p: &str) -> Option<String> {
    let no_interp = INTERP_RE.replace_all(p, "");
    let no_brace = BRACE_RE.replace_all(&no_interp, "");
    let path = no_brace.split('?').next().unwrap_or(&no_brace);
    let path = path.trim().trim_end_matches(['/', '.', ':', '\\']);
    if path.len() >= 2 && path.starts_with('/') {
        Some(path.to_string())
    } else {
        None
    }
}

/// Extract deduped (path, 1-based line) for every quoted leading-slash literal
/// in a source file. The code-side endpoint inventory for reverse-drift.
pub(crate) fn harvest_path_literals(content: &str) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (i, line) in content.lines().enumerate() {
        for cap in LITERAL_PATH_RE.captures_iter(line) {
            if let Some(m) = cap.get(1) {
                if let Some(norm) = normalize_code_path(m.as_str()) {
                    if seen.insert(norm.clone()) {
                        out.push((norm, (i + 1) as u64));
                    }
                }
            }
        }
    }
    out
}

/// True if a row's text marks the route as deprecated.
fn is_deprecated(row_lower: &str) -> bool {
    DEPRECATION_MARKERS.iter().any(|m| row_lower.contains(m))
}

#[derive(PartialEq)]
enum Section {
    Other,
    Routes,
}

/// Parse a spec's markdown into version + routes. Only rows under the ROUTES
/// section become routes; the version is matched anywhere (it lives in META).
pub(crate) fn parse_spec(spec: &str) -> ParsedSpec {
    let version = VERSION_RE
        .captures(spec)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string());

    let mut routes: Vec<DocRoute> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut section = Section::Other;

    for line in spec.lines() {
        // Section headers are bolded cells: **META**, **ROUTES**, **PREPROD**, **LINKS**.
        let upper = line.to_uppercase();
        if upper.contains("**ROUTES**") {
            section = Section::Routes;
            continue;
        } else if upper.contains("**META**")
            || upper.contains("**PREPROD**")
            || upper.contains("**LINKS**")
        {
            section = Section::Other;
            continue;
        }
        if section != Section::Routes {
            continue;
        }
        if !line.contains('|') {
            continue;
        }

        let cells: Vec<&str> = line.split('|').map(str::trim).filter(|c| !c.is_empty()).collect();
        if cells.is_empty() {
            continue;
        }
        let label = HTML_RE.replace_all(cells[0], "").trim().to_string();
        let row_lower = line.to_lowercase();
        let deprecated = is_deprecated(&row_lower);

        // The route(s) live in the value cell(s). A cell can hold several paths
        // split by <br> or newlines (e.g. v2/v3 variants of the same endpoint).
        for cell in &cells[1..] {
            for fragment in cell.split("<br").flat_map(|f| f.split('\n')) {
                if let Some(path) = normalize_path(fragment) {
                    if seen.insert(path.clone()) {
                        routes.push(DocRoute {
                            label: label.clone(),
                            query: route_search_query(&path),
                            path,
                            status: if deprecated {
                                DocStatus::Deprecated
                            } else {
                                DocStatus::Active
                            },
                        });
                    }
                }
            }
        }
    }

    ParsedSpec { version, routes }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VersionVerdict {
    InSync,
    DocBehind,
    DocAhead,
    Unknown,
}

impl VersionVerdict {
    fn as_str(self) -> &'static str {
        match self {
            VersionVerdict::InSync => "in-sync",
            VersionVerdict::DocBehind => "doc-behind",
            VersionVerdict::DocAhead => "doc-ahead",
            VersionVerdict::Unknown => "unknown",
        }
    }
}

/// First dotted-numeric run in a string → its components. Tolerates any prefix:
/// "v4.9.5", "release-4.9.10", "app-v2.3.0" all parse. Requires ≥1 dot.
fn parse_semver(s: &str) -> Option<Vec<u64>> {
    let run = SEMVER_RE.find(s)?.as_str();
    let nums: Vec<u64> = run.split('.').filter_map(|p| p.parse::<u64>().ok()).collect();
    if nums.is_empty() { None } else { Some(nums) }
}

/// Compare the documented version against the repo's latest tag.
pub(crate) fn compare_versions(doc: Option<&str>, tag: Option<&str>) -> VersionVerdict {
    let (Some(d), Some(t)) = (doc.and_then(parse_semver), tag.and_then(parse_semver)) else {
        return VersionVerdict::Unknown;
    };
    let len = d.len().max(t.len());
    for i in 0..len {
        let dv = d.get(i).copied().unwrap_or(0);
        let tv = t.get(i).copied().unwrap_or(0);
        if dv < tv {
            return VersionVerdict::DocBehind;
        }
        if dv > tv {
            return VersionVerdict::DocAhead;
        }
    }
    VersionVerdict::InSync
}

// ─── Security: secrets pasted into the spec ───

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SecretKind {
    Base64Secret,
    Uuid,
    Email,
}

impl SecretKind {
    fn label(self) -> &'static str {
        match self {
            SecretKind::Base64Secret => "key/token",
            SecretKind::Uuid => "uuid",
            SecretKind::Email => "credential email",
        }
    }
}

/// A secret-shaped string found in the spec. `value` is used only for the code
/// cross-reference and is NEVER rendered — reports show `masked` instead, so a
/// report pasted into a ticket or chat doesn't re-leak the secret.
#[derive(Debug, Clone)]
pub(crate) struct SecretFinding {
    pub kind: SecretKind,
    pub value: String,
    pub masked: String,
}

#[derive(Debug)]
pub(crate) struct SecretAudit {
    pub finding: SecretFinding,
    /// Code locations where the same literal appears (hardcoded). Empty = doc-only.
    pub hardcoded_in: Vec<(String, u64)>,
}

/// Masked, display-safe preview of a secret — never the full value.
pub(crate) fn mask_secret(kind: SecretKind, v: &str) -> String {
    match kind {
        SecretKind::Email => match v.split_once('@') {
            Some((local, domain)) => {
                let head = local.chars().next().map(|c| c.to_string()).unwrap_or_default();
                format!("{head}***@{domain}")
            }
            None => "***".to_string(),
        },
        SecretKind::Uuid => {
            let head: String = v.chars().take(8).collect();
            format!("{head}…")
        }
        SecretKind::Base64Secret => {
            let head: String = v.chars().take(4).collect();
            let tail: String = {
                let t: Vec<char> = v.chars().rev().take(4).collect();
                t.into_iter().rev().collect()
            };
            format!("{head}…{tail} ({} chars)", v.len())
        }
    }
}

/// Distinctive substring to search the codebase for this secret.
fn secret_search_query(f: &SecretFinding) -> String {
    match f.kind {
        SecretKind::Base64Secret => f.value.chars().take(24).collect(),
        SecretKind::Uuid | SecretKind::Email => f.value.clone(),
    }
}

/// Extract secret-shaped strings (base64 keys/tokens, UUIDs, credential emails)
/// from the spec. Deduped by value.
pub(crate) fn extract_secrets(spec: &str) -> Vec<SecretFinding> {
    let mut out: Vec<SecretFinding> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let push = |kind: SecretKind, v: &str, key: String, seen: &mut std::collections::HashSet<String>, out: &mut Vec<SecretFinding>| {
        if seen.insert(key) {
            out.push(SecretFinding { kind, value: v.to_string(), masked: mask_secret(kind, v) });
        }
    };
    // UUIDs and emails are matched first; base64 cannot overlap them (dashes / '@').
    for m in UUID_RE.find_iter(spec) {
        push(SecretKind::Uuid, m.as_str(), m.as_str().to_lowercase(), &mut seen, &mut out);
    }
    for m in EMAIL_RE.find_iter(spec) {
        push(SecretKind::Email, m.as_str(), m.as_str().to_lowercase(), &mut seen, &mut out);
    }
    for m in SECRET_B64_RE.find_iter(spec) {
        let v = m.as_str();
        push(SecretKind::Base64Secret, v, v.to_string(), &mut seen, &mut out);
    }
    out
}

// ─── Local metadata map: persist each run, diff against the previous one ───
//
// After the first audit we persist a compact snapshot to `~/.gl-mcp/spec_maps/`.
// On the next run for the same project+ref we diff against it and surface what
// changed — routes that drifted or got fixed, the version verdict moving, secrets
// appearing or resolved. The file holds real internal route paths, so it lives
// in the user's data dir and is never committed.

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
struct RouteSnap {
    path: String,
    verdict: String,
}

#[derive(serde::Serialize, serde::Deserialize, Clone, PartialEq)]
struct SecretSnap {
    masked: String,
    kind: String,
    hardcoded: bool,
}

#[derive(serde::Serialize, serde::Deserialize, Default)]
pub(crate) struct SpecSnapshot {
    project_id: String,
    ref_name: String,
    scanned_at: String,
    version: Option<String>,
    version_verdict: String,
    routes: Vec<RouteSnap>,
    secrets: Vec<SecretSnap>,
    #[serde(default)]
    undocumented: Vec<String>,
}

/// `~/.gl-mcp/spec_maps/{project}__{ref}.json` (path separators sanitized).
fn map_path(project_id: &str, ref_name: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let safe = |s: &str| s.replace(['/', '\\', ' '], "_");
    std::path::PathBuf::from(home)
        .join(".gl-mcp")
        .join("spec_maps")
        .join(format!("{}__{}.json", safe(project_id), safe(ref_name)))
}

fn load_snapshot(project_id: &str, ref_name: &str) -> Option<SpecSnapshot> {
    let content = std::fs::read_to_string(map_path(project_id, ref_name)).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_snapshot(snap: &SpecSnapshot) -> std::io::Result<()> {
    let path = map_path(&snap.project_id, &snap.ref_name);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&path, serde_json::to_string_pretty(snap)?)
}

#[allow(clippy::too_many_arguments)]
fn build_snapshot(
    project_id: &str,
    ref_name: &str,
    scanned_at: String,
    version: Option<&str>,
    version_verdict: VersionVerdict,
    audits: &[RouteAudit],
    secrets: &[SecretAudit],
    undocumented: &[(String, u64, String)],
) -> SpecSnapshot {
    SpecSnapshot {
        project_id: project_id.to_string(),
        ref_name: ref_name.to_string(),
        scanned_at,
        version: version.map(String::from),
        version_verdict: version_verdict.as_str().to_string(),
        routes: audits
            .iter()
            .map(|a| RouteSnap {
                path: a.route.path.clone(),
                verdict: a.verdict.as_str().to_string(),
            })
            .collect(),
        secrets: secrets
            .iter()
            .map(|s| SecretSnap {
                masked: s.finding.masked.clone(),
                kind: s.finding.kind.label().to_string(),
                hardcoded: !s.hardcoded_in.is_empty(),
            })
            .collect(),
        undocumented: undocumented.iter().map(|(p, _, _)| p.clone()).collect(),
    }
}

/// Human-readable changes from `prev` to `new`. Empty when nothing material moved.
fn diff_snapshots(prev: &SpecSnapshot, new: &SpecSnapshot) -> Vec<String> {
    use std::collections::HashMap;
    let mut lines: Vec<String> = Vec::new();

    if prev.version_verdict != new.version_verdict {
        lines.push(format!(
            "version verdict: {} → {}",
            prev.version_verdict, new.version_verdict
        ));
    }

    let prev_routes: HashMap<&str, &str> =
        prev.routes.iter().map(|r| (r.path.as_str(), r.verdict.as_str())).collect();
    let new_routes: HashMap<&str, &str> =
        new.routes.iter().map(|r| (r.path.as_str(), r.verdict.as_str())).collect();
    for r in &new.routes {
        match prev_routes.get(r.path.as_str()) {
            None => lines.push(format!("+ route `{}` ({})", r.path, r.verdict)),
            Some(&pv) if pv != r.verdict => {
                lines.push(format!("~ route `{}`: {} → {}", r.path, pv, r.verdict))
            }
            _ => {}
        }
    }
    for r in &prev.routes {
        if !new_routes.contains_key(r.path.as_str()) {
            lines.push(format!("- route `{}` removed from spec", r.path));
        }
    }

    let prev_secrets: HashMap<&str, bool> =
        prev.secrets.iter().map(|s| (s.masked.as_str(), s.hardcoded)).collect();
    let new_secrets: HashMap<&str, bool> =
        new.secrets.iter().map(|s| (s.masked.as_str(), s.hardcoded)).collect();
    for s in &new.secrets {
        match prev_secrets.get(s.masked.as_str()) {
            None => lines.push(format!("+ secret `{}` ({})", s.masked, s.kind)),
            Some(&ph) if ph != s.hardcoded => lines.push(format!(
                "~ secret `{}`: hardcoded {} → {}",
                s.masked, ph, s.hardcoded
            )),
            _ => {}
        }
    }
    for s in &prev.secrets {
        if !new_secrets.contains_key(s.masked.as_str()) {
            lines.push(format!("- secret `{}` no longer in spec", s.masked));
        }
    }

    let prev_undoc: std::collections::HashSet<&str> =
        prev.undocumented.iter().map(String::as_str).collect();
    let new_undoc: std::collections::HashSet<&str> =
        new.undocumented.iter().map(String::as_str).collect();
    for p in &new.undocumented {
        if !prev_undoc.contains(p.as_str()) {
            lines.push(format!("+ undocumented endpoint `{p}` appeared in code"));
        }
    }
    for p in &prev.undocumented {
        if !new_undoc.contains(p.as_str()) {
            lines.push(format!("- undocumented endpoint `{p}` resolved (now documented or gone)"));
        }
    }

    lines
}

/// Raw blob-search results (each has `path`, `startline`, `data`).
async fn search_blobs(
    client: &GitLabClient,
    encoded: &str,
    query: &str,
    search_ref: &str,
    per_page: &str,
) -> Vec<Value> {
    let mut params: Vec<(&str, &str)> = vec![
        ("scope", "blobs"),
        ("search", query),
        ("per_page", per_page),
    ];
    if !search_ref.is_empty() {
        params.push(("ref", search_ref));
    }
    client
        .get(&format!("/projects/{encoded}/search"), &params)
        .await
        .unwrap_or_default()
}

/// Search one route in the repo, returning capped (file, line) hits.
async fn search_route(
    client: &GitLabClient,
    encoded: &str,
    query: &str,
    search_ref: &str,
) -> Vec<(String, u64)> {
    search_blobs(client, encoded, query, search_ref, "5")
        .await
        .iter()
        .take(5)
        .map(|r| {
            (
                r["path"].as_str().unwrap_or("?").to_string(),
                r["startline"].as_u64().unwrap_or(0),
            )
        })
        .collect()
}

/// Harvest path literals across the repo via seed searches, mining each result
/// snippet for `"/..."` literals. Returns (path, line, file). Noisier than a
/// dedicated routes file, so callers namespace-filter the output.
async fn harvest_via_search(
    client: &GitLabClient,
    encoded: &str,
    search_ref: &str,
) -> Vec<(String, u64, String)> {
    let futs = HARVEST_SEEDS.iter().map(|seed| {
        let client = client.clone();
        let encoded = encoded.to_string();
        let search_ref = search_ref.to_string();
        let seed = seed.to_string();
        async move { search_blobs(&client, &encoded, &seed, &search_ref, "20").await }
    });
    let batches = join_all(futs).await;

    let mut out: Vec<(String, u64, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for results in batches {
        for r in &results {
            let file = r["path"].as_str().unwrap_or("").to_string();
            let line = r["startline"].as_u64().unwrap_or(0);
            let data = r["data"].as_str().unwrap_or("");
            for cap in LITERAL_PATH_RE.captures_iter(data) {
                if let Some(m) = cap.get(1) {
                    if let Some(norm) = normalize_code_path(m.as_str()) {
                        if seen.insert(norm.clone()) {
                            out.push((norm, line, file.clone()));
                        }
                    }
                }
            }
        }
    }
    out
}

/// Audit a documented spec against a project's code: route drift + version drift.
pub async fn audit_spec_drift(
    client: &GitLabClient,
    project_id: &str,
    spec: &str,
    ref_name: &str,
    routes_file: &str,
    summary_only: bool,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    // Project metadata for links + default branch.
    let project: Value = client.get(&format!("/projects/{encoded}"), &[]).await?;
    let web_url = project["web_url"].as_str().unwrap_or("").to_string();
    let default_branch = project["default_branch"].as_str().unwrap_or("main");
    let search_ref = if ref_name.is_empty() { default_branch } else { ref_name };

    let parsed = parse_spec(spec);

    // Latest tag for the version check.
    let tags: Vec<Value> = client
        .get(
            &format!("/projects/{encoded}/repository/tags"),
            &[("per_page", "1"), ("order_by", "updated"), ("sort", "desc")],
        )
        .await
        .unwrap_or_default();
    let latest_tag = tags.first().and_then(|t| t["name"].as_str()).map(String::from);
    let version_verdict = compare_versions(parsed.version.as_deref(), latest_tag.as_deref());

    // Documented match-keys and namespaces, captured before `parsed.routes` is
    // consumed below — used for reverse-drift.
    let documented_keys: std::collections::HashSet<String> =
        parsed.routes.iter().map(|r| path_key(&r.path)).collect();
    let documented_ns: std::collections::HashSet<String> =
        parsed.routes.iter().filter_map(|r| first_segment(&r.path)).collect();

    // Search searchable routes concurrently (chunks of 10); non-searchable routes
    // go straight to NeedsReview with no API call.
    let mut audits: Vec<RouteAudit> = Vec::with_capacity(parsed.routes.len());
    let mut searchable: Vec<DocRoute> = Vec::new();
    for route in parsed.routes {
        if route.query.is_some() {
            searchable.push(route);
        } else {
            audits.push(RouteAudit {
                verdict: Verdict::NeedsReview,
                route,
                hits: Vec::new(),
            });
        }
    }

    for chunk in searchable.chunks(10) {
        let futs = chunk.iter().map(|route| {
            let client = client.clone();
            let encoded = encoded.to_string();
            let query = route.query.clone().unwrap_or_default();
            let search_ref = search_ref.to_string();
            let route = route.clone();
            async move {
                let hits = search_route(&client, &encoded, &query, &search_ref).await;
                let verdict = classify(route.status, true, !hits.is_empty());
                RouteAudit { route, hits, verdict }
            }
        });
        audits.extend(join_all(futs).await);
    }

    // Security: secrets pasted into the spec, cross-referenced against the code.
    let mut secret_audits: Vec<SecretAudit> = Vec::new();
    for chunk in extract_secrets(spec).chunks(10) {
        let futs = chunk.iter().map(|finding| {
            let client = client.clone();
            let encoded = encoded.to_string();
            let query = secret_search_query(finding);
            let search_ref = search_ref.to_string();
            let finding = finding.clone();
            async move {
                let hardcoded_in = search_route(&client, &encoded, &query, &search_ref).await;
                SecretAudit { finding, hardcoded_in }
            }
        });
        secret_audits.extend(join_all(futs).await);
    }

    // Reverse drift: endpoints in code the spec never documented. Build a
    // code-side inventory (from a named routes file if given, else harvested by
    // search) and subtract the documented paths by match-key.
    let (code_endpoints, harvest_mode): (Vec<(String, u64, String)>, &str) = if !routes_file.is_empty()
    {
        let content = crate::tools::commits::get_file_raw(client, project_id, routes_file, search_ref)
            .await
            .unwrap_or_default();
        let eps = harvest_path_literals(&content)
            .into_iter()
            .map(|(p, l)| (p, l, routes_file.to_string()))
            .collect();
        (eps, "file")
    } else {
        // Search-harvested literals are noisy — keep only paths in a namespace
        // the spec already documents (suppresses filesystem/asset junk).
        let eps = harvest_via_search(client, &encoded, search_ref)
            .await
            .into_iter()
            .filter(|(p, _, _)| {
                first_segment(p).map(|s| documented_ns.contains(&s)).unwrap_or(false)
            })
            .collect();
        (eps, "search")
    };
    let mut undocumented: Vec<(String, u64, String)> = code_endpoints
        .into_iter()
        .filter(|(p, _, _)| !documented_keys.contains(&path_key(p)))
        .collect();
    undocumented.sort_by(|a, b| a.0.cmp(&b.0));

    // Local metadata map: diff against the previous run, then persist this one.
    let prev = load_snapshot(project_id, search_ref);
    let scanned_at = chrono::Utc::now().format("%Y-%m-%d %H:%M UTC").to_string();
    let snapshot = build_snapshot(
        project_id,
        search_ref,
        scanned_at,
        parsed.version.as_deref(),
        version_verdict,
        &audits,
        &secret_audits,
        &undocumented,
    );
    let changes = prev.as_ref().map(|p| (p.scanned_at.clone(), diff_snapshots(p, &snapshot)));
    if let Err(e) = save_snapshot(&snapshot) {
        tracing::warn!("failed to persist spec map: {e}");
    }

    Ok(render_report(
        project_id,
        search_ref,
        &web_url,
        parsed.version.as_deref(),
        latest_tag.as_deref(),
        version_verdict,
        &audits,
        &secret_audits,
        &undocumented,
        harvest_mode,
        changes.as_ref(),
        summary_only,
    ))
}

fn version_line(
    doc: Option<&str>,
    tag: Option<&str>,
    verdict: VersionVerdict,
) -> String {
    let doc = doc.unwrap_or("?");
    let tag = tag.unwrap_or("none");
    match verdict {
        VersionVerdict::InSync => format!("In sync — spec `{doc}` matches latest tag `{tag}`."),
        VersionVerdict::DocBehind => {
            format!("**Spec is stale** — spec says `{doc}`, latest tag is `{tag}`. Refresh the spec.")
        }
        VersionVerdict::DocAhead => {
            format!("Spec `{doc}` is ahead of latest tag `{tag}` — unreleased, or tags lag.")
        }
        VersionVerdict::Unknown => {
            format!("Could not compare — spec version `{doc}`, latest tag `{tag}`.")
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn render_report(
    project_id: &str,
    search_ref: &str,
    web_url: &str,
    doc_version: Option<&str>,
    latest_tag: Option<&str>,
    version_verdict: VersionVerdict,
    audits: &[RouteAudit],
    secrets: &[SecretAudit],
    undocumented: &[(String, u64, String)],
    harvest_mode: &str,
    changes: Option<&(String, Vec<String>)>,
    summary_only: bool,
) -> String {
    let count = |v: Verdict| audits.iter().filter(|a| a.verdict == v).count();
    let cleanup = count(Verdict::CleanupDebt);
    let drift = count(Verdict::Drift);
    let stale = count(Verdict::StaleDoc);
    let in_sync = count(Verdict::InSync);
    let review = count(Verdict::NeedsReview);
    let hardcoded = secrets.iter().filter(|s| !s.hardcoded_in.is_empty()).count();

    if summary_only {
        let secrets_note = if secrets.is_empty() {
            String::new()
        } else {
            format!("; {} secrets ({hardcoded} hardcoded in code)", secrets.len())
        };
        let undoc_note = if undocumented.is_empty() {
            String::new()
        } else {
            format!("; {} undocumented", undocumented.len())
        };
        return format!(
            "{project_id}: version {}; {cleanup} cleanup-debt, {drift} drift, {stale} stale-doc, {in_sync} in-sync, {review} need-review (of {} routes){secrets_note}{undoc_note}.",
            match version_verdict {
                VersionVerdict::InSync => "in-sync",
                VersionVerdict::DocBehind => "STALE",
                VersionVerdict::DocAhead => "ahead",
                VersionVerdict::Unknown => "unknown",
            },
            audits.len(),
        );
    }

    let hit_link = |file: &str, line: u64| -> String {
        if web_url.is_empty() {
            format!("`{file}:{line}`")
        } else {
            format!("[`{file}:{line}`]({web_url}/-/blob/{search_ref}/{file}#L{line})")
        }
    };

    let mut out: Vec<String> = Vec::new();
    out.push(format!("# Spec-drift audit — {project_id}"));
    out.push(format!(
        "Ref `{search_ref}` · {} routes parsed from spec",
        audits.len()
    ));
    out.push(String::new());
    out.push("## Version".to_string());
    out.push(version_line(doc_version, latest_tag, version_verdict));

    // Changes since the previous audit (only when a prior snapshot existed).
    if let Some((since, lines)) = changes {
        out.push(String::new());
        out.push(format!("## Changes since last audit ({since})"));
        if lines.is_empty() {
            out.push("No changes.".to_string());
        } else {
            for l in lines {
                out.push(format!("- {l}"));
            }
        }
    }

    let section = |out: &mut Vec<String>, v: Verdict, title: &str, blurb: &str, show_hits: bool| {
        let rows: Vec<&RouteAudit> = audits.iter().filter(|a| a.verdict == v).collect();
        if rows.is_empty() {
            return;
        }
        out.push(String::new());
        out.push(format!("## {title} ({}) — {blurb}", rows.len()));
        for a in rows {
            let mut line = format!("- `{}`", a.route.path);
            if !a.route.label.is_empty() && a.route.label != a.route.path {
                line.push_str(&format!(" ({})", a.route.label));
            }
            if show_hits && !a.hits.is_empty() {
                let links: Vec<String> =
                    a.hits.iter().map(|(f, l)| hit_link(f, *l)).collect();
                line.push_str(&format!(" → {}", links.join(", ")));
            }
            out.push(line);
        }
    };

    section(
        &mut out,
        Verdict::CleanupDebt,
        "Cleanup debt",
        "spec flags for removal, still wired in code",
        true,
    );
    section(
        &mut out,
        Verdict::Drift,
        "Drift",
        "spec lists as active, missing from code",
        false,
    );
    section(
        &mut out,
        Verdict::StaleDoc,
        "Stale doc rows",
        "flagged for removal and already gone — safe to delete from the spec",
        false,
    );
    section(
        &mut out,
        Verdict::NeedsReview,
        "Needs review",
        "path too generic to match reliably — check by hand",
        false,
    );

    // Security: secrets pasted into the spec. Hardcoded-in-code first (worse).
    if !secrets.is_empty() {
        out.push(String::new());
        out.push(format!(
            "## Security ({}) — secrets in the spec doc",
            secrets.len()
        ));
        out.push(
            "Secret material in an org-readable doc. Rotate and restrict access; values below are masked."
                .to_string(),
        );
        let mut ordered: Vec<&SecretAudit> = secrets.iter().collect();
        ordered.sort_by_key(|s| s.hardcoded_in.is_empty()); // hardcoded (false) first
        for s in ordered {
            let kind = s.finding.kind.label();
            if s.hardcoded_in.is_empty() {
                out.push(format!(
                    "- `{}` [{kind}] — doc-only leak; rotate the secret and restrict the doc.",
                    s.finding.masked
                ));
            } else {
                let loc = s
                    .hardcoded_in
                    .iter()
                    .map(|(f, l)| hit_link(f, *l))
                    .collect::<Vec<_>>()
                    .join(", ");
                out.push(format!(
                    "- `{}` [{kind}] — **also hardcoded in code** at {loc}; rotate AND remove from code.",
                    s.finding.masked
                ));
            }
        }
    }

    // Reverse drift: endpoints in code that the spec never documented.
    if !undocumented.is_empty() {
        out.push(String::new());
        out.push(format!(
            "## Undocumented endpoints ({}) — in code, not in the spec",
            undocumented.len()
        ));
        if harvest_mode == "search" {
            out.push("Harvested by search within documented namespaces; pass `routes_file` (the file that defines the routes) for full coverage including new namespaces.".to_string());
        }
        for (path, line, file) in undocumented {
            out.push(format!("- `{path}` → {}", hit_link(file, *line)));
        }
    }

    out.push(String::new());
    out.push(format!("## In sync ({in_sync})"));
    let synced: Vec<&RouteAudit> =
        audits.iter().filter(|a| a.verdict == Verdict::InSync).collect();
    for a in synced {
        out.push(format!("- `{}`", a.route.path));
    }

    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_path_strips_template_and_query() {
        assert_eq!(
            normalize_path("<span>{{api}}</span>/v3/user"),
            Some("/v3/user".to_string())
        );
        assert_eq!(
            normalize_path("{{api}}/v3/feedbacks?type=1"),
            Some("/v3/feedbacks".to_string())
        );
        assert_eq!(
            normalize_path("/app/foo/version  (remove and clean up)"),
            Some("/app/foo/version".to_string())
        );
        assert_eq!(normalize_path("token/refresh"), Some("token/refresh".to_string()));
    }

    #[test]
    fn test_normalize_path_skips_urls_and_prose() {
        assert_eq!(normalize_path("https://example.com/policy"), None);
        assert_eq!(normalize_path("just some prose"), None);
    }

    #[test]
    fn test_route_search_query_distinctiveness() {
        // multi-segment → last two
        assert_eq!(
            route_search_query("/user/nodes-pools/favorites"),
            Some("nodes-pools/favorites".to_string())
        );
        assert_eq!(route_search_query("/v3/user"), Some("v3/user".to_string()));
        // single distinctive segment (has a hyphen)
        assert_eq!(route_search_query("/nodes-list"), Some("nodes-list".to_string()));
        // single common word → too generic
        assert_eq!(route_search_query("/login"), None);
        assert_eq!(route_search_query("/register"), None);
    }

    #[test]
    fn test_classify_matrix() {
        assert_eq!(classify(DocStatus::Deprecated, true, true), Verdict::CleanupDebt);
        assert_eq!(classify(DocStatus::Deprecated, true, false), Verdict::StaleDoc);
        assert_eq!(classify(DocStatus::Active, true, false), Verdict::Drift);
        assert_eq!(classify(DocStatus::Active, true, true), Verdict::InSync);
        assert_eq!(classify(DocStatus::Active, false, false), Verdict::NeedsReview);
    }

    #[test]
    fn test_parse_spec_sections_and_status() {
        // Routes are only parsed under the ROUTES section; LINKS URLs are ignored.
        let spec = "\
| **META** | Version: 4.9.5 |
| **ROUTES** |  |
| Login | {{api}}/login |
| User | {{api}}/v3/user |
| Update | /app/foo/version (убрать и почистить компоненты) |
| **LINKS** |  |
| Policy | https://example.com/policy |";
        let parsed = parse_spec(spec);
        assert_eq!(parsed.version.as_deref(), Some("4.9.5"));
        // /login, /v3/user, /app/foo/version — policy URL excluded
        assert_eq!(parsed.routes.len(), 3);
        let update = parsed.routes.iter().find(|r| r.path == "/app/foo/version").unwrap();
        assert_eq!(update.status, DocStatus::Deprecated);
        let user = parsed.routes.iter().find(|r| r.path == "/v3/user").unwrap();
        assert_eq!(user.status, DocStatus::Active);
        // /login is active but too generic → no query
        let login = parsed.routes.iter().find(|r| r.path == "/login").unwrap();
        assert!(login.query.is_none());
    }

    #[test]
    fn test_parse_spec_splits_multi_path_cell() {
        let spec = "\
| **ROUTES** |  |
| Feedback reasons | /feedbacks/reasons<br />/v3/feedbacks/reasons |";
        let parsed = parse_spec(spec);
        let paths: Vec<&str> = parsed.routes.iter().map(|r| r.path.as_str()).collect();
        assert!(paths.contains(&"/feedbacks/reasons"));
        assert!(paths.contains(&"/v3/feedbacks/reasons"));
    }

    #[test]
    fn test_compare_versions() {
        assert_eq!(compare_versions(Some("4.9.5"), Some("4.9.5")), VersionVerdict::InSync);
        assert_eq!(compare_versions(Some("4.9.5"), Some("v5.0.0")), VersionVerdict::DocBehind);
        assert_eq!(compare_versions(Some("5.1.0"), Some("5.0.9")), VersionVerdict::DocAhead);
        assert_eq!(compare_versions(Some("4.9"), Some("4.9.0")), VersionVerdict::InSync);
        assert_eq!(compare_versions(None, Some("1.0.0")), VersionVerdict::Unknown);
    }

    #[test]
    fn test_compare_versions_tag_prefix() {
        // Tags carry prefixes — the numeric run must still be found. (Regression:
        // a live run saw `release-4.9.10` come back Unknown.)
        assert_eq!(compare_versions(Some("4.9.5"), Some("release-4.9.10")), VersionVerdict::DocBehind);
        assert_eq!(compare_versions(Some("2.3.0"), Some("app-v2.3.0")), VersionVerdict::InSync);
        // 4.9.5 vs 4.9.10 must compare numerically, not lexically (5 < 10).
        assert_eq!(compare_versions(Some("4.9.10"), Some("4.9.5")), VersionVerdict::DocAhead);
    }

    #[test]
    fn test_extract_secrets() {
        // Generic fakes — never real values.
        let spec = "\
key: AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIII0000=
id: 12345678-90ab-cdef-1234-567890abcdef
contact: svc-account@example.com
short: abc123";
        let secrets = extract_secrets(spec);
        let kinds: Vec<SecretKind> = secrets.iter().map(|s| s.kind).collect();
        assert!(kinds.contains(&SecretKind::Base64Secret));
        assert!(kinds.contains(&SecretKind::Uuid));
        assert!(kinds.contains(&SecretKind::Email));
        // "short: abc123" is below the base64 threshold → not a secret.
        assert_eq!(secrets.len(), 3);
    }

    #[test]
    fn test_diff_snapshots() {
        let prev = SpecSnapshot {
            version_verdict: "STALE".into(),
            routes: vec![
                RouteSnap { path: "/a".into(), verdict: "drift".into() },
                RouteSnap { path: "/b".into(), verdict: "in-sync".into() },
                RouteSnap { path: "/gone".into(), verdict: "in-sync".into() },
            ],
            secrets: vec![SecretSnap { masked: "x…y".into(), kind: "uuid".into(), hardcoded: false }],
            undocumented: vec!["/v4/old".into()],
            ..Default::default()
        };
        let new = SpecSnapshot {
            version_verdict: "in-sync".into(),
            routes: vec![
                RouteSnap { path: "/a".into(), verdict: "in-sync".into() }, // drift fixed
                RouteSnap { path: "/b".into(), verdict: "in-sync".into() }, // unchanged
                RouteSnap { path: "/new".into(), verdict: "drift".into() }, // added
            ],
            secrets: vec![SecretSnap { masked: "x…y".into(), kind: "uuid".into(), hardcoded: true }], // now hardcoded
            undocumented: vec!["/v4/shadow".into()], // /v4/old resolved, /v4/shadow new
            ..Default::default()
        };
        let d = diff_snapshots(&prev, &new);
        assert!(d.iter().any(|l| l.contains("version verdict") && l.contains("in-sync")));
        assert!(d.iter().any(|l| l.contains("`/a`") && l.contains("drift → in-sync")));
        assert!(d.iter().any(|l| l.contains("+ route `/new`")));
        assert!(d.iter().any(|l| l.contains("`/gone` removed")));
        assert!(d.iter().any(|l| l.contains("secret") && l.contains("hardcoded false → true")));
        assert!(d.iter().any(|l| l.contains("+ undocumented endpoint `/v4/shadow`")));
        assert!(d.iter().any(|l| l.contains("`/v4/old` resolved")));
        // unchanged /b must not appear
        assert!(!d.iter().any(|l| l.contains("`/b`")));
    }

    #[test]
    fn test_harvest_path_literals() {
        // Swift-ish endpoint enum plus a non-route literal.
        let code = r#"
        case .getUserInfo:
            return "/v3/user"
        case .hotspots:
            return "/hotspots/\(identifier)"
        case .devices:
            return "/v4/user/devices"
        let asset = "/Users/dev/Library/thing.png"
        "#;
        let eps = harvest_path_literals(code);
        let paths: Vec<&str> = eps.iter().map(|(p, _)| p.as_str()).collect();
        assert!(paths.contains(&"/v3/user"));
        assert!(paths.contains(&"/hotspots")); // interpolation segment stripped
        assert!(paths.contains(&"/v4/user/devices"));
        // line numbers are 1-based and populated
        assert!(eps.iter().all(|(_, l)| *l > 0));
    }

    #[test]
    fn test_path_key_matching() {
        // last-two-segments key tolerates prefix differences
        assert_eq!(path_key("/v3/user"), "v3/user");
        assert_eq!(path_key("/user/nodes-pools/favorites"), "nodes-pools/favorites");
        assert_eq!(path_key("/login"), "login");
        // a code path and a doc path with the same tail match
        assert_eq!(path_key("/api/v3/user"), path_key("/v3/user"));
    }

    #[test]
    fn test_normalize_code_path() {
        assert_eq!(normalize_code_path("/v3/user"), Some("/v3/user".to_string()));
        assert_eq!(normalize_code_path("/hotspots/\\(identifier)"), Some("/hotspots".to_string()));
        assert_eq!(normalize_code_path("/orders?currency=USD"), Some("/orders".to_string()));
        assert_eq!(normalize_code_path("/users/{id}"), Some("/users".to_string()));
        assert_eq!(normalize_code_path("notapath"), None);
    }

    #[test]
    fn test_mask_secret_never_reveals_full_value() {
        let key = "AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHHIIII0000=";
        let masked = mask_secret(SecretKind::Base64Secret, key);
        assert!(!masked.contains("BBBBCCCC"));
        assert!(masked.contains("chars"));
        assert_eq!(mask_secret(SecretKind::Email, "svc@example.com"), "s***@example.com");
        assert_eq!(mask_secret(SecretKind::Uuid, "12345678-90ab-cdef-1234-567890abcdef"), "12345678…");
    }
}
