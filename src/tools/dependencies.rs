//! Dependency vulnerability audit.
//!
//! Lockfiles are read over the GitLab API (no clone) and every pinned public
//! dependency is matched against OSV.dev — the open vulnerability database that
//! aggregates GitHub, RustSec, Go, PyPA and other advisory feeds, with exact
//! affected-version ranges and fixed versions per ecosystem.
//!
//! Two properties are deliberate:
//! - Only packages from public registries are sent to OSV. Anything resolved from
//!   the GitLab host itself, a git URL, or a local path is an internal package; its
//!   name never leaves the network.
//! - An audit that could not look is an error, not a clean result: an unreachable
//!   OSV fails the tool, and repositories with nothing auditable are listed as such.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use futures::future::join_all;
use serde_json::{json, Value};

use crate::client::GitLabClient;
use crate::error::{Error, FanOut, Result};

// ─── Ecosystems and lockfiles ───

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum Ecosystem {
    CratesIo,
    Npm,
    Packagist,
    Go,
    PyPI,
    RubyGems,
    Maven,
}

impl Ecosystem {
    /// The ecosystem name OSV expects.
    pub(crate) fn osv_name(self) -> &'static str {
        match self {
            Self::CratesIo => "crates.io",
            Self::Npm => "npm",
            Self::Packagist => "Packagist",
            Self::Go => "Go",
            Self::PyPI => "PyPI",
            Self::RubyGems => "RubyGems",
            Self::Maven => "Maven",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum LockKind {
    CargoLock,
    PackageLock,
    YarnLock,
    PnpmLock,
    ComposerLock,
    GoSum,
    PoetryLock,
    PipfileLock,
    Requirements,
    GemfileLock,
    GradleLockfile,
    /// Recognised but not auditable against OSV (no CocoaPods feed).
    PodfileLock,
}

/// Which lockfile a path is, by file name. Vendored trees are ignored: their
/// lockfiles describe someone else's project.
pub(crate) fn lockfile_kind(path: &str) -> Option<LockKind> {
    if path.split('/').any(|seg| matches!(seg, "node_modules" | "vendor" | ".git" | "third_party")) {
        return None;
    }
    let name = path.rsplit('/').next().unwrap_or(path);
    Some(match name {
        "Cargo.lock" => LockKind::CargoLock,
        "package-lock.json" | "npm-shrinkwrap.json" => LockKind::PackageLock,
        "yarn.lock" => LockKind::YarnLock,
        "pnpm-lock.yaml" => LockKind::PnpmLock,
        "composer.lock" => LockKind::ComposerLock,
        "go.sum" => LockKind::GoSum,
        "poetry.lock" => LockKind::PoetryLock,
        "Pipfile.lock" => LockKind::PipfileLock,
        "Gemfile.lock" => LockKind::GemfileLock,
        "gradle.lockfile" => LockKind::GradleLockfile,
        "Podfile.lock" => LockKind::PodfileLock,
        n if n.starts_with("requirements") && n.ends_with(".txt") => LockKind::Requirements,
        _ => return None,
    })
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) struct Dep {
    pub ecosystem: Ecosystem,
    pub name: String,
    pub version: String,
}

/// What a lockfile yielded: auditable public dependencies, and how many entries were
/// kept out of the audit because they resolve to internal or non-registry sources.
#[derive(Debug, Default)]
pub(crate) struct Parsed {
    pub deps: Vec<Dep>,
    pub internal_skipped: usize,
}

impl Parsed {
    fn push(&mut self, ecosystem: Ecosystem, name: &str, version: &str) {
        let (name, version) = (name.trim(), version.trim());
        if !name.is_empty() && !version.is_empty() {
            self.deps.push(Dep { ecosystem, name: name.to_string(), version: version.to_string() });
        }
    }
}

/// True when a resolved URL or source string points somewhere private: the GitLab
/// host, a git checkout, or a local path.
fn is_internal_source(source: &str, internal_host: &str) -> bool {
    let s = source.trim();
    !s.is_empty()
        && ((!internal_host.is_empty() && s.contains(internal_host))
            || s.starts_with("git+")
            || s.starts_with("git:")
            || s.starts_with("file:")
            || s.starts_with("link:")
            || s.starts_with("path+")
            || s.ends_with(".git"))
}

/// Parse a lockfile into auditable dependencies. Never fails: an unreadable file
/// yields nothing, and the report counts it as a lockfile with no packages.
pub(crate) fn parse_lockfile(kind: LockKind, text: &str, internal_host: &str) -> Parsed {
    let mut p = Parsed::default();
    match kind {
        LockKind::CargoLock | LockKind::PoetryLock => {
            let eco = if kind == LockKind::CargoLock { Ecosystem::CratesIo } else { Ecosystem::PyPI };
            let Ok(doc) = text.parse::<toml::Table>() else { return p };
            for pkg in doc.get("package").and_then(|v| v.as_array()).into_iter().flatten() {
                let get = |k: &str| pkg.get(k).and_then(|v| v.as_str()).unwrap_or("");
                let source = get("source");
                // Cargo: workspace members have no source; only registry crates are public.
                if kind == LockKind::CargoLock && !source.starts_with("registry+") {
                    if !source.is_empty() {
                        p.internal_skipped += 1;
                    }
                    continue;
                }
                if kind == LockKind::PoetryLock {
                    let src_url = pkg.get("source").and_then(|s| s.get("url")).and_then(|u| u.as_str()).unwrap_or("");
                    if is_internal_source(src_url, internal_host) {
                        p.internal_skipped += 1;
                        continue;
                    }
                }
                p.push(eco, get("name"), get("version"));
            }
        }
        LockKind::PackageLock => {
            let Ok(doc) = serde_json::from_str::<Value>(text) else { return p };
            if let Some(packages) = doc["packages"].as_object() {
                // lockfileVersion 2/3: "node_modules/<name>" keys; "" is the root.
                for (path, info) in packages {
                    let Some(idx) = path.rfind("node_modules/") else { continue };
                    if info["link"].as_bool() == Some(true) {
                        continue;
                    }
                    if is_internal_source(info["resolved"].as_str().unwrap_or(""), internal_host) {
                        p.internal_skipped += 1;
                        continue;
                    }
                    let name = info["name"].as_str().unwrap_or(&path[idx + "node_modules/".len()..]);
                    p.push(Ecosystem::Npm, name, info["version"].as_str().unwrap_or(""));
                }
            } else {
                // lockfileVersion 1: nested "dependencies".
                fn walk(deps: &Value, host: &str, p: &mut Parsed) {
                    for (name, info) in deps.as_object().into_iter().flatten() {
                        let version = info["version"].as_str().unwrap_or("");
                        if is_internal_source(info["resolved"].as_str().unwrap_or(version), host) {
                            p.internal_skipped += 1;
                        } else {
                            p.push(Ecosystem::Npm, name, version);
                        }
                        walk(&info["dependencies"], host, p);
                    }
                }
                walk(&doc["dependencies"], internal_host, &mut p);
            }
        }
        LockKind::YarnLock => {
            // v1: `"name@range", name@range:` then `  version "x"` / `  resolved "url"`;
            // berry: `version: x` / `resolution: "…"`. `resolved` follows `version`, so
            // each entry is decided only when its block ends — deciding at `version`
            // would send an internal package's name out before seeing where it lives.
            struct Entry {
                name: String,
                version: Option<String>,
                internal: bool,
            }
            let finish = |e: Option<Entry>, p: &mut Parsed| {
                if let Some(Entry { name, version: Some(v), internal }) = e {
                    if internal {
                        p.internal_skipped += 1;
                    } else {
                        p.push(Ecosystem::Npm, &name, &v);
                    }
                }
            };
            let mut current: Option<Entry> = None;
            for line in text.lines() {
                if !line.starts_with(' ') && line.trim_end().ends_with(':') && !line.starts_with('#') {
                    finish(current.take(), &mut p);
                    let first = line.trim_end_matches(':').split(',').next().unwrap_or("").trim().trim_matches('"');
                    // The name ends at the first '@' after a possible scope '@'.
                    let at = first.get(1..).and_then(|rest| rest.find('@')).map(|i| i + 1);
                    current = at.map(|i| Entry {
                        name: first[..i].to_string(),
                        version: None,
                        internal: first.contains("@workspace:") || first.contains("@link:") || first.contains("@file:"),
                    });
                } else if let Some(e) = current.as_mut() {
                    let t = line.trim();
                    if let Some(rest) = t.strip_prefix("resolved ").or_else(|| t.strip_prefix("resolution: ")) {
                        if is_internal_source(rest.trim_matches('"'), internal_host) {
                            e.internal = true;
                        }
                    } else if let Some(v) = t.strip_prefix("version ").or_else(|| t.strip_prefix("version: ")) {
                        e.version = Some(v.trim_matches('"').to_string());
                    }
                }
            }
            finish(current.take(), &mut p);
        }
        LockKind::PnpmLock => {
            // Package keys under `packages:` — `/name@1.2.3:`, `/name/1.2.3:` (v5) or
            // `name@1.2.3:` (v9); scoped names keep their leading '@'. Peer suffixes
            // in parentheses are dropped.
            let mut in_packages = false;
            for line in text.lines() {
                if line.trim().is_empty() {
                    continue; // blank lines separate entries; they don't end the section
                }
                if !line.starts_with(' ') {
                    in_packages = line.trim_end() == "packages:";
                    continue;
                }
                if !in_packages || line.starts_with("    ") {
                    continue;
                }
                let key = line.trim().trim_end_matches(':').trim_matches('\'').trim_matches('"');
                let key = key.split('(').next().unwrap_or(key).trim_start_matches('/');
                let split = key.get(1..).and_then(|rest| rest.rfind(['@', '/'])).map(|i| i + 1);
                if let Some(i) = split {
                    let (name, version) = (&key[..i], &key[i + 1..]);
                    if version.chars().next().is_some_and(|c| c.is_ascii_digit()) {
                        p.push(Ecosystem::Npm, name, version);
                    }
                }
            }
        }
        LockKind::ComposerLock => {
            let Ok(doc) = serde_json::from_str::<Value>(text) else { return p };
            for section in ["packages", "packages-dev"] {
                for pkg in doc[section].as_array().into_iter().flatten() {
                    let src = pkg["source"]["url"].as_str().or_else(|| pkg["dist"]["url"].as_str()).unwrap_or("");
                    if !internal_host.is_empty() && src.contains(internal_host) {
                        p.internal_skipped += 1;
                        continue;
                    }
                    let version = pkg["version"].as_str().unwrap_or("");
                    if version.starts_with("dev-") {
                        continue; // a branch, not a release: nothing to match
                    }
                    p.push(Ecosystem::Packagist, pkg["name"].as_str().unwrap_or(""), version.trim_start_matches('v'));
                }
            }
        }
        LockKind::GoSum => {
            let mut seen = BTreeSet::new();
            for line in text.lines() {
                let mut parts = line.split_whitespace();
                let (Some(module), Some(version)) = (parts.next(), parts.next()) else { continue };
                let version = version.trim_end_matches("/go.mod").trim_end_matches("+incompatible");
                if !internal_host.is_empty() && module.starts_with(internal_host) {
                    p.internal_skipped += usize::from(seen.insert((module.to_string(), String::new())));
                    continue;
                }
                if seen.insert((module.to_string(), version.to_string())) {
                    p.push(Ecosystem::Go, module, version.trim_start_matches('v'));
                }
            }
        }
        LockKind::PipfileLock => {
            let Ok(doc) = serde_json::from_str::<Value>(text) else { return p };
            for section in ["default", "develop"] {
                for (name, info) in doc[section].as_object().into_iter().flatten() {
                    if let Some(v) = info["version"].as_str().and_then(|v| v.strip_prefix("==")) {
                        p.push(Ecosystem::PyPI, name, v);
                    } else if info.get("git").is_some() || info.get("path").is_some() {
                        p.internal_skipped += 1;
                    }
                }
            }
        }
        LockKind::Requirements => {
            for line in text.lines() {
                let l = line.split('#').next().unwrap_or("").trim();
                if l.is_empty() || l.starts_with('-') {
                    continue;
                }
                if is_internal_source(l, internal_host) || l.contains(" @ ") {
                    p.internal_skipped += 1;
                    continue;
                }
                // Only exact pins can be matched; ranges are not a version.
                if let Some((name, version)) = l.split_once("==") {
                    let name = name.split('[').next().unwrap_or(name);
                    let version = version.split([';', ' ', ',']).next().unwrap_or(version);
                    p.push(Ecosystem::PyPI, name, version);
                }
            }
        }
        LockKind::GemfileLock => {
            // `GEM` → `  specs:` → `    name (1.2.3)` (four spaces; deeper lines are
            // requirements, not resolved gems). GIT/PATH sections are internal.
            let mut section = "";
            let mut in_specs = false;
            for line in text.lines() {
                if !line.starts_with(' ') {
                    section = line.trim();
                    in_specs = false;
                    continue;
                }
                if line.trim() == "specs:" {
                    in_specs = true;
                    continue;
                }
                if !in_specs || !line.starts_with("    ") || line.starts_with("     ") {
                    continue;
                }
                let t = line.trim();
                if let Some((name, rest)) = t.split_once(" (") {
                    let version = rest.trim_end_matches(')').split('-').next().unwrap_or("");
                    if section == "GEM" {
                        p.push(Ecosystem::RubyGems, name, version);
                    } else {
                        p.internal_skipped += 1;
                    }
                }
            }
        }
        LockKind::GradleLockfile => {
            for line in text.lines() {
                let l = line.trim();
                if l.starts_with('#') || l.starts_with("empty=") {
                    continue;
                }
                let coords = l.split('=').next().unwrap_or("");
                let parts: Vec<&str> = coords.split(':').collect();
                if parts.len() == 3 {
                    p.push(Ecosystem::Maven, &format!("{}:{}", parts[0], parts[1]), parts[2]);
                }
            }
        }
        LockKind::PodfileLock => {}
    }
    p
}

// ─── Severity ───

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub(crate) enum VulnSeverity {
    Critical,
    High,
    Medium,
    Low,
    Unrated,
}

impl VulnSeverity {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Critical => "CRITICAL",
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
            Self::Low => "LOW",
            Self::Unrated => "UNRATED",
        }
    }

