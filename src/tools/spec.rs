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

/// A run of 2+ string literals joined by `+` — a path split across fragments,
/// e.g. `"/v3" + "/user"`. Captures the whole run; literals mined separately.
static CONCAT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#""[^"]*"(?:\s*\+\s*"[^"]*")+"#).unwrap());

/// A single double-quoted string literal; group 1 is its contents.
static STRING_LIT_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r#""([^"]*)""#).unwrap());

/// Two or more consecutive slashes (left after stripping an interpolated segment).
static MULTISLASH_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"/{2,}").unwrap());

/// Seed queries to surface lines that define path literals when no routes file
/// is given. Each match's snippet is mined for path literals. Both double- and
/// single-quote forms are seeded so single-quote languages (PHP/Laravel, Ruby,
/// Python) aren't missed, plus call-style forms like `Route::get('/...')`.
const HARVEST_SEEDS: &[&str] = &[
    "return \"/", "= \"/", ": \"/", "\"/v", "\"/api",
    "return '/", "= '/", ": '/", "'/v", "'/api",
    "('/", "(\"/",
];

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

/// Normalize a path literal pulled from code: strip `\(interp)` / `{param}`
/// segments, collapse the resulting double slashes, drop the query, trim
/// trailing punctuation. Returns None if it isn't a usable path.
pub(crate) fn normalize_code_path(p: &str) -> Option<String> {
    let no_interp = INTERP_RE.replace_all(p, "");
    let no_brace = BRACE_RE.replace_all(&no_interp, "");
    let collapsed = MULTISLASH_RE.replace_all(&no_brace, "/");
    let path = collapsed.split('?').next().unwrap_or(&collapsed);
    let path = path.trim().trim_end_matches(['/', '.', ':', '\\']);
    if path.len() >= 2 && path.starts_with('/') {
        Some(path.to_string())
    } else {
        None
    }
}