    fn from_score(score: f64) -> Self {
        match score {
            s if s >= 9.0 => Self::Critical,
            s if s >= 7.0 => Self::High,
            s if s >= 4.0 => Self::Medium,
            s if s > 0.0 => Self::Low,
            _ => Self::Unrated,
        }
    }
}

/// CVSS v3.x base score from a vector string, per the FIRST specification.
/// `None` for anything that is not a complete v3 vector.
pub(crate) fn cvss3_base_score(vector: &str) -> Option<f64> {
    let mut m: BTreeMap<&str, &str> = BTreeMap::new();
    let mut parts = vector.split('/');
    if !parts.next()?.starts_with("CVSS:3") {
        return None;
    }
    for part in parts {
        let (k, v) = part.split_once(':')?;
        m.insert(k, v);
    }
    let scope_changed = *m.get("S")? == "C";
    let av = match *m.get("AV")? { "N" => 0.85, "A" => 0.62, "L" => 0.55, "P" => 0.2, _ => return None };
    let ac = match *m.get("AC")? { "L" => 0.77, "H" => 0.44, _ => return None };
    let pr = match (*m.get("PR")?, scope_changed) {
        ("N", _) => 0.85,
        ("L", false) => 0.62,
        ("L", true) => 0.68,
        ("H", false) => 0.27,
        ("H", true) => 0.5,
        _ => return None,
    };
    let ui = match *m.get("UI")? { "N" => 0.85, "R" => 0.62, _ => return None };
    let cia = |k: &str| match m.get(k).copied() {
        Some("H") => Some(0.56),
        Some("L") => Some(0.22),
        Some("N") => Some(0.0),
        _ => None,
    };
    let (c, i, a) = (cia("C")?, cia("I")?, cia("A")?);
    let iss = 1.0 - (1.0 - c) * (1.0 - i) * (1.0 - a);
    let impact = if scope_changed {
        7.52 * (iss - 0.029) - 3.25 * (iss - 0.02f64).powi(15)
    } else {
        6.42 * iss
    };
    if impact <= 0.0 {
        return Some(0.0);
    }
    let exploitability = 8.22 * av * ac * pr * ui;
    let raw = if scope_changed {
        (1.08 * (impact + exploitability)).min(10.0)
    } else {
        (impact + exploitability).min(10.0)
    };
    // CVSS "round up" to one decimal.
    Some((raw * 10.0).ceil() / 10.0)
}

/// Severity of an OSV record: the advisory's own rating when it has one (GitHub
/// advisories carry LOW/MODERATE/HIGH/CRITICAL), else the CVSS v3 base score.
pub(crate) fn osv_severity(vuln: &Value) -> VulnSeverity {
    let rated = vuln["database_specific"]["severity"]
        .as_str()
        .or_else(|| vuln["affected"][0]["database_specific"]["severity"].as_str());
    match rated.map(str::to_ascii_uppercase).as_deref() {
        Some("CRITICAL") => return VulnSeverity::Critical,
        Some("HIGH") => return VulnSeverity::High,
        Some("MODERATE" | "MEDIUM") => return VulnSeverity::Medium,
        Some("LOW") => return VulnSeverity::Low,
        _ => {}
    }
    vuln["severity"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|s| s["score"].as_str())
        .filter_map(cvss3_base_score)
        .fold(None, |best: Option<f64>, s| Some(best.map_or(s, |b| b.max(s))))
        .map_or(VulnSeverity::Unrated, VulnSeverity::from_score)
}

/// The advisory facts the report shows for one vulnerable dependency.
#[derive(Debug, Clone)]
pub(crate) struct Advisory {
    pub id: String,
    pub cve: Option<String>,
    pub summary: String,
    pub severity: VulnSeverity,
    /// Fixed versions published for this package, in advisory order.
    pub fixed: Vec<String>,
}