/// Extract deduped (path, 1-based line) endpoints from a source file. Handles
/// both whole path literals (`"/v3/user"`) and paths split across a `+`-joined
/// run of fragments (`"/v3" + "/user"`), so fragment-assembled routes aren't
/// missed or split into bogus pieces.
pub(crate) fn harvest_path_literals(content: &str) -> Vec<(String, u64)> {
    let mut out: Vec<(String, u64)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut emit = |norm: String, line: u64, out: &mut Vec<(String, u64)>, seen: &mut std::collections::HashSet<String>| {
        if seen.insert(norm.clone()) {
            out.push((norm, line));
        }
    };
    for (i, line) in content.lines().enumerate() {
        let lineno = (i + 1) as u64;

        // 1. Concatenation runs: stitch the literal fragments into one path.
        let mut consumed: Vec<(usize, usize)> = Vec::new();
        for run in CONCAT_RE.find_iter(line) {
            let joined: String = STRING_LIT_RE
                .captures_iter(run.as_str())
                .map(|c| c.get(1).map(|m| m.as_str()).unwrap_or(""))
                .collect();
            if let Some(norm) = normalize_code_path(&joined) {
                emit(norm, lineno, &mut out, &mut seen);
            }
            consumed.push((run.start(), run.end()));
        }

        // 2. Standalone leading-slash literals outside any consumed run.
        for cap in LITERAL_PATH_RE.captures_iter(line) {
            let whole = cap.get(0).unwrap();
            if consumed.iter().any(|(s, e)| whole.start() >= *s && whole.end() <= *e) {
                continue;
            }
            if let Some(m) = cap.get(1) {
                if let Some(norm) = normalize_code_path(m.as_str()) {
                    emit(norm, lineno, &mut out, &mut seen);
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
        if looks_like_b64_secret(v) {
            push(SecretKind::Base64Secret, v, v.to_string(), &mut seen, &mut out);
        }
    }
    out
}

/// Distinguish a real base64 key/token from a slash-delimited route path, which
/// the base64 character class (`/` included) otherwise matches. Real keys/tokens
/// carry base64 padding (`=`) or a `+`, or have no `/` at all; a long string
/// that's all `word/word/word` with neither is a path, not a secret.
fn looks_like_b64_secret(s: &str) -> bool {
    s.contains('=') || s.contains('+') || !s.contains('/')
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

/// Reduce a caller-controlled token to a single, traversal-proof filename
/// component: keep only `[A-Za-z0-9._-]`, map everything else (path separators
/// included) to `_`, and strip leading `.` so the result can neither carry a
/// separator nor form a `.`/`..` dotfile. Allowlist by construction (CWE-22
/// defense-in-depth) rather than a denylist of "known-bad" characters.
fn safe_component(s: &str) -> String {
    let mapped: String = s
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
        .collect();
    let trimmed = mapped.trim_start_matches('.');
    if trimmed.is_empty() { "_".to_string() } else { trimmed.to_string() }
}

/// `~/.gl-mcp/spec_maps/{project}__{ref}[__{key}].json`. Every caller-controlled
/// token is reduced to a safe filename component (see [`safe_component`]), so the
/// snapshot always lands inside `spec_maps/` regardless of input — no path
/// traversal is possible.
///
/// `map_key` disambiguates multiple specs audited against the same project+ref
/// (e.g. Windows and macOS specs both targeting one desktop repo in a sweep), so
/// their snapshots — and their "changes since last audit" history — don't collide.
fn map_path(project_id: &str, ref_name: &str, map_key: &str) -> std::path::PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    let name = if map_key.is_empty() {
        format!("{}__{}.json", safe_component(project_id), safe_component(ref_name))
    } else {
        format!(
            "{}__{}__{}.json",
            safe_component(project_id),
            safe_component(ref_name),
            safe_component(map_key)
        )
    };
    std::path::PathBuf::from(home).join(".gl-mcp").join("spec_maps").join(name)
}

fn load_snapshot(project_id: &str, ref_name: &str, map_key: &str) -> Option<SpecSnapshot> {
    let content = std::fs::read_to_string(map_path(project_id, ref_name, map_key)).ok()?;
    serde_json::from_str(&content).ok()
}

fn save_snapshot(snap: &SpecSnapshot, map_key: &str) -> std::io::Result<()> {
    let path = map_path(&snap.project_id, &snap.ref_name, map_key);
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

/// Files to harvest from one routes-file resolution are capped, to bound API
/// calls when a directory entry expands to many blobs.
const ROUTES_FILE_CAP: usize = 60;

/// Skip clearly non-source files when expanding a directory entry — the literal
/// harvester would find nothing useful in them anyway, and they cost a fetch.
fn is_code_file(path: &str) -> bool {
    const SKIP: &[&str] = &[
        ".png", ".jpg", ".jpeg", ".gif", ".svg", ".ico", ".webp", ".bmp", ".pdf",
        ".lock", ".zip", ".gz", ".tar", ".woff", ".woff2", ".ttf", ".eot", ".otf",
        ".mp4", ".mp3", ".wav", ".bin", ".so", ".a", ".o", ".class", ".jar",
        ".keystore", ".jks", ".p12", ".ipa", ".apk", ".dmg",
    ];
    let lower = path.to_lowercase();
    !SKIP.iter().any(|ext| lower.ends_with(ext))
}

/// Harvest path literals from several (file, content) pairs, deduped by path
/// (first file wins), each tagged with its source file. Pure — testable.
pub(crate) fn harvest_multi(files: &[(String, String)]) -> Vec<(String, u64, String)> {
    let mut out: Vec<(String, u64, String)> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (fpath, content) in files {
        for (p, line) in harvest_path_literals(content) {
            if seen.insert(p.clone()) {
                out.push((p, line, fpath.clone()));
            }
        }
    }
    out
}

/// Resolve a `routes_file` value into (path, content) pairs to harvest.
/// Accepts a comma-separated list; each entry is either a file (fetched as-is)
/// or a directory (every code file under it, recursively). Fetches concurrently.
async fn resolve_routes_files(
    client: &GitLabClient,
    project_id: &str,
    search_ref: &str,
    routes_file: &str,
) -> Vec<(String, String)> {
    let encoded = urlencoding::encode(project_id);

    // Build the flat list of file paths to fetch.
    let mut paths: Vec<String> = Vec::new();
    for entry in routes_file.split(',').map(str::trim).filter(|e| !e.is_empty()) {
        let mut params: Vec<(&str, &str)> =
            vec![("path", entry), ("recursive", "true"), ("per_page", "100")];
        if !search_ref.is_empty() {
            params.push(("ref", search_ref));
        }
        let tree: Vec<Value> = client
            .get(&format!("/projects/{encoded}/repository/tree"), &params)
            .await
            .unwrap_or_default();
        let blobs: Vec<String> = tree
            .iter()
            .filter(|t| t["type"].as_str() == Some("blob"))
            .filter_map(|t| t["path"].as_str())
            .filter(|p| is_code_file(p))
            .map(String::from)
            .collect();
        if blobs.is_empty() {
            // Not a directory (or empty) — treat the entry as a single file.
            paths.push(entry.to_string());
        } else {
            paths.extend(blobs);
        }
    }
    if paths.len() > ROUTES_FILE_CAP {
        tracing::warn!("routes_file expanded to {} files; capping at {ROUTES_FILE_CAP}", paths.len());
        paths.truncate(ROUTES_FILE_CAP);
    }

    // Fetch concurrently (chunks of 10), keeping only files that exist.
    let mut out: Vec<(String, String)> = Vec::new();
    for chunk in paths.chunks(10) {
        let futs = chunk.iter().map(|path| {
            let client = client.clone();
            let project = project_id.to_string();
            let path = path.clone();
            let search_ref = search_ref.to_string();
            async move {
                crate::tools::commits::get_file_raw(&client, &project, &path, &search_ref)
                    .await
                    .map(|c| (path, c))
            }
        });
        out.extend(join_all(futs).await.into_iter().flatten());
    }
    out
}

/// Everything an audit produces — shared by the markdown and HTML renderers.
pub(crate) struct AuditOutcome {
    project_id: String,
    search_ref: String,
    web_url: String,
    doc_version: Option<String>,
    latest_tag: Option<String>,
    version_verdict: VersionVerdict,
    audits: Vec<RouteAudit>,
    secrets: Vec<SecretAudit>,
    undocumented: Vec<(String, u64, String)>,
    harvest_mode: String,
    changes: Option<(String, Vec<String>)>,
}

/// Run the full audit (routes + version + security + reverse-drift) and persist
/// the metadata-map snapshot. Pure computation — no presentation.
pub(crate) async fn compute_audit(
    client: &GitLabClient,
    project_id: &str,
    spec: &str,
    ref_name: &str,
    routes_file: &str,
    map_key: &str,
) -> Result<AuditOutcome> {
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
        // routes_file may be a single file, a comma-separated list, or a directory
        // (expanded to every code file under it). Harvest all, dedup by path.
        let files = resolve_routes_files(client, project_id, search_ref, routes_file).await;
        (harvest_multi(&files), "file")
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
    let prev = load_snapshot(project_id, search_ref, map_key);
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
    if let Err(e) = save_snapshot(&snapshot, map_key) {
        tracing::warn!("failed to persist spec map: {e}");
    }

    Ok(AuditOutcome {
        project_id: project_id.to_string(),
        search_ref: search_ref.to_string(),
        web_url,
        doc_version: parsed.version,
        latest_tag,
        version_verdict,
        audits,
        secrets: secret_audits,
        undocumented,
        harvest_mode: harvest_mode.to_string(),
        changes,
    })
}

/// Audit a documented spec against a project's code: markdown report.
pub async fn audit_spec_drift(
    client: &GitLabClient,
    project_id: &str,
    spec: &str,
    ref_name: &str,
    routes_file: &str,
    summary_only: bool,
) -> Result<String> {
    let o = compute_audit(client, project_id, spec, ref_name, routes_file, "").await?;
    Ok(render_report(
        &o.project_id,
        &o.search_ref,
        &o.web_url,
        o.doc_version.as_deref(),
        o.latest_tag.as_deref(),
        o.version_verdict,
        &o.audits,
        &o.secrets,
        &o.undocumented,
        &o.harvest_mode,
        o.changes.as_ref(),
        summary_only,
    ))
}

/// Audit a documented spec against a project's code: HTML report.
pub async fn generate_spec_audit_report(
    client: &GitLabClient,
    project_id: &str,
    spec: &str,
    ref_name: &str,
    routes_file: &str,
) -> Result<String> {
    let o = compute_audit(client, project_id, spec, ref_name, routes_file, "").await?;
    Ok(render_html(&o))
}

/// One platform to audit in a sweep.
pub struct SweepTarget {
    pub label: String,
    pub project_id: String,
    pub spec: String,
    pub ref_name: String,
    pub routes_file: String,
}

/// Platforms run concurrently, but capped — each audit already fans out ~10
/// sub-requests internally, so this bounds total in-flight load on the API.
const SWEEP_CONCURRENCY: usize = 3;

/// One row of the cross-platform rollup.
struct SweepRow {
    label: String,
    error: Option<String>,
    version: String,
    cleanup: usize,
    drift: usize,
    stale: usize,
    undoc: usize,
    /// Reverse-drift was search-harvested (namespace-gated, approximate) rather
    /// than read from a routes file — undoc count is a lower bound.
    approx: bool,
    secrets: usize,
    hardcoded: usize,
    in_sync: usize,
}

fn version_cell(o: &AuditOutcome) -> String {
    match o.version_verdict {
        VersionVerdict::DocBehind => format!(
            "STALE ({}<{})",
            o.doc_version.as_deref().unwrap_or("?"),
            o.latest_tag.as_deref().unwrap_or("?")
        ),
        VersionVerdict::InSync => "in-sync".to_string(),
        VersionVerdict::DocAhead => "ahead".to_string(),
        VersionVerdict::Unknown => "unknown".to_string(),
    }
}

fn sweep_row(label: &str, o: &AuditOutcome) -> SweepRow {
    let count = |v: Verdict| o.audits.iter().filter(|a| a.verdict == v).count();
    SweepRow {
        label: label.to_string(),
        error: None,
        version: version_cell(o),
        cleanup: count(Verdict::CleanupDebt),
        drift: count(Verdict::Drift),
        stale: count(Verdict::StaleDoc),
        undoc: o.undocumented.len(),
        approx: o.harvest_mode == "search",
        secrets: o.secrets.len(),
        hardcoded: o.secrets.iter().filter(|s| !s.hardcoded_in.is_empty()).count(),
        in_sync: count(Verdict::InSync),
    }
}

/// Render the cross-platform rollup. Pure — unit-tested without the network.
fn render_sweep(rows: &[SweepRow], summary_only: bool) -> String {
    let mut out: Vec<String> = Vec::new();
    out.push(format!("# Cross-platform spec-drift sweep — {} platforms", rows.len()));
    out.push(String::new());
    out.push("| Platform | Version | Cleanup | Drift | Stale | Undoc | Secrets | In-sync |".to_string());
    out.push("|----------|---------|---------|-------|-------|-------|---------|---------|".to_string());
    for r in rows {
        if let Some(e) = &r.error {
            out.push(format!("| {} | failed: {} | | | | | | |", r.label, e));
        } else {
            let undoc = if r.approx { format!("{}~", r.undoc) } else { r.undoc.to_string() };
            out.push(format!(
                "| {} | {} | {} | {} | {} | {} | {} ({} hc) | {} |",
                r.label, r.version, r.cleanup, r.drift, r.stale, undoc, r.secrets, r.hardcoded, r.in_sync
            ));
        }
    }
    if rows.iter().any(|r| r.error.is_none() && r.approx) {
        out.push(String::new());
        out.push("`~` = reverse-drift search-harvested (namespace-gated, lower bound); pass a `routes_file` per platform for a precise count.".to_string());
    }

    if summary_only {
        return out.join("\n");
    }

    // Needs-attention: platforms with actionable findings.
    let flagged: Vec<&SweepRow> = rows
        .iter()
        .filter(|r| r.error.is_none() && (r.cleanup > 0 || r.drift > 0 || r.hardcoded > 0 || r.version.starts_with("STALE")))
        .collect();
    if !flagged.is_empty() {
        out.push(String::new());
        out.push("## Needs attention".to_string());
        for r in flagged {
            let mut notes: Vec<String> = Vec::new();
            if r.version.starts_with("STALE") {
                notes.push(format!("version {}", r.version));
            }
            if r.cleanup > 0 {
                notes.push(format!("{} cleanup-debt", r.cleanup));
            }
            if r.drift > 0 {
                notes.push(format!("{} drift", r.drift));
            }
            if r.hardcoded > 0 {
                notes.push(format!("{} hardcoded secret(s)", r.hardcoded));
            }
            out.push(format!("- **{}**: {}", r.label, notes.join(", ")));
        }
    }

    // Totals across platforms that audited successfully.
    let ok: Vec<&SweepRow> = rows.iter().filter(|r| r.error.is_none()).collect();
    let sum = |f: fn(&SweepRow) -> usize| ok.iter().map(|r| f(r)).sum::<usize>();
    out.push(String::new());
    out.push("## Totals".to_string());
    out.push(format!(
        "Across {} platforms: {} cleanup-debt, {} drift, {} stale-doc, {} undocumented, {} secrets ({} hardcoded).",
        ok.len(),
        sum(|r| r.cleanup),
        sum(|r| r.drift),
        sum(|r| r.stale),
        sum(|r| r.undoc),
        sum(|r| r.secrets),
        sum(|r| r.hardcoded),
    ));
    let failed = rows.len() - ok.len();
    if failed > 0 {
        out.push(format!("{failed} platform(s) failed to audit — see the table."));
    }

    out.join("\n")
}

/// Audit several specs against their repos concurrently, rolled up into one
/// cross-platform table. Each target audits independently (and persists its own
/// metadata-map snapshot via compute_audit); a failure on one platform doesn't
/// sink the others.
pub async fn sweep_spec_audit(
    client: &GitLabClient,
    targets: &[SweepTarget],
    summary_only: bool,
) -> Result<String> {
    let mut rows: Vec<SweepRow> = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(SWEEP_CONCURRENCY) {
        let futs = chunk.iter().map(|t| {
            let client = client.clone();
            async move {
                // Disambiguate the metadata-map snapshot by label so platforms
                // sharing a repo+ref (e.g. Windows & macOS desktop) don't collide.
                let r = compute_audit(&client, &t.project_id, &t.spec, &t.ref_name, &t.routes_file, &t.label).await;
                (t.label.clone(), r)
            }
        });
        for (label, result) in join_all(futs).await {
            match result {
                Ok(o) => rows.push(sweep_row(&label, &o)),
                Err(e) => rows.push(SweepRow {
                    label,
                    error: Some(e.short_message()),
                    version: String::new(),
                    cleanup: 0,
                    drift: 0,
                    stale: 0,
                    undoc: 0,
                    approx: false,
                    secrets: 0,
                    hardcoded: 0,
                    in_sync: 0,
                }),
            }
        }
    }
    Ok(render_sweep(&rows, summary_only))
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

/// Dark-theme styles shared with the other HTML reports (kept as a plain const
/// so no format-brace escaping is needed).
const SPEC_STYLE: &str = r#"*{margin:0;padding:0;box-sizing:border-box}
body{font-family:-apple-system,BlinkMacSystemFont,'Segoe UI',Roboto,sans-serif;background:#0d1117;color:#c9d1d9;padding:32px;line-height:1.6}
h1{color:#58a6ff;margin-bottom:8px;font-size:24px}
h2{color:#58a6ff;margin:36px 0 16px;font-size:18px;border-bottom:1px solid #21262d;padding-bottom:8px}
.sub{color:#8b949e;margin-bottom:24px;font-size:14px}
.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(150px,1fr));gap:14px;margin:16px 0}
.card{background:#161b22;border:1px solid #21262d;border-radius:8px;padding:18px}
.card-t{color:#8b949e;font-size:11px;text-transform:uppercase;letter-spacing:1px;margin-bottom:6px}
.card-v{font-size:28px;font-weight:700}
.card-s{color:#8b949e;font-size:12px;margin-top:4px}
.g{color:#3fb950}.r{color:#f85149}.y{color:#d29922}.b{color:#58a6ff}.gr{color:#8b949e}
table{width:100%;border-collapse:collapse;margin:12px 0}
th{background:#161b22;color:#8b949e;text-align:left;padding:10px 14px;font-size:11px;text-transform:uppercase;letter-spacing:.5px;border-bottom:2px solid #21262d}
td{padding:10px 14px;border-bottom:1px solid #21262d;font-size:14px}
.issue{background:#161b22;border:1px solid #21262d;border-radius:6px;padding:14px 18px;margin:8px 0}
.issue b{font-weight:600}.issue .m{color:#8b949e;font-size:13px;margin-top:4px}
.risk{border-left:3px solid #f85149}.warn{border-left:3px solid #d29922}.ok{border-left:3px solid #3fb950}
code{background:#21262d;padding:1px 6px;border-radius:4px;font-size:13px}
a{color:inherit;text-decoration:none;border-bottom:1px dotted #58a6ff}
a:hover{color:#58a6ff}
details{margin:24px 0;color:#8b949e;font-size:13px}
details summary{cursor:pointer;color:#58a6ff}
details p{margin-top:8px;max-width:900px}
footer{margin-top:48px;padding-top:16px;border-top:1px solid #21262d;color:#484f58;font-size:12px}
"#;

/// Expands a collapsed <details> when an in-page anchor targets something inside it.
const AUTO_OPEN_SCRIPT: &str = "<script>\nfunction openTarget(){var el=document.getElementById(location.hash.slice(1));while(el){if(el.tagName==='DETAILS'){el.open=true;break}el=el.parentElement}}\nwindow.addEventListener('hashchange',openTarget);openTarget();\n</script>\n";

/// Render the audit as a clickable dark-theme HTML report (matches the AI-adoption
/// report house style: summary cards → anchors, GitLab file links, Export PDF).
fn render_html(o: &AuditOutcome) -> String {
    use crate::tools::reports::{htmlescape as esc, EXPORT_BUTTON, PRINT_CSS};
    let date_str = chrono::Utc::now().format("%A, %d %B %Y").to_string();
    let version = env!("CARGO_PKG_VERSION");
    let pid = esc(&o.project_id);

    let blob = |file: &str, line: u64| -> String {
        if o.web_url.is_empty() {
            format!("<code>{}:{}</code>", esc(file), line)
        } else {
            format!(
                "<a href=\"{}/-/blob/{}/{}#L{}\"><code>{}:{}</code></a>",
                esc(&o.web_url),
                esc(&o.search_ref),
                esc(file),
                line,
                esc(file),
                line
            )
        }
    };

    let count = |v: Verdict| o.audits.iter().filter(|a| a.verdict == v).count();
    let cleanup = count(Verdict::CleanupDebt);
    let drift = count(Verdict::Drift);
    let stale = count(Verdict::StaleDoc);
    let in_sync = count(Verdict::InSync);
    let review = count(Verdict::NeedsReview);
    let hardcoded = o.secrets.iter().filter(|s| !s.hardcoded_in.is_empty()).count();
    let by = |v: Verdict| -> Vec<&RouteAudit> { o.audits.iter().filter(|a| a.verdict == v).collect() };

    let (vclass, vword) = match o.version_verdict {
        VersionVerdict::InSync => ("g", "in sync"),
        VersionVerdict::DocBehind => ("r", "STALE"),
        VersionVerdict::DocAhead => ("y", "ahead"),
        VersionVerdict::Unknown => ("gr", "unknown"),
    };

    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
    h.push_str(&format!("<title>Spec-drift audit — {pid} — {date_str}</title>\n"));
    h.push_str("<style>\n");
    h.push_str(SPEC_STYLE);
    h.push_str(PRINT_CSS);
    h.push_str("\n@media print{a{border-bottom:none !important;color:inherit !important}}\n</style>\n</head>\n<body>\n");
    h.push_str(EXPORT_BUTTON);
    h.push_str(AUTO_OPEN_SCRIPT);

    h.push_str(&format!("<h1>Spec-drift audit — {pid}</h1>\n"));
    h.push_str(&format!(
        "<div class=\"sub\">Ref <code>{}</code> &middot; {} routes parsed from spec &middot; {date_str}</div>\n",
        esc(&o.search_ref),
        o.audits.len()
    ));

    // Summary cards.
    h.push_str("<div class=\"grid\">\n");
    h.push_str(&format!(
        "<div class=\"card\"><div class=\"card-t\">Version</div><div class=\"card-v {vclass}\"><a href=\"#version\">{vword}</a></div><div class=\"card-s\">spec vs latest tag</div></div>\n"
    ));
    let card = |title: &str, n: usize, cls: &str, href: &str, sub: &str| -> String {
        format!("<div class=\"card\"><div class=\"card-t\">{title}</div><div class=\"card-v {cls}\"><a href=\"{href}\">{n}</a></div><div class=\"card-s\">{sub}</div></div>\n")
    };
    h.push_str(&card("Cleanup debt", cleanup, if cleanup > 0 { "y" } else { "g" }, "#cleanup", "flagged, still in code"));
    h.push_str(&card("Drift", drift, if drift > 0 { "y" } else { "g" }, "#drift", "active, missing"));
    h.push_str(&card("Stale doc", stale, "gr", "#stale", "flagged & gone"));
    h.push_str(&card("Undocumented", o.undocumented.len(), if o.undocumented.is_empty() { "g" } else { "y" }, "#undocumented", "in code, not in spec"));
    h.push_str(&card("Secrets", o.secrets.len(), if hardcoded > 0 { "r" } else if o.secrets.is_empty() { "g" } else { "y" }, "#security", &format!("{hardcoded} hardcoded")));
    h.push_str("</div>\n");

    // Version.
    h.push_str("<h2 id=\"version\">Version</h2>\n");
    let vcls = if o.version_verdict == VersionVerdict::DocBehind { "risk" } else if o.version_verdict == VersionVerdict::InSync { "ok" } else { "warn" };
    h.push_str(&format!(
        "<div class=\"issue {vcls}\"><b>spec <code>{}</code> &middot; latest tag <code>{}</code></b><div class=\"m\">{}</div></div>\n",
        esc(o.doc_version.as_deref().unwrap_or("?")),
        esc(o.latest_tag.as_deref().unwrap_or("none")),
        match o.version_verdict {
            VersionVerdict::InSync => "In sync.",
            VersionVerdict::DocBehind => "Spec is stale — refresh it to the shipped version.",
            VersionVerdict::DocAhead => "Spec is ahead of the latest tag — unreleased, or tags lag.",
            VersionVerdict::Unknown => "Could not compare.",
        }
    ));

    // Changes since last audit.
    if let Some((since, lines)) = &o.changes {
        h.push_str(&format!("<h2 id=\"changes\">Changes since last audit ({})</h2>\n", esc(since)));
        if lines.is_empty() {
            h.push_str("<div class=\"issue ok\"><div class=\"m\">No changes.</div></div>\n");
        } else {
            for l in lines {
                h.push_str(&format!("<div class=\"issue\"><div class=\"m\">{}</div></div>\n", esc(l)));
            }
        }
    }

    // Route-drift sections.
    let route_section = |h: &mut String, id: &str, title: &str, blurb: &str, cls: &str, rows: &[&RouteAudit], show_hits: bool| {
        if rows.is_empty() {
            return;
        }
        h.push_str(&format!("<h2 id=\"{id}\">{title} ({})</h2>\n", rows.len()));
        h.push_str(&format!("<p class=\"sub\">{blurb}</p>\n"));
        for a in rows {
            let label = if !a.route.label.is_empty() && a.route.label != a.route.path {
                format!(" <span class=\"gr\">({})</span>", esc(&a.route.label))
            } else {
                String::new()
            };
            let hits = if show_hits && !a.hits.is_empty() {
                let links: Vec<String> = a.hits.iter().map(|(f, l)| blob(f, *l)).collect();
                format!("<div class=\"m\">{}</div>", links.join(", "))
            } else {
                String::new()
            };
            h.push_str(&format!(
                "<div class=\"issue {cls}\"><b><code>{}</code></b>{label}{hits}</div>\n",
                esc(&a.route.path)
            ));
        }
    };
    route_section(&mut h, "cleanup", "Cleanup debt", "Spec flags these for removal, but they're still wired in code.", "warn", &by(Verdict::CleanupDebt), true);
    route_section(&mut h, "drift", "Drift", "Spec lists these as active, but they're missing from code.", "warn", &by(Verdict::Drift), false);
    route_section(&mut h, "stale", "Stale doc rows", "Flagged for removal and already gone — safe to delete from the spec.", "ok", &by(Verdict::StaleDoc), false);

    // Reverse drift.
    if !o.undocumented.is_empty() {
        h.push_str(&format!("<h2 id=\"undocumented\">Undocumented endpoints ({})</h2>\n", o.undocumented.len()));
        let blurb = if o.harvest_mode == "search" {
            "In code, not in the spec. Harvested by search within documented namespaces — pass a routes file for full coverage."
        } else {
            "In code, not in the spec — shadow surface that escaped the doc."
        };
        h.push_str(&format!("<p class=\"sub\">{blurb}</p>\n"));
        h.push_str("<table>\n<tr><th>Endpoint</th><th>Location</th></tr>\n");
        for (path, line, file) in &o.undocumented {
            h.push_str(&format!("<tr><td><code>{}</code></td><td>{}</td></tr>\n", esc(path), blob(file, *line)));
        }
        h.push_str("</table>\n");
    }

    // Security.
    if !o.secrets.is_empty() {
        h.push_str(&format!("<h2 id=\"security\">Security ({})</h2>\n", o.secrets.len()));
        h.push_str("<p class=\"sub\">Secret material in an org-readable doc. Rotate and restrict access; values are masked.</p>\n");
        let mut ordered: Vec<&SecretAudit> = o.secrets.iter().collect();
        ordered.sort_by_key(|s| s.hardcoded_in.is_empty());
        for s in ordered {
            let kind = s.finding.kind.label();
            if s.hardcoded_in.is_empty() {
                h.push_str(&format!(
                    "<div class=\"issue warn\"><b><code>{}</code> [{kind}]</b><div class=\"m\">Doc-only leak — rotate the secret and restrict the doc.</div></div>\n",
                    esc(&s.finding.masked)
                ));
            } else {
                let loc = s.hardcoded_in.iter().map(|(f, l)| blob(f, *l)).collect::<Vec<_>>().join(", ");
                h.push_str(&format!(
                    "<div class=\"issue risk\"><b><code>{}</code> [{kind}]</b><div class=\"m\">Also hardcoded in code at {loc} — rotate AND remove from code.</div></div>\n",
                    esc(&s.finding.masked)
                ));
            }
        }
    }

    // Needs review.
    let review_rows = by(Verdict::NeedsReview);
    if !review_rows.is_empty() {
        h.push_str(&format!("<h2 id=\"review\">Needs review ({review})</h2>\n"));
        h.push_str("<p class=\"sub\">Path too generic to match reliably — check by hand.</p>\n");
        for a in &review_rows {
            h.push_str(&format!("<div class=\"issue\"><b><code>{}</code></b> <span class=\"gr\">({})</span></div>\n", esc(&a.route.path), esc(&a.route.label)));
        }
    }

    // In sync (collapsible).
    let synced = by(Verdict::InSync);
    h.push_str(&format!("<details id=\"insync\"><summary>In sync ({in_sync})</summary>\n"));
    for a in &synced {
        h.push_str(&format!("<div><code>{}</code></div>\n", esc(&a.route.path)));
    }
    h.push_str("</details>\n");

    h.push_str(&format!("\n<footer>gl-mcp v{version} &middot; {date_str}</footer>\n</body>\n</html>"));
    h
}

/// HTML head (dark theme + print CSS + export button + auto-open script),
/// shared by the spec-audit HTML reports. `title` is caller-escaped.
fn html_head(title: &str) -> String {
    use crate::tools::reports::{EXPORT_BUTTON, PRINT_CSS};
    let mut h = String::new();
    h.push_str("<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"UTF-8\">\n<meta name=\"viewport\" content=\"width=device-width, initial-scale=1.0\">\n");
    h.push_str(&format!("<title>{title}</title>\n<style>\n"));
    h.push_str(SPEC_STYLE);
    h.push_str(PRINT_CSS);
    h.push_str("\n@media print{a{border-bottom:none !important;color:inherit !important}}\n</style>\n</head>\n<body>\n");
    h.push_str(EXPORT_BUTTON);
    h.push_str(AUTO_OPEN_SCRIPT);
    h
}

/// Cross-team HTML rollup: summary cards, a clickable team table, needs-attention,
/// and a collapsible per-team detail block (version, drift, stale, undocumented
/// with GitLab links, secrets). `None` outcome = that team failed to audit.
fn render_sweep_html(teams: &[(String, Option<AuditOutcome>)]) -> String {
    use crate::tools::reports::htmlescape as esc;
    let date_str = chrono::Utc::now().format("%A, %d %B %Y").to_string();
    let version = env!("CARGO_PKG_VERSION");
    let count = |o: &AuditOutcome, v: Verdict| o.audits.iter().filter(|a| a.verdict == v).count();
    let hc = |o: &AuditOutcome| o.secrets.iter().filter(|s| !s.hardcoded_in.is_empty()).count();
    let anchor_of = |label: &str| -> String {
        label.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' }).collect()
    };
    let vshort = |v: VersionVerdict| match v {
        VersionVerdict::DocBehind => "STALE",
        VersionVerdict::InSync => "in-sync",
        VersionVerdict::DocAhead => "ahead",
        VersionVerdict::Unknown => "unknown",
    };
    let blob = |web_url: &str, search_ref: &str, file: &str, line: u64| -> String {
        if web_url.is_empty() {
            format!("<code>{}:{}</code>", esc(file), line)
        } else {
            format!("<a href=\"{}/-/blob/{}/{}#L{}\"><code>{}:{}</code></a>", esc(web_url), esc(search_ref), esc(file), line, esc(file), line)
        }
    };

    let oks: Vec<(&String, &AuditOutcome)> = teams.iter().filter_map(|(l, o)| o.as_ref().map(|x| (l, x))).collect();
    let tot_drift: usize = oks.iter().map(|(_, o)| count(o, Verdict::Drift)).sum();
    let tot_stale: usize = oks.iter().map(|(_, o)| count(o, Verdict::StaleDoc)).sum();
    let tot_undoc: usize = oks.iter().map(|(_, o)| o.undocumented.len()).sum();
    let tot_secrets: usize = oks.iter().map(|(_, o)| o.secrets.len()).sum();
    let tot_hc: usize = oks.iter().map(|(_, o)| hc(o)).sum();
    let stale_ver = oks.iter().filter(|(_, o)| o.version_verdict == VersionVerdict::DocBehind).count();

    let mut h = html_head(&format!("Cross-team spec-drift report — {date_str}"));
    h.push_str(&format!(
        "<h1>Cross-team spec-drift report</h1>\n<div class=\"sub\">{} teams &middot; {date_str}</div>\n",
        teams.len()
    ));

    // Summary cards.
    let card = |t: &str, v: String, cls: &str, sub: &str| {
        format!("<div class=\"card\"><div class=\"card-t\">{t}</div><div class=\"card-v {cls}\">{v}</div><div class=\"card-s\">{sub}</div></div>\n")
    };
    h.push_str("<div class=\"grid\">\n");
    h.push_str(&card("Teams", teams.len().to_string(), "b", "audited"));
    h.push_str(&card("Stale versions", stale_ver.to_string(), if stale_ver > 0 { "r" } else { "g" }, "spec behind tag"));
    h.push_str(&card("Drift", tot_drift.to_string(), if tot_drift > 0 { "y" } else { "g" }, "active, missing"));
    h.push_str(&card("Stale doc", tot_stale.to_string(), "gr", "flagged & gone"));
    h.push_str(&card("Undocumented", tot_undoc.to_string(), if tot_undoc > 0 { "y" } else { "g" }, "in code, not in spec"));
    h.push_str(&card("Secrets", tot_secrets.to_string(), if tot_hc > 0 { "r" } else if tot_secrets > 0 { "y" } else { "g" }, &format!("{tot_hc} hardcoded")));
    h.push_str("</div>\n");

    // Cross-team table.
    h.push_str("<h2>By team</h2>\n<table>\n<tr><th>Team</th><th>Version</th><th>Cleanup</th><th>Drift</th><th>Stale</th><th>Undoc</th><th>Secrets</th><th>In-sync</th></tr>\n");
    let mut approx_seen = false;
    for (label, o) in teams {
        match o {
            None => h.push_str(&format!("<tr><td><b>{}</b></td><td class=\"r\">failed to audit</td><td></td><td></td><td></td><td></td><td></td><td></td></tr>\n", esc(label))),
            Some(o) => {
                if o.harvest_mode == "search" { approx_seen = true; }
                let undoc = if o.harvest_mode == "search" { format!("{}~", o.undocumented.len()) } else { o.undocumented.len().to_string() };
                let vcls = match o.version_verdict {
                    VersionVerdict::DocBehind => "r",
                    VersionVerdict::InSync => "g",
                    VersionVerdict::DocAhead => "y",
                    VersionVerdict::Unknown => "gr",
                };
                h.push_str(&format!(
                    "<tr><td><b><a href=\"#{}\">{}</a></b></td><td class=\"{vcls}\">{}</td><td>{}</td><td>{}</td><td>{}</td><td>{undoc}</td><td>{} ({} hc)</td><td>{}</td></tr>\n",
                    anchor_of(label), esc(label), vshort(o.version_verdict),
                    count(o, Verdict::CleanupDebt), count(o, Verdict::Drift), count(o, Verdict::StaleDoc),
                    o.secrets.len(), hc(o), count(o, Verdict::InSync)
                ));
            }
        }
    }
    h.push_str("</table>\n");
    if approx_seen {
        h.push_str("<p class=\"sub\"><code>~</code> = reverse-drift search-harvested (namespace-gated lower bound); pass a routes file per team for a precise count.</p>\n");
    }

    // Needs attention.
    let flagged: Vec<(&String, &AuditOutcome)> = oks.iter().copied()
        .filter(|(_, o)| count(o, Verdict::CleanupDebt) > 0 || count(o, Verdict::Drift) > 0 || o.version_verdict == VersionVerdict::DocBehind || hc(o) > 0)
        .collect();
    if !flagged.is_empty() {
        h.push_str("<h2>Needs attention</h2>\n");
        for (label, o) in flagged {
            let mut notes: Vec<String> = Vec::new();
            if o.version_verdict == VersionVerdict::DocBehind { notes.push("version stale".to_string()); }
            let d = count(o, Verdict::Drift); if d > 0 { notes.push(format!("{d} drift")); }
            let c = count(o, Verdict::CleanupDebt); if c > 0 { notes.push(format!("{c} cleanup-debt")); }
            if hc(o) > 0 { notes.push(format!("{} hardcoded secret(s)", hc(o))); }
            h.push_str(&format!("<div class=\"issue warn\"><b><a href=\"#{}\">{}</a></b><div class=\"m\">{}</div></div>\n", anchor_of(label), esc(label), notes.join(", ")));
        }
    }

    // Per-team detail.
    for (label, o) in teams {
        let Some(o) = o else { continue };
        h.push_str(&format!("<details id=\"{}\"><summary>{} — {} routes, {} undocumented</summary>\n", anchor_of(label), esc(label), o.audits.len(), o.undocumented.len()));
        h.push_str(&format!(
            "<div class=\"m\">Version: spec <code>{}</code> vs tag <code>{}</code> ({})</div>\n",
            esc(o.doc_version.as_deref().unwrap_or("?")), esc(o.latest_tag.as_deref().unwrap_or("none")), vshort(o.version_verdict)
        ));
        let routes_of = |v: Verdict| -> String {
            o.audits.iter().filter(|x| x.verdict == v).map(|x| format!("<code>{}</code>", esc(&x.route.path))).collect::<Vec<_>>().join(", ")
        };
        let drift = routes_of(Verdict::Drift);
        if !drift.is_empty() { h.push_str(&format!("<div class=\"m\"><b>Drift (active, missing):</b> {drift}</div>\n")); }
        let stale = routes_of(Verdict::StaleDoc);
        if !stale.is_empty() { h.push_str(&format!("<div class=\"m\"><b>Stale doc rows:</b> {stale}</div>\n")); }
        if !o.undocumented.is_empty() {
            h.push_str(&format!("<div class=\"m\"><b>Undocumented endpoints ({}):</b></div>\n", o.undocumented.len()));
            for (p, line, file) in o.undocumented.iter().take(15) {
                h.push_str(&format!("<div class=\"m\">&middot; <code>{}</code> &rarr; {}</div>\n", esc(p), blob(&o.web_url, &o.search_ref, file, *line)));
            }
            if o.undocumented.len() > 15 { h.push_str(&format!("<div class=\"m\">&hellip; and {} more</div>\n", o.undocumented.len() - 15)); }
        }
        if !o.secrets.is_empty() {
            let secs = o.secrets.iter().map(|s| format!("<code>{}</code>{}", esc(&s.finding.masked), if s.hardcoded_in.is_empty() { "" } else { " (hardcoded)" })).collect::<Vec<_>>().join(", ");
            h.push_str(&format!("<div class=\"m\"><b>Secrets ({}):</b> {secs}</div>\n", o.secrets.len()));
        }
        h.push_str("</details>\n");
    }

    h.push_str(&format!("\n<footer>gl-mcp v{version} &middot; {date_str}</footer>\n</body>\n</html>"));
    h
}

/// Audit several teams' specs against their repos concurrently and render one
/// clickable cross-team HTML report (summary cards, team table, per-team detail).
pub async fn generate_sweep_report(client: &GitLabClient, targets: &[SweepTarget]) -> Result<String> {
    let mut teams: Vec<(String, Option<AuditOutcome>)> = Vec::with_capacity(targets.len());
    for chunk in targets.chunks(SWEEP_CONCURRENCY) {
        let futs = chunk.iter().map(|t| {
            let client = client.clone();
            async move {
                let r = compute_audit(&client, &t.project_id, &t.spec, &t.ref_name, &t.routes_file, &t.label).await.ok();
                (t.label.clone(), r)
            }
        });
        teams.extend(join_all(futs).await);
    }
    Ok(render_sweep_html(&teams))
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
    fn test_extract_secrets_ignores_route_paths() {
        // A long slash-delimited route path must NOT be mistaken for a base64
        // secret (regression: the base64 class includes '/'). Synthetic paths.
        let spec = "| Nodes | /api/v2/resource/group/segment/item |\n| Cfg | /aBcD1234eFgH5678iJkL/mNoP9012qRsT |";
        assert!(extract_secrets(spec).is_empty());
        // but a real key with padding is still caught
        let key = "key: AAAABBBBCCCCDDDDEEEEFFFFGGGGHHHH0000=";
        assert_eq!(extract_secrets(key).len(), 1);
        // and one containing a '+' (base64-distinctive) is caught
        let key2 = "tok: AAAABBBBCCCC+DDDDEEEEFFFFGGGGHHHHIIII";
        assert_eq!(extract_secrets(key2).len(), 1);
    }

    #[test]
    fn test_map_path_discriminator() {
        // Same project+ref, different map_key → different snapshot files (so
        // shared-repo sweep targets don't clobber each other). Empty key keeps
        // the legacy unsuffixed name.
        let win = map_path("org/desktop", "main", "Windows");
        let mac = map_path("org/desktop", "main", "macOS");
        let bare = map_path("org/desktop", "main", "");
        assert_ne!(win, mac);
        assert!(win.to_string_lossy().ends_with("org_desktop__main__Windows.json"));
        assert!(mac.to_string_lossy().ends_with("org_desktop__main__macOS.json"));
        assert!(bare.to_string_lossy().ends_with("org_desktop__main.json"));
    }

    #[test]
    fn test_render_sweep_html_smoke() {
        let mk = |path: &str, v: Verdict| RouteAudit {
            route: DocRoute { label: String::new(), path: path.to_string(), status: DocStatus::Active, query: None },
            hits: Vec::new(),
            verdict: v,
        };
        let ios = AuditOutcome {
            project_id: "org/ios".into(), search_ref: "main".into(), web_url: "https://ex.com/org/ios".into(),
            doc_version: Some("4.9.5".into()), latest_tag: Some("release-4.9.10".into()),
            version_verdict: VersionVerdict::DocBehind,
            audits: vec![mk("/v3/feedbacks", Verdict::Drift), mk("/v3/user", Verdict::InSync)],
            secrets: Vec::new(),
            undocumented: vec![("/v4/devices".into(), 91, "Net.swift".into())],
            harvest_mode: "file".into(), changes: None,
        };
        let teams = vec![("iOS".to_string(), Some(ios)), ("Windows".to_string(), None)];
        let html = render_sweep_html(&teams);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("Cross-team spec-drift report"));
        // team table links to per-team anchor; failed team marked
        assert!(html.contains("href=\"#ios\""));
        assert!(html.contains("id=\"ios\""));
        assert!(html.contains("failed to audit"));
        // per-team detail has the undocumented endpoint with a blob link
        assert!(html.contains("/-/blob/main/Net.swift#L91"));
        assert!(html.contains("STALE"));
        assert!(html.ends_with("</html>"));
    }

    #[test]
    fn test_render_sweep() {
        let rows = vec![
            SweepRow {
                label: "iOS".into(),
                error: None,
                version: "STALE (4.9.5<4.9.10)".into(),
                cleanup: 0,
                drift: 1,
                stale: 2,
                undoc: 19,
                approx: false, // precise (routes_file)
                secrets: 1,
                hardcoded: 0,
                in_sync: 6,
            },
            SweepRow {
                label: "Android".into(),
                error: None,
                version: "in-sync".into(),
                cleanup: 0,
                drift: 0,
                stale: 0,
                undoc: 5,
                approx: true, // search-harvested
                secrets: 0,
                hardcoded: 0,
                in_sync: 8,
            },
            SweepRow {
                label: "Windows".into(),
                error: Some("404 project not found".into()),
                version: String::new(),
                cleanup: 0,
                drift: 0,
                stale: 0,
                undoc: 0,
                approx: false,
                secrets: 0,
                hardcoded: 0,
                in_sync: 0,
            },
        ];
        let out = render_sweep(&rows, false);
        // table has all three platforms
        assert!(out.contains("| iOS | STALE (4.9.5<4.9.10) |"));
        assert!(out.contains("| Android | in-sync |"));
        assert!(out.contains("| Windows | failed: 404 project not found |"));
        // approximate reverse-drift marked with ~ and a legend; precise is bare
        assert!(out.contains("| 5~ |"));
        assert!(out.contains("| 19 |"));
        assert!(out.contains("`~` = reverse-drift search-harvested"));
        // iOS is flagged (stale + drift); Android (clean) is not
        assert!(out.contains("## Needs attention"));
        assert!(out.contains("**iOS**"));
        assert!(!out.contains("**Android**"));
        // totals exclude the failed platform (2 ok) and sum drift across them
        assert!(out.contains("Across 2 platforms"));
        assert!(out.contains("1 drift"));
        assert!(out.contains("1 platform(s) failed"));
        // summary_only drops the detail sections
        let summary = render_sweep(&rows, true);
        assert!(summary.contains("| iOS |"));
        assert!(!summary.contains("## Needs attention"));
        assert!(!summary.contains("## Totals"));
    }

    #[test]
    fn test_render_html_smoke() {
        let route = |path: &str, label: &str, verdict: Verdict| RouteAudit {
            route: DocRoute {
                label: label.to_string(),
                path: path.to_string(),
                status: DocStatus::Active,
                query: None,
            },
            hits: Vec::new(),
            verdict,
        };
        let outcome = AuditOutcome {
            project_id: "my-org/app".to_string(),
            search_ref: "main".to_string(),
            web_url: "https://example.com/my-org/app".to_string(),
            doc_version: Some("4.9.5".to_string()),
            latest_tag: Some("release-4.9.10".to_string()),
            version_verdict: VersionVerdict::DocBehind,
            audits: vec![route("/v3/feedbacks", "Feedback", Verdict::Drift)],
            secrets: vec![SecretAudit {
                finding: SecretFinding {
                    kind: SecretKind::Base64Secret,
                    value: "AAAA0000".to_string(),
                    masked: "AAAA…0000 (8 chars)".to_string(),
                },
                hardcoded_in: Vec::new(),
            }],
            undocumented: vec![("/v4/feedbacks".to_string(), 146, "Net.swift".to_string())],
            harvest_mode: "file".to_string(),
            changes: Some(("2026-06-15 00:00 UTC".to_string(), vec!["+ route `/x`".to_string()])),
        };
        let html = render_html(&outcome);
        assert!(html.starts_with("<!DOCTYPE html>"));
        assert!(html.contains("Spec-drift audit — my-org/app"));
        assert!(html.contains("id=\"undocumented\""));
        assert!(html.contains("id=\"security\""));
        // masked secret present, raw value never rendered
        assert!(html.contains("AAAA…0000 (8 chars)"));
        assert!(!html.contains(">AAAA0000<"));
        // file link points at the blob with line anchor
        assert!(html.contains("/-/blob/main/Net.swift#L146"));
        assert!(html.ends_with("</html>"));
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
    fn test_is_code_file() {
        assert!(is_code_file("routes/api/withoutPrefix.php"));
        assert!(is_code_file("network/ApiConst.kt"));
        assert!(is_code_file("resources/url_info/urls.json"));
        assert!(!is_code_file("assets/logo.png"));
        assert!(!is_code_file("Gemfile.lock"));
        assert!(!is_code_file("app/release.apk"));
    }

    #[test]
    fn test_harvest_multi_dedup_and_attribution() {
        let files = vec![
            ("routes/api.php".to_string(), "Route::get('/v3/user', f);\nRoute::post('/orders', f);".to_string()),
            ("routes/crm.php".to_string(), "Route::get('/v3/user', f);\nRoute::get('/crm/stats', f);".to_string()),
        ];
        let eps = harvest_multi(&files);
        let paths: Vec<&str> = eps.iter().map(|(p, _, _)| p.as_str()).collect();
        // union across files, /v3/user deduped (first file wins its attribution)
        assert!(paths.contains(&"/v3/user"));
        assert!(paths.contains(&"/orders"));
        assert!(paths.contains(&"/crm/stats"));
        assert_eq!(paths.iter().filter(|p| **p == "/v3/user").count(), 1);
        let user = eps.iter().find(|(p, _, _)| p == "/v3/user").unwrap();
        assert_eq!(user.2, "routes/api.php"); // first file wins
        let crm = eps.iter().find(|(p, _, _)| p == "/crm/stats").unwrap();
        assert_eq!(crm.2, "routes/crm.php"); // tagged with its own file
    }

    #[test]
    fn test_harvest_single_quoted_routes() {
        // PHP/Laravel and Ruby style — single-quoted leading-slash literals.
        let code = "Route::get('/user/settings', [C::class, 'm']);\n  get '/v1/votes'\n";
        let paths: Vec<String> =
            harvest_path_literals(code).into_iter().map(|(p, _)| p).collect();
        assert!(paths.contains(&"/user/settings".to_string()));
        assert!(paths.contains(&"/v1/votes".to_string()));
    }

    #[test]
    fn test_harvest_seeds_cover_both_quote_styles() {
        // Regression: search-harvest must seed single-quote forms too, or it
        // silently misses PHP/Ruby/Python repos (caught on a Laravel backend).
        assert!(HARVEST_SEEDS.iter().any(|s| s.contains('\'')));
        assert!(HARVEST_SEEDS.iter().any(|s| s.contains('"')));
    }

    #[test]
    fn test_harvest_fragment_assembled_routes() {
        // The blind spot: paths split across concatenated literals, or with an
        // interpolated middle segment.
        let code = r#"
        let a = "/v3" + "/user"
        let b = base + "/orders"
        let c = "/users/\(id)/posts"
        let ct = "application/json"
        let bearer = "Bearer " + token
        "#;
        let paths: Vec<String> =
            harvest_path_literals(code).into_iter().map(|(p, _)| p).collect();
        // "/v3" + "/user" stitched into one endpoint, not split into /v3 and /user
        assert!(paths.contains(&"/v3/user".to_string()));
        assert!(!paths.contains(&"/v3".to_string()));
        // base + "/orders": base is a variable, the literal tail is still caught
        assert!(paths.contains(&"/orders".to_string()));
        // interpolated middle segment removed, double slash collapsed
        assert!(paths.contains(&"/users/posts".to_string()));
        // header values are not endpoints
        assert!(!paths.iter().any(|p| p.contains("application/json")));
        assert!(!paths.iter().any(|p| p.contains("Bearer")));
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

    #[test]
    fn snapshot_path_is_traversal_proof() {
        use std::path::PathBuf;
        let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
        let root = PathBuf::from(&home).join(".gl-mcp").join("spec_maps");

        // Hostile inputs across all three caller-controlled tokens must still
        // resolve to a flat file directly inside spec_maps/.
        let cases = [
            ("../../../etc/passwd", "..\\..\\windows", ".."),
            ("/etc/shadow", "~/.ssh/id_rsa", "a/b/c"),
            ("..", ".", ""),
            ("normal-project", "main", "ios"),
        ];
        for (proj, refn, key) in cases {
            let p = map_path(proj, refn, key);
            assert_eq!(p.parent(), Some(root.as_path()), "escaped spec_maps for {proj:?}");
            let fname = p.file_name().unwrap().to_str().unwrap();
            assert!(!fname.contains('/') && !fname.contains('\\'), "separator in {fname:?}");
            assert!(!fname.starts_with('.'), "dotfile/leading-dot in {fname:?}");
            assert!(fname.ends_with(".json"));
        }
    }

    #[test]
    fn safe_component_allowlist() {
        assert_eq!(safe_component("../../etc"), "_.._etc");
        assert_eq!(safe_component(".."), "_");
        assert_eq!(safe_component("main"), "main");
        assert_eq!(safe_component("feature/x y"), "feature_x_y");
        assert_eq!(safe_component(""), "_");
    }
}