pub(crate) fn advisory_for(vuln: &Value, dep: &Dep) -> Advisory {
    let id = vuln["id"].as_str().unwrap_or("?").to_string();
    let cve = vuln["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .find(|a| a.starts_with("CVE-"))
        .map(str::to_string)
        .or_else(|| id.starts_with("CVE-").then(|| id.clone()));
    let summary = vuln["summary"]
        .as_str()
        .or_else(|| vuln["details"].as_str())
        .unwrap_or("")
        .lines()
        .next()
        .unwrap_or("")
        .chars()
        .take(120)
        .collect();
    let mut fixed: Vec<String> = Vec::new();
    for affected in vuln["affected"].as_array().into_iter().flatten() {
        let pkg = &affected["package"];
        if pkg["name"].as_str() != Some(dep.name.as_str()) {
            continue;
        }
        for range in affected["ranges"].as_array().into_iter().flatten() {
            for event in range["events"].as_array().into_iter().flatten() {
                if let Some(f) = event["fixed"].as_str() {
                    if !fixed.iter().any(|x| x == f) {
                        fixed.push(f.to_string());
                    }
                }
            }
        }
    }
    Advisory { id, cve, summary, severity: osv_severity(vuln), fixed }
}

// ─── OSV client ───

/// Batch size OSV accepts per `querybatch` call.
const OSV_BATCH: usize = 1000;

pub(crate) struct Osv {
    http: reqwest::Client,
    base: String,
}

impl Osv {
    /// `OSV_API_URL` points at a mirror (an air-gapped osv.dev instance); default is
    /// the public API.
    pub(crate) fn from_env() -> Result<Self> {
        let base = std::env::var("OSV_API_URL").unwrap_or_else(|_| "https://api.osv.dev".to_string());
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .map_err(|e| Error::config(format!("OSV client: {e}")))?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string() })
    }

    /// Vulnerability ids per dependency, in input order.
    async fn query_batch(&self, deps: &[Dep]) -> Result<Vec<Vec<String>>> {
        let mut out = Vec::with_capacity(deps.len());
        for chunk in deps.chunks(OSV_BATCH) {
            let queries: Vec<Value> = chunk
                .iter()
                .map(|d| json!({"package": {"name": d.name, "ecosystem": d.ecosystem.osv_name()}, "version": d.version}))
                .collect();
            let resp = self
                .http
                .post(format!("{}/v1/querybatch", self.base))
                .json(&json!({ "queries": queries }))
                .send()
                .await?;
            if !resp.status().is_success() {
                return Err(Error::other(format!("OSV query failed: HTTP {}", resp.status())));
            }
            let body: Value = resp.json().await?;
            let results = body["results"].as_array().cloned().unwrap_or_default();
            if results.len() != chunk.len() {
                return Err(Error::other("OSV returned a result count that does not match the query"));
            }
            out.extend(results.iter().map(|r| {
                r["vulns"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|v| v["id"].as_str().map(str::to_string))
                    .collect()
            }));
        }
        Ok(out)
    }

    pub(crate) async fn vuln(&self, id: &str) -> Result<Value> {
        let resp = self
            .http
            .get(format!("{}/v1/vulns/{}", self.base, urlencoding::encode(id)))
            .send()
            .await?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Err(Error::NotFound(format!("{id} is not in OSV")));
        }
        if !resp.status().is_success() {
            return Err(Error::other(format!("OSV lookup failed: HTTP {}", resp.status())));
        }
        Ok(resp.json().await?)
    }
}

// ─── Audit ───

/// Lockfiles read per repository at most; monorepos beyond this are reported as capped.
const MAX_LOCKFILES_PER_REPO: usize = 12;
/// Tree pages (100 entries each) walked per repository.
const TREE_PAGES: usize = 20;

/// What one repository contributed.
#[derive(Debug, Default)]
pub(crate) struct RepoScan {
    pub path: String,
    /// (lockfile path, dependency count)
    pub lockfiles: Vec<(String, usize)>,
    /// Lockfiles found but not auditable against OSV (e.g. Podfile.lock).
    pub unsupported: Vec<String>,
    pub internal_skipped: usize,
    pub capped: bool,
    pub deps: Vec<Dep>,
    /// Which lockfile each dependency came from (same order as `deps`).
    pub dep_lockfile: Vec<String>,
}

async fn scan_repo(client: &GitLabClient, path: &str, internal_host: &str) -> Result<RepoScan> {
    let enc = urlencoding::encode(path);
    let tree: Vec<Value> = client
        .get_all_pages(
            &format!("/projects/{enc}/repository/tree"),
            &[("recursive", "true"), ("per_page", "100")],
            TREE_PAGES,
        )
        .await?;
    let mut scan = RepoScan { path: path.to_string(), ..Default::default() };
    let mut found: Vec<(String, LockKind)> = tree
        .iter()
        .filter(|e| e["type"].as_str() == Some("blob"))
        .filter_map(|e| e["path"].as_str())
        .filter_map(|p| lockfile_kind(p).map(|k| (p.to_string(), k)))
        .collect();
    found.sort_by_key(|(p, _)| p.matches('/').count());
    if found.len() > MAX_LOCKFILES_PER_REPO {
        scan.capped = true;
        found.truncate(MAX_LOCKFILES_PER_REPO);
    }
    for (file, kind) in found {
        if kind == LockKind::PodfileLock {
            scan.unsupported.push(file);
            continue;
        }
        let text = client
            .get_text(&format!("/projects/{enc}/repository/files/{}/raw", urlencoding::encode(&file)), &[("ref", "HEAD")])
            .await?;
        let parsed = parse_lockfile(kind, &text, internal_host);
        scan.internal_skipped += parsed.internal_skipped;
        scan.lockfiles.push((file.clone(), parsed.deps.len()));
        for d in parsed.deps {
            scan.dep_lockfile.push(file.clone());
            scan.deps.push(d);
        }
    }
    Ok(scan)
}

/// One finding: an advisory against a dependency, with every place it is pinned.
#[derive(Debug, Clone)]
pub(crate) struct Finding {
    pub dep: Dep,
    pub advisory: Advisory,
    /// "repo:lockfile" locations.
    pub locations: Vec<String>,
}

/// Audit pinned dependencies across one project or a whole group.
pub async fn audit_dependencies(
    client: &GitLabClient,
    project_id: &str,
    group_path: &str,
    max_projects: usize,
) -> Result<String> {
    let (targets, total_repos, scope) = if !group_path.is_empty() {
        let projects: Vec<Value> = client
            .get_all_pages(
                &format!("/groups/{}/projects", urlencoding::encode(group_path)),
                &[
                    ("include_subgroups", "true"),
                    ("archived", "false"),
                    ("order_by", "last_activity_at"),
                    ("sort", "desc"),
                ],
                10,
            )
            .await?;
        let total = projects.len();
        let targets: Vec<String> = projects
            .iter()
            .take(max_projects)
            .filter_map(|p| p["path_with_namespace"].as_str().map(str::to_string))
            .collect();
        (targets, total, format!("group `{group_path}`"))
    } else if !project_id.is_empty() {
        (vec![project_id.to_string()], 1, format!("project `{project_id}`"))
    } else {
        return Err(Error::user_input("pass project_id (one repo) or group_path (a whole group)."));
    };

    let internal_host = client.host();
    let mut fan = FanOut::default();
    let mut scans: Vec<RepoScan> = Vec::new();
    for chunk in targets.chunks(6) {
        let results = join_all(chunk.iter().map(|p| scan_repo(client, p, &internal_host))).await;
        scans.extend(results.into_iter().filter_map(|r| fan.take(r)));
    }
    let warning = fan.finish("repository scans")?.unwrap_or_default();

    // Each distinct dependency is queried once, however many repos pin it.
    let unique: Vec<Dep> = scans
        .iter()
        .flat_map(|s| s.deps.iter().cloned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect();
    let osv = Osv::from_env()?;
    let ids_per_dep = if unique.is_empty() { Vec::new() } else { osv.query_batch(&unique).await? };

    let ids: BTreeSet<&String> = ids_per_dep.iter().flatten().collect();
    let mut records: BTreeMap<String, Value> = BTreeMap::new();
    let mut lookup = FanOut::default();
    let ids: Vec<&String> = ids.into_iter().collect();
    for chunk in ids.chunks(8) {
        let fetched = join_all(chunk.iter().map(|id| osv.vuln(id))).await;
        for (id, r) in chunk.iter().zip(fetched) {
            if let Some(v) = lookup.take(r) {
                records.insert((*id).clone(), v);
            }
        }
    }
    let lookup_warning = lookup.finish("advisory lookups")?.unwrap_or_default();

    let mut findings: Vec<Finding> = Vec::new();
    for (dep, ids) in unique.iter().zip(&ids_per_dep) {
        let locations: Vec<String> = scans
            .iter()
            .flat_map(|s| {
                s.deps
                    .iter()
                    .zip(&s.dep_lockfile)
                    .filter(move |(d, _)| *d == dep)
                    .map(move |(_, f)| format!("{}:{f}", s.path))
            })
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        for id in ids {
            if let Some(v) = records.get(id) {
                findings.push(Finding { dep: dep.clone(), advisory: advisory_for(v, dep), locations: locations.clone() });
            }
        }
    }

    Ok(format!(
        "{warning}{lookup_warning}{}",
        render_dependency_audit(&scope, total_repos, &scans, unique.len(), findings)
    ))
}

/// The audit report. Pure: ordering, counts, the coverage notes and the "nothing to
/// audit" cases are verified on plain values.
pub(crate) fn render_dependency_audit(
    scope: &str,
    total_repos: usize,
    scans: &[RepoScan],
    unique_deps: usize,
    mut findings: Vec<Finding>,
) -> String {
    findings.sort_by(|a, b| {
        (a.advisory.severity, &a.dep.ecosystem, &a.dep.name, &a.advisory.id)
            .cmp(&(b.advisory.severity, &b.dep.ecosystem, &b.dep.name, &b.advisory.id))
    });
    let lockfiles: usize = scans.iter().map(|s| s.lockfiles.len()).sum();
    let mut by_sev: BTreeMap<VulnSeverity, usize> = BTreeMap::new();
    for f in &findings {
        *by_sev.entry(f.advisory.severity).or_default() += 1;
    }
    let vulnerable_pkgs: BTreeSet<&Dep> = findings.iter().map(|f| &f.dep).collect();
    let mut out = vec![
        format!("# Dependency vulnerability audit — {scope}"),
        String::new(),
        format!(
            "**{} repo(s) scanned** of {total_repos} · **{lockfiles} lockfile(s)** · **{unique_deps} distinct packages** · **{} vulnerable** · {} advisories",
            scans.len(),
            vulnerable_pkgs.len(),
            findings.len()
        ),
    ];
    if !findings.is_empty() {
        out.push(
            by_sev
                .iter()
                .map(|(s, n)| format!("{} {n}", s.label()))
                .collect::<Vec<_>>()
                .join(" · "),
        );
    }
    out.push(String::new());
    if findings.is_empty() {
        out.push(if unique_deps == 0 {
            "No auditable dependencies found — see coverage below; this is not a clean bill of health.".to_string()
        } else {
            "No known vulnerabilities in the pinned versions.".to_string()
        });
    } else {
        out.push("| Severity | Package | Advisory | Fixed in | Where |".to_string());
        out.push("|----------|---------|----------|----------|-------|".to_string());
        const MAX_ROWS: usize = 150;
        for f in findings.iter().take(MAX_ROWS) {
            let advisory = match &f.advisory.cve {
                Some(cve) if *cve != f.advisory.id => format!("{} ({cve})", f.advisory.id),
                _ => f.advisory.id.clone(),
            };
            let fixed = if f.advisory.fixed.is_empty() { "no fix yet".to_string() } else { f.advisory.fixed.join(", ") };
            let where_ = match f.locations.len() {
                0 => "–".to_string(),
                1 => f.locations[0].clone(),
                n => format!("{} +{} more", f.locations[0], n - 1),
            };
            out.push(format!(
                "| {} | {}@{} ({}) | {advisory} — {} | {fixed} | {where_} |",
                f.advisory.severity.label(),
                f.dep.name,
                f.dep.version,
                f.dep.ecosystem.osv_name(),
                f.advisory.summary.replace('|', "/"),
            ));
        }
        if findings.len() > MAX_ROWS {
            out.push(format!("\n_{} more advisories not shown — narrow to a project._", findings.len() - MAX_ROWS));
        }
    }

    // Coverage: what could not be audited is part of the answer.
    let no_lockfile: Vec<&str> = scans
        .iter()
        .filter(|s| s.lockfiles.is_empty() && s.unsupported.is_empty())
        .map(|s| s.path.as_str())
        .collect();
    let unsupported: Vec<String> = scans
        .iter()
        .flat_map(|s| s.unsupported.iter().map(move |f| format!("{}:{f}", s.path)))
        .collect();
    let capped: Vec<&str> = scans.iter().filter(|s| s.capped).map(|s| s.path.as_str()).collect();
    let internal: usize = scans.iter().map(|s| s.internal_skipped).sum();
    let mut notes: Vec<String> = Vec::new();
    if !no_lockfile.is_empty() {
        notes.push(format!(
            "- **No supported lockfile** ({}): {} — dependencies there are not pinned, so they cannot be audited.",
            no_lockfile.len(),
            no_lockfile.join(", ")
        ));
    }
    if !unsupported.is_empty() {
        notes.push(format!(
            "- **Not auditable against OSV** ({}): {} — no advisory feed for that ecosystem.",
            unsupported.len(),
            unsupported.join(", ")
        ));
    }
    if !capped.is_empty() {
        notes.push(format!(
            "- **Lockfiles capped at {MAX_LOCKFILES_PER_REPO}** in: {} — audit those projects individually.",
            capped.join(", ")
        ));
    }
    if internal > 0 {
        notes.push(format!(
            "- {internal} internal or non-registry dependencies (GitLab-hosted, git or path sources) were kept out of the query and not audited."
        ));
    }
    if scans.len() < total_repos {
        notes.push(format!("- Only the {} most recently active of {total_repos} repos were scanned; raise `max_projects` to cover more.", scans.len()));
    }
    if !notes.is_empty() {
        out.push(String::new());
        out.push("## Coverage".to_string());
        out.extend(notes);
    }
    out.push(String::new());
    let mut footer = String::new();
    let _ = write!(
        footer,
        "_Matched against OSV.dev (GitHub, RustSec, Go, PyPA and other advisory feeds). Only exact pinned versions are checked; severity is the advisory's own rating, else its CVSS v3 base score._"
    );
    out.push(footer);
    out.join("\n")
}

/// Look up one advisory by id: CVE, GHSA, RUSTSEC, GO, PYSEC and other OSV ids.
pub async fn get_vulnerability(id: &str) -> Result<String> {
    let id = id.trim();
    if id.is_empty() {
        return Err(Error::user_input("pass an advisory id, e.g. CVE-2024-3094 or GHSA-…"));
    }
    let v = Osv::from_env()?.vuln(id).await?;
    Ok(render_vulnerability(&v))
}

pub(crate) fn render_vulnerability(v: &Value) -> String {
    let id = v["id"].as_str().unwrap_or("?");
    let aliases: Vec<&str> = v["aliases"].as_array().into_iter().flatten().filter_map(Value::as_str).collect();
    let mut out = vec![format!("# {id} — {}", osv_severity(v).label())];
    if !aliases.is_empty() {
        out.push(format!("**Aliases:** {}", aliases.join(", ")));
    }
    if let Some(s) = v["summary"].as_str() {
        out.push(format!("**Summary:** {s}"));
    }
    if let Some(p) = v["published"].as_str() {
        out.push(format!("**Published:** {}", p.get(..10).unwrap_or(p)));
    }
    out.push(String::new());
    out.push("| Ecosystem | Package | Fixed in |".to_string());
    out.push("|-----------|---------|----------|".to_string());
    for affected in v["affected"].as_array().into_iter().flatten().take(20) {
        let pkg = &affected["package"];
        let fixed: Vec<&str> = affected["ranges"]
            .as_array()
            .into_iter()
            .flatten()
            .flat_map(|r| r["events"].as_array().into_iter().flatten())
            .filter_map(|e| e["fixed"].as_str())
            .collect();
        out.push(format!(
            "| {} | {} | {} |",
            pkg["ecosystem"].as_str().unwrap_or("?"),
            pkg["name"].as_str().unwrap_or("?"),
            if fixed.is_empty() { "no fix yet".to_string() } else { fixed.join(", ") }
        ));
    }
    if let Some(d) = v["details"].as_str() {
        out.push(String::new());
        out.push(d.chars().take(1500).collect());
    }
    let refs: Vec<&str> = v["references"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|r| r["url"].as_str())
        .take(5)
        .collect();
    if !refs.is_empty() {
        out.push(String::new());
        out.push("**References:**".to_string());
        out.extend(refs.iter().map(|r| format!("- {r}")));
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    const HOST: &str = "git.example.com";

    fn names(p: &Parsed) -> Vec<String> {
        p.deps.iter().map(|d| format!("{}@{}", d.name, d.version)).collect()
    }

    #[test]
    fn lockfiles_are_recognised_by_name_and_vendored_trees_ignored() {
        assert_eq!(lockfile_kind("Cargo.lock"), Some(LockKind::CargoLock));
        assert_eq!(lockfile_kind("web/package-lock.json"), Some(LockKind::PackageLock));
        assert_eq!(lockfile_kind("requirements-dev.txt"), Some(LockKind::Requirements));
        assert_eq!(lockfile_kind("ios/Podfile.lock"), Some(LockKind::PodfileLock));
        assert_eq!(lockfile_kind("web/node_modules/x/package-lock.json"), None);
        assert_eq!(lockfile_kind("vendor/foo/composer.lock"), None);
        assert_eq!(lockfile_kind("src/main.rs"), None);
    }

    #[test]
    fn cargo_lock_keeps_registry_crates_and_skips_git_and_workspace_members() {
        let lock = r#"
[[package]]
name = "serde"
version = "1.0.200"
source = "registry+https://github.com/rust-lang/crates.io-index"

[[package]]
name = "internal-sdk"
version = "0.3.0"
source = "git+https://git.example.com/acme/sdk.git#abc"

[[package]]
name = "my-app"
version = "0.1.0"
"#;
        let p = parse_lockfile(LockKind::CargoLock, lock, HOST);
        assert_eq!(names(&p), vec!["serde@1.0.200"]);
        assert_eq!(p.internal_skipped, 1, "the git crate is internal; the workspace member is ours, not a dependency");
        assert!(p.deps.iter().all(|d| d.ecosystem == Ecosystem::CratesIo));
    }

    #[test]
    fn package_lock_v3_and_v1_skip_internal_and_linked_packages() {
        let v3 = serde_json::json!({"lockfileVersion": 3, "packages": {
            "": {"name": "app"},
            "node_modules/lodash": {"version": "4.17.20", "resolved": "https://registry.npmjs.org/lodash/-/lodash-4.17.20.tgz"},
            "node_modules/@scope/pkg": {"version": "1.2.3", "resolved": "https://registry.npmjs.org/@scope/pkg/-/pkg-1.2.3.tgz"},
            "node_modules/a/node_modules/minimist": {"version": "0.0.8"},
            "node_modules/internal": {"version": "1.0.0", "resolved": "https://git.example.com/api/v4/projects/1/packages/npm/internal.tgz"},
            "node_modules/local": {"link": true},
        }});
        let p = parse_lockfile(LockKind::PackageLock, &v3.to_string(), HOST);
        let mut got = names(&p);
        got.sort();
        assert_eq!(got, vec!["@scope/pkg@1.2.3", "lodash@4.17.20", "minimist@0.0.8"]);
        assert_eq!(p.internal_skipped, 1);

        let v1 = serde_json::json!({"lockfileVersion": 1, "dependencies": {
            "express": {"version": "4.17.1", "dependencies": {"qs": {"version": "6.7.0"}}},
            "own": {"version": "git+https://git.example.com/acme/own.git"},
        }});
        let p = parse_lockfile(LockKind::PackageLock, &v1.to_string(), HOST);
        let mut got = names(&p);
        got.sort();
        assert_eq!(got, vec!["express@4.17.1", "qs@6.7.0"]);
        assert_eq!(p.internal_skipped, 1);
    }

    #[test]
    fn yarn_v1_and_berry_lockfiles_resolve_scoped_names() {
        let v1 = "# yarn lockfile v1\n\n\"@babel/core@^7.0.0\", \"@babel/core@^7.1.0\":\n  version \"7.24.0\"\n  resolved \"https://registry.yarnpkg.com/@babel/core/-/core-7.24.0.tgz\"\n\nleft-pad@^1.0.0:\n  version \"1.3.0\"\n\ninternal@^1.0.0:\n  version \"1.0.0\"\n  resolved \"https://git.example.com/acme/internal.tgz\"\n";
        let p = parse_lockfile(LockKind::YarnLock, v1, HOST);
        assert_eq!(names(&p), vec!["@babel/core@7.24.0", "left-pad@1.3.0"]);
        assert_eq!(p.internal_skipped, 1);
        let berry = "__metadata:\n  version: 6\n\n\"chalk@npm:^5.0.0\":\n  version: 5.3.0\n  resolution: \"chalk@npm:5.3.0\"\n\n\"app@workspace:.\":\n  version: 0.0.0-use.local\n";
        let p = parse_lockfile(LockKind::YarnLock, berry, HOST);
        assert_eq!(names(&p), vec!["chalk@5.3.0"]);
    }

    #[test]
    fn pnpm_keys_of_every_lockfile_version_parse() {
        let lock = "lockfileVersion: '9.0'\n\npackages:\n\n  '@types/node@20.11.0':\n    resolution: {integrity: x}\n\n  /react@18.2.0:\n    resolution: {integrity: x}\n\n  /lodash/4.17.21:\n    resolution: {integrity: x}\n\n  /react-dom@18.2.0(react@18.2.0):\n    resolution: {integrity: x}\n\nsnapshots:\n\n  ignored@1.0.0: {}\n";
        let p = parse_lockfile(LockKind::PnpmLock, lock, HOST);
        assert_eq!(names(&p), vec!["@types/node@20.11.0", "react@18.2.0", "lodash@4.17.21", "react-dom@18.2.0"]);
    }

    #[test]
    fn composer_lock_strips_v_skips_branches_and_internal_sources() {
        let lock = serde_json::json!({
            "packages": [
                {"name": "guzzlehttp/guzzle", "version": "7.4.0", "source": {"url": "https://github.com/guzzle/guzzle.git"}},
                {"name": "acme/billing", "version": "v2.0.0", "source": {"url": "https://git.example.com/acme/billing.git"}},
                {"name": "acme/wip", "version": "dev-main"},
            ],
            "packages-dev": [{"name": "phpunit/phpunit", "version": "v9.5.0"}]
        });
        let p = parse_lockfile(LockKind::ComposerLock, &lock.to_string(), HOST);
        assert_eq!(names(&p), vec!["guzzlehttp/guzzle@7.4.0", "phpunit/phpunit@9.5.0"]);
        assert_eq!(p.internal_skipped, 1);
    }

    #[test]
    fn go_sum_dedupes_modules_and_keeps_internal_modules_out() {
        let sum = "golang.org/x/net v0.17.0 h1:abc=\ngolang.org/x/net v0.17.0/go.mod h1:def=\ngithub.com/old/lib v2.0.0+incompatible h1:x=\ngit.example.com/acme/sdk v1.0.0 h1:y=\ngit.example.com/acme/sdk v1.0.0/go.mod h1:z=\n";
        let p = parse_lockfile(LockKind::GoSum, sum, HOST);
        assert_eq!(names(&p), vec!["golang.org/x/net@0.17.0", "github.com/old/lib@2.0.0"]);
        assert_eq!(p.internal_skipped, 1, "one internal module, counted once");
    }

    #[test]
    fn python_lockfiles_take_exact_pins_only() {
        let req = "requests==2.31.0\nflask>=2.0  # a range is not a version\nuvicorn[standard]==0.29.0 ; python_version > '3.8'\n-r base.txt\nown @ git+https://git.example.com/acme/own.git\n";
        let p = parse_lockfile(LockKind::Requirements, req, HOST);
        assert_eq!(names(&p), vec!["requests@2.31.0", "uvicorn@0.29.0"]);
        assert_eq!(p.internal_skipped, 1);
        let poetry = "[[package]]\nname = \"urllib3\"\nversion = \"1.26.5\"\n\n[[package]]\nname = \"own\"\nversion = \"0.1.0\"\n[package.source]\ntype = \"git\"\nurl = \"https://git.example.com/acme/own.git\"\n";
        let p = parse_lockfile(LockKind::PoetryLock, poetry, HOST);
        assert_eq!(names(&p), vec!["urllib3@1.26.5"]);
        let pipfile = serde_json::json!({"default": {"jinja2": {"version": "==3.1.2"}, "own": {"git": "https://x"}}, "develop": {"pytest": {"version": "==8.0.0"}}});
        let p = parse_lockfile(LockKind::PipfileLock, &pipfile.to_string(), HOST);
        assert_eq!(names(&p), vec!["jinja2@3.1.2", "pytest@8.0.0"]);
    }

    #[test]
    fn gemfile_and_gradle_lockfiles_parse() {
        let gem = "GIT\n  remote: https://git.example.com/acme/gem.git\n  specs:\n    owngem (0.1.0)\n\nGEM\n  remote: https://rubygems.org/\n  specs:\n    rack (2.2.3)\n    nokogiri (1.15.0-arm64-darwin)\n      racc (~> 1.4)\n\nPLATFORMS\n  ruby\n";
        let p = parse_lockfile(LockKind::GemfileLock, gem, HOST);
        assert_eq!(names(&p), vec!["rack@2.2.3", "nokogiri@1.15.0"]);
        assert_eq!(p.internal_skipped, 1);
        let gradle = "# comment\ncom.fasterxml.jackson.core:jackson-databind:2.13.0=compileClasspath\nempty=annotationProcessor\n";
        let p = parse_lockfile(LockKind::GradleLockfile, gradle, HOST);
        assert_eq!(names(&p), vec!["com.fasterxml.jackson.core:jackson-databind@2.13.0"]);
        assert!(parse_lockfile(LockKind::CargoLock, "not toml [[[", HOST).deps.is_empty(), "garbage yields nothing, no panic");
    }

    #[test]
    fn cvss_v3_base_scores_match_the_specification() {
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), Some(9.8));
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:C/C:H/I:H/A:H"), Some(10.0));
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:R/S:C/C:L/I:L/A:N"), Some(6.1));
        assert_eq!(cvss3_base_score("CVSS:3.0/AV:L/AC:L/PR:L/UI:N/S:U/C:H/I:N/A:N"), Some(5.5));
        assert_eq!(cvss3_base_score("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:N/I:N/A:N"), Some(0.0));
        assert_eq!(cvss3_base_score("CVSS:4.0/AV:N"), None, "v4 vectors are not scored here");
        assert_eq!(cvss3_base_score("garbage"), None);
    }

    fn vuln(id: &str, rating: Option<&str>, vector: Option<&str>, pkg: &str, fixed: &[&str]) -> Value {
        let events: Vec<Value> = std::iter::once(json!({"introduced": "0"}))
            .chain(fixed.iter().map(|f| json!({"fixed": f})))
            .collect();
        let mut v = json!({
            "id": id, "summary": "Prototype pollution in merge", "aliases": ["CVE-2020-8203"],
            "affected": [
                {"package": {"name": pkg, "ecosystem": "npm"}, "ranges": [{"type": "SEMVER", "events": events}]},
                {"package": {"name": "other", "ecosystem": "npm"}, "ranges": [{"type": "SEMVER", "events": [{"fixed": "9.9.9"}]}]},
            ],
        });
        if let Some(r) = rating {
            v["database_specific"] = json!({"severity": r});
        }
        if let Some(vec) = vector {
            v["severity"] = json!([{"type": "CVSS_V3", "score": vec}]);
        }
        v
    }

    #[test]
    fn severity_prefers_the_advisory_rating_then_cvss() {
        assert_eq!(osv_severity(&vuln("G", Some("MODERATE"), Some("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), "x", &[])), VulnSeverity::Medium);
        assert_eq!(osv_severity(&vuln("R", None, Some("CVSS:3.1/AV:N/AC:L/PR:N/UI:N/S:U/C:H/I:H/A:H"), "x", &[])), VulnSeverity::Critical);
        assert_eq!(osv_severity(&vuln("U", None, None, "x", &[])), VulnSeverity::Unrated);
    }

    #[test]
    fn advisories_keep_only_this_packages_fixed_versions_and_the_cve_alias() {
        let dep = Dep { ecosystem: Ecosystem::Npm, name: "lodash".into(), version: "4.17.15".into() };
        let a = advisory_for(&vuln("GHSA-p6mc-m468-83gw", Some("HIGH"), None, "lodash", &["4.17.19"]), &dep);
        assert_eq!(a.cve.as_deref(), Some("CVE-2020-8203"));
        assert_eq!(a.fixed, vec!["4.17.19".to_string()], "the other package's fix is not ours");
        assert_eq!(a.severity, VulnSeverity::High);
    }

    fn scan(path: &str, lockfiles: &[(&str, usize)], deps: &[(&str, &str)]) -> RepoScan {
        RepoScan {
            path: path.into(),
            lockfiles: lockfiles.iter().map(|(f, n)| (f.to_string(), *n)).collect(),
            deps: deps.iter().map(|(n, v)| Dep { ecosystem: Ecosystem::Npm, name: (*n).into(), version: (*v).into() }).collect(),
            dep_lockfile: deps.iter().map(|_| lockfiles.first().map_or(String::new(), |l| l.0.to_string())).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn golden_dependency_audit_report() {
        let lodash = Dep { ecosystem: Ecosystem::Npm, name: "lodash".into(), version: "4.17.15".into() };
        let minimist = Dep { ecosystem: Ecosystem::Npm, name: "minimist".into(), version: "0.0.8".into() };
        let findings = vec![
            Finding {
                dep: minimist.clone(),
                advisory: advisory_for(&vuln("GHSA-xvch-5gv4-984h", Some("CRITICAL"), None, "minimist", &["0.2.4", "1.2.6"]), &minimist),
                locations: vec!["acme/web:package-lock.json".into()],
            },
            Finding {
                dep: lodash.clone(),
                advisory: advisory_for(&vuln("GHSA-p6mc-m468-83gw", Some("HIGH"), None, "lodash", &["4.17.19"]), &lodash),
                locations: vec!["acme/web:package-lock.json".into(), "acme/admin:yarn.lock".into()],
            },
        ];
        let mut no_lock = scan("acme/docs", &[], &[]);
        no_lock.internal_skipped = 3;
        let mut ios = scan("acme/ios", &[], &[]);
        ios.unsupported = vec!["Podfile.lock".into()];
        let scans = vec![
            scan("acme/web", &[("package-lock.json", 120)], &[("lodash", "4.17.15"), ("minimist", "0.0.8")]),
            scan("acme/admin", &[("yarn.lock", 80)], &[("lodash", "4.17.15")]),
            no_lock,
            ios,
        ];
        let out = render_dependency_audit("group `acme`", 6, &scans, 180, findings);
        crate::golden::assert_golden("render/dependency_audit.md", &out);
        assert!(out.contains("CRITICAL 1 · HIGH 1"));
        assert!(out.find("minimist").unwrap() < out.find("| HIGH").unwrap(), "most severe first");
        assert!(out.contains("acme/web:package-lock.json +1 more"));
        assert!(out.contains("No supported lockfile** (1): acme/docs"));
        assert!(out.contains("Not auditable against OSV** (1): acme/ios:Podfile.lock"));
        assert!(out.contains("3 internal or non-registry dependencies"));
        assert!(out.contains("Only the 4 most recently active of 6 repos"));
    }

    #[test]
    fn nothing_auditable_is_never_reported_as_clean() {
        let out = render_dependency_audit("project `p`", 1, &[scan("p", &[], &[])], 0, vec![]);
        assert!(out.contains("not a clean bill of health"));
        assert!(!out.contains("No known vulnerabilities"));
        let clean = render_dependency_audit("project `p`", 1, &[scan("p", &[("Cargo.lock", 3)], &[("x", "1")])], 3, vec![]);
        assert!(clean.contains("No known vulnerabilities in the pinned versions."));
    }

    #[test]
    fn a_vulnerability_record_renders_aliases_fixes_and_references() {
        let mut v = vuln("GHSA-p6mc-m468-83gw", Some("HIGH"), None, "lodash", &["4.17.19"]);
        v["published"] = json!("2020-07-15T19:15:00Z");
        v["references"] = json!([{"url": "https://github.com/advisories/GHSA-p6mc-m468-83gw"}]);
        let out = render_vulnerability(&v);
        assert!(out.starts_with("# GHSA-p6mc-m468-83gw — HIGH"));
        assert!(out.contains("**Aliases:** CVE-2020-8203"));
        assert!(out.contains("| npm | lodash | 4.17.19 |"));
        assert!(out.contains("**Published:** 2020-07-15"));
    }
}
