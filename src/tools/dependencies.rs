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

use futures::future::join_all;
use serde_json::{json, Value};

use crate::client::GitLabClient;
use crate::error::{Error, FanOut, Result};

use super::infra;
use super::npm_roles::{self, Role};

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
    /// Swift Package Manager, keyed by repository URL.
    SwiftURL,
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
            Self::SwiftURL => "SwiftURL",
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
    /// The build list Go actually selects — preferred over `go.sum`, which keeps a
    /// checksum for every version ever seen in the module graph.
    GoMod,
    /// Swift Package Manager pins.
    SwiftResolved,
    /// Gradle version catalog: declared versions of direct dependencies only.
    GradleCatalog,
    PoetryLock,
    PipfileLock,
    Requirements,
    GemfileLock,
    GradleLockfile,
    /// Recognised but not auditable against OSV (no CocoaPods feed).
    PodfileLock,
    /// Terraform provider pins; providers are Go modules in OSV.
    TerraformLock,
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
        "go.mod" => LockKind::GoMod,
        "Package.resolved" => LockKind::SwiftResolved,
        "libs.versions.toml" => LockKind::GradleCatalog,
        "poetry.lock" => LockKind::PoetryLock,
        "Pipfile.lock" => LockKind::PipfileLock,
        "Gemfile.lock" => LockKind::GemfileLock,
        "gradle.lockfile" => LockKind::GradleLockfile,
        "Podfile.lock" => LockKind::PodfileLock,
        ".terraform.lock.hcl" => LockKind::TerraformLock,
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
    /// Parallel to `deps`: declared as a development-only dependency.
    pub dev: Vec<bool>,
    pub internal_skipped: usize,
    /// Pinned to a branch or commit rather than a release: nothing to match.
    pub unversioned: usize,
}

impl Parsed {
    fn push(&mut self, ecosystem: Ecosystem, name: &str, version: &str) {
        self.push_dev(ecosystem, name, version, false);
    }

    fn push_dev(&mut self, ecosystem: Ecosystem, name: &str, version: &str, dev: bool) {
        let (name, version) = (name.trim(), version.trim());
        if !name.is_empty() && !version.is_empty() {
            self.deps.push(Dep { ecosystem, name: name.to_string(), version: version.to_string() });
            self.dev.push(dev);
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
                p.push_dev(eco, get("name"), get("version"), get("category") == "dev");
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
                    let dev = info["dev"].as_bool() == Some(true) || info["devOptional"].as_bool() == Some(true);
                    p.push_dev(Ecosystem::Npm, name, info["version"].as_str().unwrap_or(""), dev);
                }
            } else {
                // lockfileVersion 1: nested "dependencies".
                fn walk(deps: &Value, host: &str, p: &mut Parsed) {
                    for (name, info) in deps.as_object().into_iter().flatten() {
                        let version = info["version"].as_str().unwrap_or("");
                        if is_internal_source(info["resolved"].as_str().unwrap_or(version), host) {
                            p.internal_skipped += 1;
                        } else {
                            p.push_dev(Ecosystem::Npm, name, version, info["dev"].as_bool() == Some(true));
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
                    // Branches, not releases: `dev-main`, and branch aliases like `12.x-dev`.
                    // OSV would match the branch name as if it were a version.
                    if version.starts_with("dev-") || version.ends_with("-dev") {
                        p.unversioned += 1;
                        continue;
                    }
                    p.push_dev(
                        Ecosystem::Packagist,
                        pkg["name"].as_str().unwrap_or(""),
                        version.trim_start_matches('v'),
                        section == "packages-dev",
                    );
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
                        p.push_dev(Ecosystem::PyPI, name, v, section == "develop");
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
        LockKind::GoMod => {
            // `require x vY` lines and `require ( … )` blocks. `replace` directives
            // win: a replacement with a version is audited instead of the original,
            // one pointing at a local path is internal.
            let mut required: Vec<(String, String)> = Vec::new();
            let mut replaced: BTreeMap<String, Option<(String, String)>> = BTreeMap::new();
            let mut block = "";
            for raw in text.lines() {
                let line = raw.split("//").next().unwrap_or("").trim();
                if line.is_empty() {
                    continue;
                }
                if line == ")" {
                    block = "";
                    continue;
                }
                let (directive, rest) = match line.split_once(char::is_whitespace) {
                    Some((d, r)) if matches!(d, "require" | "replace" | "exclude" | "retract") => (d, r.trim()),
                    _ if !block.is_empty() => (block, line),
                    _ => continue,
                };
                if rest == "(" {
                    block = directive;
                    continue;
                }
                match directive {
                    "require" => {
                        let mut it = rest.split_whitespace();
                        if let (Some(m), Some(v)) = (it.next(), it.next()) {
                            required.push((m.to_string(), v.to_string()));
                        }
                    }
                    "replace" => {
                        if let Some((from, to)) = rest.split_once("=>") {
                            let from = from.split_whitespace().next().unwrap_or("").to_string();
                            let mut t = to.split_whitespace();
                            let target = match (t.next(), t.next()) {
                                (Some(m), Some(v)) if !m.starts_with('.') && !m.starts_with('/') => Some((m.to_string(), v.to_string())),
                                _ => None,
                            };
                            replaced.insert(from, target);
                        }
                    }
                    _ => {}
                }
            }
            for (module, version) in required {
                let (module, version) = match replaced.get(&module) {
                    Some(Some((m, v))) => (m.clone(), v.clone()),
                    Some(None) => {
                        p.internal_skipped += 1;
                        continue;
                    }
                    None => (module, version),
                };
                if !internal_host.is_empty() && module.starts_with(internal_host) {
                    p.internal_skipped += 1;
                    continue;
                }
                let v = version.trim_end_matches("+incompatible");
                p.push(Ecosystem::Go, &module, v.trim_start_matches('v'));
            }
        }
        LockKind::SwiftResolved => {
            // v1: object.pins[].{repositoryURL, state.version}; v2/v3: pins[].{location,
            // state.version}. OSV keys Swift packages by URL without scheme or ".git".
            let Ok(doc) = serde_json::from_str::<Value>(text) else { return p };
            let pins = doc["pins"].as_array().or_else(|| doc["object"]["pins"].as_array());
            for pin in pins.into_iter().flatten() {
                let url = pin["location"].as_str().or_else(|| pin["repositoryURL"].as_str()).unwrap_or("");
                let remote = url.starts_with("https://") || url.starts_with("http://") || url.starts_with("git@");
                if !remote || (!internal_host.is_empty() && url.contains(internal_host)) {
                    p.internal_skipped += 1;
                    continue;
                }
                let Some(version) = pin["state"]["version"].as_str() else {
                    p.unversioned += 1; // pinned to a branch or revision
                    continue;
                };
                let name = url
                    .trim_start_matches("https://")
                    .trim_start_matches("http://")
                    .trim_start_matches("git@")
                    .replacen(':', "/", usize::from(url.starts_with("git@")))
                    .trim_end_matches('/')
                    .trim_end_matches(".git")
                    .to_string();
                p.push(Ecosystem::SwiftURL, &name, version);
            }
        }
        LockKind::GradleCatalog => {
            let Ok(doc) = text.parse::<toml::Table>() else { return p };
            let version_of = |v: &toml::Value| -> Option<String> {
                match v {
                    toml::Value::String(s) => Some(s.clone()),
                    toml::Value::Table(t) => ["strictly", "require", "prefer"]
                        .iter()
                        .find_map(|k| t.get(*k).and_then(|x| x.as_str()).map(str::to_string)),
                    _ => None,
                }
            };
            let versions = doc.get("versions").and_then(|v| v.as_table());
            for lib in doc.get("libraries").and_then(|v| v.as_table()).into_iter().flat_map(|t| t.values()) {
                let (coords, version) = match lib {
                    toml::Value::String(s) => match s.rsplitn(2, ':').collect::<Vec<_>>().as_slice() {
                        [v, ga] if ga.contains(':') => (ga.to_string(), Some(v.to_string())),
                        _ => continue,
                    },
                    toml::Value::Table(t) => {
                        let coords = match (t.get("module").and_then(|m| m.as_str()), t.get("group").and_then(|g| g.as_str()), t.get("name").and_then(|n| n.as_str())) {
                            (Some(m), _, _) => m.to_string(),
                            (None, Some(g), Some(n)) => format!("{g}:{n}"),
                            _ => continue,
                        };
                        let version = match t.get("version") {
                            Some(toml::Value::Table(vt)) if vt.contains_key("ref") => vt
                                .get("ref")
                                .and_then(|r| r.as_str())
                                .and_then(|r| versions.and_then(|vs| vs.get(r)))
                                .and_then(version_of),
                            Some(v) => version_of(v),
                            None => None,
                        };
                        (coords, version)
                    }
                    _ => continue,
                };
                match version {
                    // Only exact versions can be matched; ranges and dynamic versions can't.
                    Some(v) if !v.contains(['+', '[', '(', ',']) && !v.is_empty() => p.push(Ecosystem::Maven, &coords, &v),
                    _ => p.unversioned += 1,
                }
            }
        }
        LockKind::PodfileLock => {}
        LockKind::TerraformLock => {
            // provider "registry.terraform.io/<ns>/<name>" { version = "x" … }. A
            // provider is the Go module github.com/<ns>/terraform-provider-<name>,
            // which is how its advisories are published.
            let mut current: Option<String> = None;
            for line in text.lines() {
                let t = line.trim();
                if let Some(rest) = t.strip_prefix("provider \"") {
                    let source = rest.split('"').next().unwrap_or("");
                    let parts: Vec<&str> = source.split('/').collect();
                    current = match parts.as_slice() {
                        [host, _, _] if !internal_host.is_empty() && host.contains(internal_host) => {
                            p.internal_skipped += 1;
                            None
                        }
                        [_, ns, name] => Some(format!("github.com/{ns}/terraform-provider-{name}")),
                        _ => None,
                    };
                } else if let (Some(module), Some(v)) = (current.as_ref(), t.strip_prefix("version")) {
                    if let Some(ver) = v.trim_start().strip_prefix('=').map(|x| x.trim().trim_matches('"')) {
                        p.push(Ecosystem::Go, module, ver);
                        current = None;
                    }
                }
            }
        }
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
    /// Other ids of the same issue (CVE, GHSA, GO-, RUSTSEC-…).
    pub aliases: Vec<String>,
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
    let aliases = vuln["aliases"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_string)
        .collect();
    Advisory { id, cve, summary, severity: osv_severity(vuln), fixed, aliases }
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
/// Infrastructure files read per repository at most.
const MAX_INFRA_FILES_PER_REPO: usize = 40;
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
    /// Gradle version catalogs: declared direct dependencies, not a resolved graph.
    pub declared_only: Vec<String>,
    pub internal_skipped: usize,
    pub unversioned: usize,
    pub capped: bool,
    pub deps: Vec<Dep>,
    /// Which lockfile each dependency came from (same order as `deps`).
    pub dep_lockfile: Vec<String>,
    /// Development-only, per dependency (same order as `deps`).
    pub dep_dev: Vec<bool>,
    /// Reached only through build tooling (npm), per dependency (same order as `deps`).
    pub dep_build: Vec<bool>,
    /// Infrastructure files read (Dockerfiles, compose/CI/k8s YAML, Terraform).
    pub infra_files: usize,
    pub infra_capped: bool,
    /// (file, declared version) for images and platform versions.
    pub infra_refs: Vec<(String, infra::InfraRef)>,
    /// (file, image) public images without a version.
    pub unpinned_images: Vec<(String, String)>,
    /// Images from private registries: covered through their own Dockerfiles.
    pub internal_images: usize,
}

/// Lockfiles worth reading, in the order to read them. A `go.sum` next to a `go.mod`
/// is dropped: it keeps checksums for every version ever seen in the module graph,
/// so auditing it reports versions the build does not use.
pub(crate) fn select_lockfiles(paths: &[String]) -> Vec<(String, LockKind)> {
    let mut found: Vec<(String, LockKind)> = paths
        .iter()
        .filter_map(|p| lockfile_kind(p).map(|k| (p.clone(), k)))
        .collect();
    let dir = |p: &str| p.rsplit_once('/').map_or("", |(d, _)| d).to_string();
    let mod_dirs: BTreeSet<String> = found
        .iter()
        .filter(|(_, k)| *k == LockKind::GoMod)
        .map(|(p, _)| dir(p))
        .collect();
    found.retain(|(p, k)| *k != LockKind::GoSum || !mod_dirs.contains(&dir(p)));
    found.sort_by_key(|(p, _)| (p.matches('/').count(), p.clone()));
    found
}

/// Runtime / build / dev per npm package name, from the lockfile graph and, where the
/// lockfile doesn't record its roots, the package.json beside it.
pub(crate) fn npm_roles(kind: LockKind, lock: &str, package_json: Option<&str>) -> Option<BTreeMap<String, Role>> {
    let graph = match kind {
        LockKind::PackageLock => npm_roles::graph_package_lock(lock)?,
        LockKind::YarnLock => npm_roles::graph_yarn_lock(lock),
        LockKind::PnpmLock => npm_roles::graph_pnpm_lock(lock),
        _ => return None,
    };
    let manifest_roots = package_json.and_then(npm_roles::roots_from_package_json);
    npm_roles::classify(&graph, manifest_roots.as_ref())
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
    let blobs: Vec<String> = tree
        .iter()
        .filter(|e| e["type"].as_str() == Some("blob"))
        .filter_map(|e| e["path"].as_str().map(str::to_string))
        .collect();
    let mut scan = RepoScan { path: path.to_string(), ..Default::default() };
    let mut infra_files: Vec<(String, infra::InfraKind)> =
        blobs.iter().filter_map(|b| infra::infra_kind(b).map(|k| (b.clone(), k))).collect();
    infra_files.sort_by_key(|(p, _)| (p.matches('/').count(), p.clone()));
    if infra_files.len() > MAX_INFRA_FILES_PER_REPO {
        scan.infra_capped = true;
        infra_files.truncate(MAX_INFRA_FILES_PER_REPO);
    }
    for (file, kind) in &infra_files {
        let Ok(text) = client
            .get_text(&format!("/projects/{enc}/repository/files/{}/raw", urlencoding::encode(file)), &[("ref", "HEAD")])
            .await
        else {
            continue;
        };
        scan.infra_files += 1;
        let images = match kind {
            infra::InfraKind::Dockerfile => infra::dockerfile_images(&text),
            infra::InfraKind::Yaml => infra::yaml_images(&text),
            infra::InfraKind::Terraform => {
                scan.infra_refs.extend(infra::terraform_versions(&text).into_iter().map(|r| (file.clone(), r)));
                Vec::new()
            }
        };
        for image in images {
            match infra::parse_image(&image, internal_host) {
                infra::ImageRef::Versioned(refs) => scan.infra_refs.extend(refs.into_iter().map(|r| (file.clone(), r))),
                infra::ImageRef::Unpinned(i) => scan.unpinned_images.push((file.clone(), i)),
                infra::ImageRef::Internal => scan.internal_images += 1,
                infra::ImageRef::Unknown => {}
            }
        }
    }
    let mut found = select_lockfiles(&blobs);
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
        // npm roles by reachability; yarn and package-lock v1 keep their roots in the
        // package.json beside them.
        let roles = if matches!(kind, LockKind::PackageLock | LockKind::YarnLock | LockKind::PnpmLock) {
            let needs_manifest = kind == LockKind::YarnLock || (kind == LockKind::PackageLock && !text.contains("\"packages\""));
            let manifest = if needs_manifest {
                let dir = file.rsplit_once('/').map_or(String::new(), |(d, _)| format!("{d}/"));
                client
                    .get_text(
                        &format!("/projects/{enc}/repository/files/{}/raw", urlencoding::encode(&format!("{dir}package.json"))),
                        &[("ref", "HEAD")],
                    )
                    .await
                    .ok()
            } else {
                None
            };
            npm_roles(kind, &text, manifest.as_deref())
        } else {
            None
        };
        if kind == LockKind::GradleCatalog {
            scan.declared_only.push(file.clone());
        }
        scan.internal_skipped += parsed.internal_skipped;
        scan.unversioned += parsed.unversioned;
        scan.lockfiles.push((file.clone(), parsed.deps.len()));
        for (d, dev) in parsed.deps.into_iter().zip(parsed.dev) {
            let role = roles.as_ref().and_then(|r| r.get(&d.name).copied());
            scan.dep_lockfile.push(file.clone());
            scan.dep_dev.push(role.map_or(dev, |r| r == Role::Dev));
            scan.dep_build.push(role == Some(Role::Build));
            scan.deps.push(d);
        }
    }
    Ok(scan)
}

/// One vulnerable package version, with every advisory against it and every place it
/// is pinned.
#[derive(Debug, Clone)]
pub(crate) struct PackageFinding {
    pub dep: Dep,
    pub advisories: Vec<Advisory>,
    /// "repo:lockfile" locations.
    pub locations: Vec<String>,
    /// Pinned somewhere as a runtime (not dev-only) dependency.
    pub runtime: bool,
    /// Repositories that pin it as a runtime dependency.
    pub runtime_repos: BTreeSet<String>,
    /// Repositories where only build tooling reaches it.
    pub build_repos: BTreeSet<String>,
}

impl PackageFinding {
    /// " · build" / " · dev" suffix for non-runtime packages.
    pub(crate) fn role_suffix(&self) -> &'static str {
        if self.runtime {
            ""
        } else if !self.build_repos.is_empty() {
            " · build"
        } else {
            " · dev"
        }
    }
}

impl PackageFinding {
    pub(crate) fn severity(&self) -> VulnSeverity {
        self.advisories.iter().map(|a| a.severity).min().unwrap_or(VulnSeverity::Unrated)
    }
}

/// Everything one audit run gathered, before rendering.
#[derive(Debug, Default)]
pub(crate) struct Audit {
    pub scope: String,
    pub total_repos: usize,
    pub scans: Vec<RepoScan>,
    pub unique_deps: usize,
    pub findings: Vec<PackageFinding>,
    /// End-of-life findings: base images, platform versions, the GitLab instance.
    pub infra: Vec<infra::InfraFinding>,
    /// Partial-failure notes (some repos or advisories could not be fetched).
    pub warnings: String,
}

/// Scan lockfiles and match them against OSV. Shared by the MCP tool and the
/// weekly report CLI, so both see exactly the same findings.
pub(crate) async fn collect_audit(
    client: &GitLabClient,
    project_id: &str,
    group_path: &str,
    max_projects: usize,
) -> Result<Audit> {
    let (targets, total_repos, scope) = if !group_path.is_empty() {
        // Several groups may be given, comma-separated: one instance, one report.
        let groups: Vec<&str> = group_path.split(',').map(str::trim).filter(|g| !g.is_empty()).collect();
        let mut projects: Vec<Value> = Vec::new();
        for g in &groups {
            let mut page: Vec<Value> = client
                .get_all_pages(
                    &format!("/groups/{}/projects", urlencoding::encode(g)),
                    &[
                        ("include_subgroups", "true"),
                        ("archived", "false"),
                        ("order_by", "last_activity_at"),
                        ("sort", "desc"),
                    ],
                    10,
                )
                .await?;
            projects.append(&mut page);
        }
        projects.sort_by(|a, b| b["last_activity_at"].as_str().cmp(&a["last_activity_at"].as_str()));
        let mut seen = BTreeSet::new();
        projects.retain(|p| seen.insert(p["id"].as_u64()));
        let total = projects.len();
        let targets: Vec<String> = projects
            .iter()
            .take(max_projects)
            .filter_map(|p| p["path_with_namespace"].as_str().map(str::to_string))
            .collect();
        let label = if groups.len() == 1 {
            format!("group `{}`", groups[0])
        } else {
            format!("groups {}", groups.iter().map(|g| format!("`{g}`")).collect::<Vec<_>>().join(", "))
        };
        (targets, total, label)
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

    let ids: Vec<&String> = ids_per_dep.iter().flatten().collect::<BTreeSet<_>>().into_iter().collect();
    let mut records: BTreeMap<String, Value> = BTreeMap::new();
    let mut lookup = FanOut::default();
    for chunk in ids.chunks(8) {
        let fetched = join_all(chunk.iter().map(|id| osv.vuln(id))).await;
        for (id, r) in chunk.iter().zip(fetched) {
            if let Some(v) = lookup.take(r) {
                records.insert((*id).clone(), v);
            }
        }
    }
    let lookup_warning = lookup.finish("advisory lookups")?.unwrap_or_default();

    let mut findings: Vec<PackageFinding> = Vec::new();
    for (dep, ids) in unique.iter().zip(&ids_per_dep) {
        let advisories: Vec<Advisory> = ids
            .iter()
            .filter_map(|id| records.get(id))
            .map(|v| advisory_for(v, dep))
            .collect();
        if advisories.is_empty() {
            continue;
        }
        let mut locations = BTreeSet::new();
        let mut runtime_repos = BTreeSet::new();
        let mut build_repos = BTreeSet::new();
        for s in &scans {
            for (((d, f), dev), build) in s.deps.iter().zip(&s.dep_lockfile).zip(&s.dep_dev).zip(&s.dep_build) {
                if d == dep {
                    locations.insert(format!("{}:{f}", s.path));
                    if *build {
                        build_repos.insert(s.path.clone());
                    } else if !dev {
                        runtime_repos.insert(s.path.clone());
                    }
                }
            }
        }
        findings.push(PackageFinding {
            dep: dep.clone(),
            advisories: merge_advisories(advisories),
            locations: locations.into_iter().collect(),
            runtime: !runtime_repos.is_empty(),
            runtime_repos,
            build_repos,
        });
    }
    let (infra, infra_warning) = collect_infra(client, &scans).await?;
    Ok(Audit {
        scope,
        total_repos,
        unique_deps: unique.len(),
        scans,
        findings,
        infra,
        warnings: format!("{warning}{lookup_warning}{infra_warning}"),
    })
}

/// End-of-life findings for what the scans declared, plus the platform itself: the
/// GitLab instance's version and, where the token may list them, its runners'.
/// endoflife.date being unreachable is a warning, not a failed audit: the package
/// findings stand on their own.
async fn collect_infra(client: &GitLabClient, scans: &[RepoScan]) -> Result<(Vec<infra::InfraFinding>, String)> {
    let today = chrono::Utc::now().date_naive();
    let eol = infra::EndOfLife::from_env()?;
    let mut products: BTreeSet<String> = scans.iter().flat_map(|s| s.infra_refs.iter().map(|(_, r)| r.product.clone())).collect();
    products.insert("gitlab".into());
    let products: Vec<String> = products.into_iter().collect();
    let mut cycles: BTreeMap<String, Vec<infra::Cycle>> = BTreeMap::new();
    let mut failed = 0;
    for chunk in products.chunks(8) {
        let got = join_all(chunk.iter().map(|p| eol.cycles(p))).await;
        for (p, r) in chunk.iter().zip(got) {
            match r {
                Ok(c) => {
                    cycles.insert(p.clone(), c);
                }
                Err(_) => failed += 1,
            }
        }
    }
    let mut warning = String::new();
    if failed > 0 {
        warning = format!(
            "*Warning: end-of-life data unavailable for {failed} of {} products (endoflife.date); infrastructure findings are incomplete.*\n\n",
            products.len()
        );
    }
    let mut out: Vec<infra::InfraFinding> = Vec::new();
    for s in scans {
        out.extend(infra::findings_for(&s.path, &s.infra_refs, &s.unpinned_images, &cycles, today));
    }

    // The platform: the GitLab instance this audit ran against.
    let host = client.host();
    if let (Ok(v), Some(gl)) = (client.get::<Value>("/version", &[]).await, cycles.get("gitlab")) {
        let version = v["version"].as_str().unwrap_or("").split('-').next().unwrap_or("").to_string();
        if let Some(verdict) = infra::verdict(gl, &version, today, true) {
            out.push(infra::InfraFinding {
                repo: format!("GitLab instance ({host})"),
                files: Vec::new(),
                product: "gitlab".into(),
                version: version.clone(),
                what: format!("GitLab {version}"),
                verdict,
            });
        }
        // Runners: listing every runner needs an admin token; without one this is skipped.
        if let Ok(runners) = client.get_all_pages::<Value>("/runners/all", &[("per_page", "100")], 3).await {
            let ids: Vec<u64> = runners.iter().filter_map(|r| r["id"].as_u64()).take(50).collect();
            let paths: Vec<String> = ids.iter().map(|id| format!("/runners/{id}")).collect();
            let details = join_all(paths.iter().map(|p| client.get::<Value>(p, &[]))).await;
            for d in details.into_iter().flatten() {
                let rv = d["version"].as_str().unwrap_or("").to_string();
                let name = d["description"].as_str().filter(|s| !s.is_empty()).unwrap_or("unnamed").to_string();
                if rv.is_empty() || d["online"].as_bool() == Some(false) {
                    continue;
                }
                if let Some(verdict) = infra::verdict(gl, &rv, today, false) {
                    out.push(infra::InfraFinding {
                        repo: format!("GitLab runner \"{name}\""),
                        files: Vec::new(),
                        product: "gitlab-runner".into(),
                        version: rv.clone(),
                        what: format!("runner {rv}"),
                        verdict,
                    });
                }
            }
        }
    }
    let rank = |s: &str| ["CRITICAL", "HIGH", "MEDIUM", "LOW"].iter().position(|x| *x == s).unwrap_or(9);
    out.sort_by(|a, b| (rank(a.verdict.severity), &a.repo, &a.product).cmp(&(rank(b.verdict.severity), &b.repo, &b.product)));
    Ok((out, warning))
}

/// Audit pinned dependencies across one project or a whole group.
pub async fn audit_dependencies(
    client: &GitLabClient,
    project_id: &str,
    group_path: &str,
    max_projects: usize,
    summary_only: bool,
    min_severity: Option<&str>,
) -> Result<String> {
    let min_severity = parse_min_severity(min_severity)?;
    let a = collect_audit(client, project_id, group_path, max_projects).await?;
    let report = if summary_only {
        render_dependency_summary(&a.scope, &a.scans, a.unique_deps, &a.findings)
    } else {
        render_dependency_audit(&a.scope, a.total_repos, &a.scans, a.unique_deps, a.findings, &a.infra, min_severity)
    };
    Ok(format!("{}{report}", a.warnings))
}

/// The `min_severity` parameter. Unknown values are a caller mistake, not "all".
pub(crate) fn parse_min_severity(raw: Option<&str>) -> Result<Option<VulnSeverity>> {
    match raw.map(|r| r.trim().to_ascii_lowercase()).as_deref() {
        None | Some("") | Some("all") => Ok(None),
        Some("critical") => Ok(Some(VulnSeverity::Critical)),
        Some("high") => Ok(Some(VulnSeverity::High)),
        Some("medium") | Some("moderate") => Ok(Some(VulnSeverity::Medium)),
        Some("low") => Ok(Some(VulnSeverity::Low)),
        Some(other) => Err(Error::user_input(format!(
            "min_severity '{other}' is not one of critical, high, medium, low."
        ))),
    }
}

/// Where to upgrade a package to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UpgradeTarget {
    /// The lowest version fixing every advisory that has a fix; `None` when none has.
    pub version: Option<String>,
    /// That version crosses a major version (for 0.x, a minor one).
    pub major: bool,
    /// Advisories with no fixed version above the current one.
    pub unfixed: usize,
}

impl UpgradeTarget {
    pub(crate) fn label(&self) -> String {
        let unfixed = match self.unfixed {
            0 => String::new(),
            1 => "1 advisory has no fix yet".to_string(),
            n => format!("{n} advisories have no fix yet"),
        };
        match (&self.version, self.unfixed) {
            (None, _) => "no fix yet".to_string(),
            (Some(v), 0) if self.major => format!("→ {v} (major upgrade)"),
            (Some(v), 0) => format!("→ {v}"),
            (Some(v), _) if self.major => format!("→ {v} (major upgrade; {unfixed})"),
            (Some(v), _) => format!("→ {v} ({unfixed})"),
        }
    }
}

/// The upgrade for a package: for each advisory the lowest fixed version above the
/// current one (release lines below it don't help), then the highest of those.
/// Advisories without such a fix are counted rather than blocking the rest.
pub(crate) fn upgrade_target(current: &str, advisories: &[Advisory]) -> UpgradeTarget {
    let mut target: Option<String> = None;
    let mut unfixed = 0;
    for a in advisories {
        let fix = a
            .fixed
            .iter()
            .filter(|f| compare_versions(f, current) == std::cmp::Ordering::Greater)
            .min_by(|x, y| compare_versions(x, y));
        match fix {
            None => unfixed += 1,
            Some(fix) => {
                if target.as_ref().is_none_or(|t| compare_versions(fix, t) == std::cmp::Ordering::Greater) {
                    target = Some(fix.clone());
                }
            }
        }
    }
    let major_of = |v: &str| {
        // 0.x: the minor is the breaking component.
        match version_numbers(v).as_slice() {
            [0, minor, ..] => (0, *minor),
            [m, ..] => (*m, 0),
            [] => (0, 0),
        }
    };
    let major = target.as_deref().is_some_and(|t| major_of(t) != major_of(current));
    UpgradeTarget { version: target, major, unfixed }
}

/// Leading numeric components of a version ("v1.2.3-rc1" → [1, 2, 3]).
fn version_numbers(v: &str) -> Vec<u64> {
    v.trim_start_matches(['v', 'V'])
        .split(['.', '-', '+'])
        .map_while(|p| p.parse::<u64>().ok())
        .collect()
}

/// Ecosystem-agnostic version ordering: numeric components first, then the rest as
/// text. Right for the semver-like versions every supported ecosystem uses.
pub(crate) fn compare_versions(a: &str, b: &str) -> std::cmp::Ordering {
    let (na, nb) = (version_numbers(a), version_numbers(b));
    for i in 0..na.len().max(nb.len()) {
        let (x, y) = (na.get(i).copied().unwrap_or(0), nb.get(i).copied().unwrap_or(0));
        if x != y {
            return x.cmp(&y);
        }
    }
    // Same numbers: a release sorts after its pre-releases ("1.0.0" > "1.0.0-rc1").
    let pre = |v: &str| v.contains('-');
    match (pre(a), pre(b)) {
        (false, true) => std::cmp::Ordering::Greater,
        (true, false) => std::cmp::Ordering::Less,
        (true, true) => a.cmp(b),
        (false, false) => std::cmp::Ordering::Equal,
    }
}

/// Merge advisories that describe the same issue — the GitHub record and the Go or
/// RustSec record of one CVE share an alias — keeping the highest rating and the
/// union of fixed versions.
pub(crate) fn merge_advisories(advisories: Vec<Advisory>) -> Vec<Advisory> {
    let mut merged: Vec<Advisory> = Vec::new();
    for a in advisories {
        let keys: BTreeSet<&str> = a.aliases.iter().map(String::as_str).chain([a.id.as_str()]).collect();
        let existing = merged.iter_mut().find(|m| {
            m.aliases.iter().map(String::as_str).chain([m.id.as_str()]).any(|k| keys.contains(k))
        });
        match existing {
            Some(m) => {
                // Prefer the rated, GitHub-style record as the face of the merged entry.
                if a.severity < m.severity || (m.severity == VulnSeverity::Unrated && a.id.starts_with("GHSA-")) {
                    let previous = std::mem::replace(&mut m.id, a.id.clone());
                    m.aliases.push(previous);
                    let face = m.id.clone();
                    m.aliases.retain(|x| *x != face);
                    m.summary = a.summary.clone();
                    m.severity = m.severity.min(a.severity);
                }
                for alias in a.aliases.iter().chain([&a.id]) {
                    if !m.aliases.contains(alias) && *alias != m.id {
                        m.aliases.push(alias.clone());
                    }
                }
                if m.cve.is_none() {
                    m.cve = a.cve.clone();
                }
                for f in a.fixed {
                    if !m.fixed.contains(&f) {
                        m.fixed.push(f);
                    }
                }
            }
            None => merged.push(a),
        }
    }
    merged.sort_by(|a, b| (a.severity, &a.id).cmp(&(b.severity, &b.id)));
    merged
}

fn order_findings(findings: &mut [PackageFinding]) {
    findings.sort_by(|a, b| {
        (a.severity(), !a.runtime, std::cmp::Reverse(a.advisories.len()), &a.dep)
            .cmp(&(b.severity(), !b.runtime, std::cmp::Reverse(b.advisories.len()), &b.dep))
    });
}

/// One line per scope, for sweeps: counts and the packages to fix first.
pub(crate) fn render_dependency_summary(
    scope: &str,
    scans: &[RepoScan],
    unique_deps: usize,
    findings: &[PackageFinding],
) -> String {
    let mut f = findings.to_vec();
    order_findings(&mut f);
    let runtime_crit_high = f
        .iter()
        .filter(|x| x.runtime && x.severity() <= VulnSeverity::High)
        .count();
    let top: Vec<String> = f
        .iter()
        .take(5)
        .map(|x| format!("{}@{} ({}, {} adv.)", x.dep.name, x.dep.version, x.severity().label(), x.advisories.len()))
        .collect();
    format!(
        "{scope}: {} repos, {unique_deps} packages, {} vulnerable ({runtime_crit_high} runtime CRITICAL/HIGH). Top: {}",
        scans.len(),
        f.len(),
        if top.is_empty() { "–".to_string() } else { top.join("; ") }
    )
}

/// The audit report. Pure: ordering, roll-up, upgrade targets, coverage notes and the
/// "nothing to audit" cases are verified on plain values.
pub(crate) fn render_dependency_audit(
    scope: &str,
    total_repos: usize,
    scans: &[RepoScan],
    unique_deps: usize,
    mut findings: Vec<PackageFinding>,
    infra: &[infra::InfraFinding],
    min_severity: Option<VulnSeverity>,
) -> String {
    order_findings(&mut findings);
    let lockfiles: usize = scans.iter().map(|s| s.lockfiles.len()).sum();
    let advisories: usize = findings.iter().map(|f| f.advisories.len()).sum();
    let mut by_sev: BTreeMap<VulnSeverity, usize> = BTreeMap::new();
    for f in &findings {
        *by_sev.entry(f.severity()).or_default() += 1;
    }
    let runtime = findings.iter().filter(|f| f.runtime).count();
    let build = findings.iter().filter(|f| f.role_suffix() == " · build").count();
    let mut out = vec![
        format!("# Dependency vulnerability audit — {scope}"),
        String::new(),
        format!(
            "**{} repo(s) scanned** of {total_repos} · **{lockfiles} lockfile(s)** · **{unique_deps} distinct packages** · **{} vulnerable** ({runtime} runtime, {build} build tooling, {} dev-only) · {advisories} distinct advisories",
            scans.len(),
            findings.len(),
            findings.len() - runtime - build
        ),
    ];
    if !findings.is_empty() {
        out.push(format!(
            "Vulnerable packages by worst advisory: {}",
            by_sev.iter().map(|(s, n)| format!("{} {n}", s.label())).collect::<Vec<_>>().join(" · ")
        ));
    }
    out.push(String::new());
    let shown: Vec<&PackageFinding> = findings
        .iter()
        .filter(|f| min_severity.is_none_or(|min| f.severity() <= min))
        .collect();
    if findings.is_empty() {
        out.push(if unique_deps == 0 {
            "No auditable dependencies found — see coverage below; this is not a clean bill of health.".to_string()
        } else {
            "No known vulnerabilities in the pinned versions.".to_string()
        });
    } else {
        if let Some(min) = min_severity {
            out.push(format!(
                "_Showing {} of {} vulnerable packages ({} and above)._\n",
                shown.len(),
                findings.len(),
                min.label()
            ));
        }
        out.push("| Severity | Package | Advisories | Fix | Where |".to_string());
        out.push("|----------|---------|------------|-----|-------|".to_string());
        const MAX_ROWS: usize = 120;
        for f in shown.iter().take(MAX_ROWS) {
            let ids: Vec<String> = f
                .advisories
                .iter()
                .map(|a| a.cve.clone().unwrap_or_else(|| a.id.clone()))
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let ids_text = if ids.len() > 4 {
                format!("{} +{} more", ids[..4].join(", "), ids.len() - 4)
            } else {
                ids.join(", ")
            };
            let headline = &f.advisories[0];
            let fix = upgrade_target(&f.dep.version, &f.advisories).label();
            let where_ = match f.locations.len() {
                0 => "–".to_string(),
                1 => f.locations[0].clone(),
                n => format!("{} +{} more", f.locations[0], n - 1),
            };
            out.push(format!(
                "| {} | {}@{} ({}){} | {} — {}: {} | {fix} | {where_} |",
                f.severity().label(),
                f.dep.name,
                f.dep.version,
                f.dep.ecosystem.osv_name(),
                f.role_suffix(),
                f.advisories.len(),
                ids_text,
                headline.summary.replace('|', "/"),
            ));
        }
        if shown.len() > MAX_ROWS {
            out.push(format!("\n_{} more packages not shown — narrow to a project or raise min_severity._", shown.len() - MAX_ROWS));
        }
    }

    if !infra.is_empty() {
        out.push(String::new());
        out.push("## Infrastructure: end of life".to_string());
        out.push(String::new());
        out.push("| Severity | Where | What | Status | Fix |".to_string());
        out.push("|----------|-------|------|--------|-----|".to_string());
        for f in infra {
            let where_ = match f.files.as_slice() {
                [] => f.repo.clone(),
                [one] => format!("{}:{one}", f.repo),
                [one, rest @ ..] => format!("{}:{one} +{} more", f.repo, rest.len()),
            };
            out.push(format!(
                "| {} | {} | {} ({} {}) | {} | {} |",
                f.verdict.severity,
                where_.replace('|', "/"),
                f.what.replace('|', "/"),
                f.product,
                f.version,
                f.verdict.note.replace('|', "/"),
                f.verdict.fix
            ));
        }
    }

    // Coverage: what could not be audited is part of the answer.
    let no_lockfile: Vec<&str> = scans
        .iter()
        .filter(|s| s.lockfiles.is_empty() && s.unsupported.is_empty() && s.infra_files == 0)
        .map(|s| s.path.as_str())
        .collect();
    let unsupported: Vec<String> = scans
        .iter()
        .flat_map(|s| s.unsupported.iter().map(move |f| format!("{}:{f}", s.path)))
        .collect();
    let declared: Vec<String> = scans
        .iter()
        .flat_map(|s| s.declared_only.iter().map(move |f| format!("{}:{f}", s.path)))
        .collect();
    let capped: Vec<&str> = scans.iter().filter(|s| s.capped).map(|s| s.path.as_str()).collect();
    let internal: usize = scans.iter().map(|s| s.internal_skipped).sum();
    let unversioned: usize = scans.iter().map(|s| s.unversioned).sum();
    let infra_files: usize = scans.iter().map(|s| s.infra_files).sum();
    let internal_images: usize = scans.iter().map(|s| s.internal_images).sum();
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
    if !declared.is_empty() {
        notes.push(format!(
            "- **Direct dependencies only** ({}): {} — a version catalog declares direct dependencies; transitive ones need a `gradle.lockfile`.",
            declared.len(),
            declared.join(", ")
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
    if unversioned > 0 {
        notes.push(format!(
            "- {unversioned} dependencies are pinned to a branch, commit or range rather than a release, so they could not be matched."
        ));
    }
    if infra_files > 0 {
        notes.push(format!(
            "- {infra_files} infrastructure files read (Dockerfiles, compose/CI/Kubernetes YAML, Terraform) for image and platform versions; {internal_images} images from private registries were not checked — their bases are covered through their own Dockerfiles."
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
    out.push(
        "_Matched against OSV.dev (GitHub, RustSec, Go, PyPA and other advisory feeds). Only exact pinned versions are checked; advisories for the same issue are merged; severity is the advisory's own rating, else its CVSS v3 base score. Go is read from `go.mod` (the selected build list), not `go.sum`. Images, Terraform-declared platforms and the GitLab instance are checked against endoflife.date support dates. npm roles come from reachability: a package reached only through build tools (webpack, babel, vite…) is build tooling even when declared under `dependencies`._"
            .to_string(),
    );
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
                {"name": "laravel/framework", "version": "12.x-dev"},
                {"name": "league/flysystem", "version": "1.x-dev"},
            ],
            "packages-dev": [{"name": "phpunit/phpunit", "version": "v9.5.0"}]
        });
        let p = parse_lockfile(LockKind::ComposerLock, &lock.to_string(), HOST);
        assert_eq!(names(&p), vec!["guzzlehttp/guzzle@7.4.0", "phpunit/phpunit@9.5.0"]);
        assert_eq!(p.internal_skipped, 1);
        assert_eq!(p.unversioned, 3, "dev-main and both *.x-dev branch aliases");
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
            dep_dev: deps.iter().map(|_| false).collect(),
            dep_build: deps.iter().map(|_| false).collect(),
            ..Default::default()
        }
    }

    fn adv(id: &str, aliases: &[&str], severity: VulnSeverity, fixed: &[&str]) -> Advisory {
        Advisory {
            id: id.into(),
            cve: aliases.iter().find(|a| a.starts_with("CVE-")).map(|a| a.to_string()),
            summary: format!("issue {id}"),
            severity,
            fixed: fixed.iter().map(|f| f.to_string()).collect(),
            aliases: aliases.iter().map(|a| a.to_string()).collect(),
        }
    }

    fn package(name: &str, version: &str, advisories: Vec<Advisory>, locations: &[&str], runtime: bool) -> PackageFinding {
        PackageFinding {
            dep: Dep { ecosystem: Ecosystem::Npm, name: name.into(), version: version.into() },
            advisories: merge_advisories(advisories),
            locations: locations.iter().map(|l| l.to_string()).collect(),
            runtime,
            build_repos: BTreeSet::new(),
            runtime_repos: if runtime {
                locations.iter().map(|l| l.split_once(':').map_or(*l, |(r, _)| r).to_string()).collect()
            } else {
                BTreeSet::new()
            },
        }
    }

    fn sample_findings() -> Vec<PackageFinding> {
        vec![
            package(
                "lodash",
                "4.17.15",
                vec![
                    adv("GHSA-p6mc-m468-83gw", &["CVE-2020-8203"], VulnSeverity::High, &["4.17.19"]),
                    adv("GHSA-35jh-r3h4-6jhm", &["CVE-2021-23337"], VulnSeverity::High, &["4.17.21"]),
                ],
                &["acme/web:package-lock.json", "acme/admin:yarn.lock"],
                true,
            ),
            package(
                "minimist",
                "0.0.8",
                vec![adv("GHSA-xvch-5gv4-984h", &["CVE-2021-44906"], VulnSeverity::Critical, &["0.2.4", "1.2.6"])],
                &["acme/web:package-lock.json"],
                false,
            ),
            package(
                "semver",
                "5.7.1",
                vec![adv("GHSA-c2qf-rxjj-qqgw", &["CVE-2022-25883"], VulnSeverity::Medium, &["5.7.2", "6.3.1", "7.5.2"])],
                &["acme/web:package-lock.json"],
                true,
            ),
        ]
    }

    #[test]
    fn golden_dependency_audit_report() {
        let mut no_lock = scan("acme/docs", &[], &[]);
        no_lock.internal_skipped = 3;
        let mut ios = scan("acme/ios", &[], &[]);
        ios.unsupported = vec!["Podfile.lock".into()];
        let mut android = scan("acme/android", &[("gradle/libs.versions.toml", 40)], &[]);
        android.declared_only = vec!["gradle/libs.versions.toml".into()];
        android.unversioned = 2;
        let scans = vec![
            scan("acme/web", &[("package-lock.json", 120)], &[("lodash", "4.17.15"), ("minimist", "0.0.8")]),
            scan("acme/admin", &[("yarn.lock", 80)], &[("lodash", "4.17.15")]),
            no_lock,
            ios,
            android,
        ];
        let v = |sev: &'static str, note: &str, fix: &str| infra::Verdict { severity: sev, note: note.into(), fix: fix.into() };
        let infra = vec![
            infra::InfraFinding {
                repo: "acme/web".into(),
                files: vec!["Dockerfile".into(), "docker-compose.yml".into()],
                product: "nodejs".into(),
                version: "16".into(),
                what: "node:16-alpine".into(),
                verdict: v("HIGH", "16 reached end of life on 2023-09-11: no security fixes since", "→ 22 (supported until 2027-04-30)"),
            },
            infra::InfraFinding {
                repo: "GitLab instance (git.example.com)".into(),
                files: vec![],
                product: "gitlab".into(),
                version: "17.5.2".into(),
                what: "GitLab 17.5.2".into(),
                verdict: v("HIGH", "behind the latest 17.5 patch release (17.5.5); patch releases carry security fixes", "→ 17.5.5"),
            },
            infra::InfraFinding {
                repo: "acme/web".into(),
                files: vec![".gitlab-ci.yml".into()],
                product: "nginx".into(),
                version: "latest".into(),
                what: "nginx:latest".into(),
                verdict: v("LOW", "unpinned: each build pulls whatever is newest, so what runs is unknown", "pin a supported version"),
            },
        ];
        let out = render_dependency_audit("group `acme`", 7, &scans, 180, sample_findings(), &infra, None);
        assert!(out.contains("## Infrastructure: end of life"));
        assert!(out.contains("| HIGH | acme/web:Dockerfile +1 more | node:16-alpine (nodejs 16) |"));
        assert!(out.contains("| HIGH | GitLab instance (git.example.com) | GitLab 17.5.2 (gitlab 17.5.2) |"));
        crate::golden::assert_golden("render/dependency_audit.md", &out);
        assert!(out.contains("**3 vulnerable** (2 runtime, 0 build tooling, 1 dev-only)"));
        assert!(out.contains("CRITICAL 1 · HIGH 1 · MEDIUM 1"));
        assert!(out.find("minimist").unwrap() < out.find("lodash@").unwrap(), "most severe first");
        assert!(out.contains("minimist@0.0.8 (npm) · dev"));
        assert!(out.contains("| 2 — CVE-2020-8203, CVE-2021-23337"), "one row, both advisories");
        assert!(out.contains("→ 4.17.21 |"), "the version fixing every advisory");
        assert!(out.contains("→ 0.2.4 (major upgrade) |"), "the nearest fixed line, not the newest; 0.x minor is breaking");
        assert!(out.contains("acme/web:package-lock.json +1 more"));
        assert!(out.contains("No supported lockfile** (1): acme/docs"));
        assert!(out.contains("Not auditable against OSV** (1): acme/ios:Podfile.lock"));
        assert!(out.contains("Direct dependencies only** (1): acme/android:gradle/libs.versions.toml"));
        assert!(out.contains("3 internal or non-registry dependencies"));
        assert!(out.contains("2 dependencies are pinned to a branch, commit or range"));
        assert!(out.contains("Only the 5 most recently active of 7 repos"));
    }

    #[test]
    fn min_severity_filters_rows_but_not_totals() {
        let scans = vec![scan("acme/web", &[("package-lock.json", 3)], &[])];
        let out = render_dependency_audit("project `acme/web`", 1, &scans, 3, sample_findings(), &[], Some(VulnSeverity::High));
        assert!(out.contains("**3 vulnerable**"), "totals count everything");
        assert!(out.contains("_Showing 2 of 3 vulnerable packages (HIGH and above)._"));
        assert!(!out.contains("semver@"));
        assert_eq!(parse_min_severity(Some(" High ")).unwrap(), Some(VulnSeverity::High));
        assert_eq!(parse_min_severity(Some("moderate")).unwrap(), Some(VulnSeverity::Medium));
        assert_eq!(parse_min_severity(None).unwrap(), None);
        assert_eq!(parse_min_severity(Some("all")).unwrap(), None);
        assert!(parse_min_severity(Some("severe")).is_err(), "a typo is not 'all'");
    }

    #[test]
    fn summary_puts_runtime_critical_and_high_first() {
        let scans = vec![scan("acme/web", &[("package-lock.json", 3)], &[])];
        let out = render_dependency_summary("group `acme`", &scans, 180, &sample_findings());
        assert_eq!(
            out,
            "group `acme`: 1 repos, 180 packages, 3 vulnerable (1 runtime CRITICAL/HIGH). Top: minimist@0.0.8 (CRITICAL, 1 adv.); lodash@4.17.15 (HIGH, 2 adv.); semver@5.7.1 (MEDIUM, 1 adv.)"
        );
        assert!(render_dependency_summary("p", &scans, 0, &[]).ends_with("Top: –"));
    }

    #[test]
    fn nothing_auditable_is_never_reported_as_clean() {
        let out = render_dependency_audit("project `p`", 1, &[scan("p", &[], &[])], 0, vec![], &[], None);
        assert!(out.contains("not a clean bill of health"));
        assert!(!out.contains("No known vulnerabilities"));
        let clean = render_dependency_audit("project `p`", 1, &[scan("p", &[("Cargo.lock", 3)], &[("x", "1")])], 3, vec![], &[], None);
        assert!(clean.contains("No known vulnerabilities in the pinned versions."));
    }

    #[test]
    fn duplicate_advisories_of_one_issue_merge() {
        let merged = merge_advisories(vec![
            adv("GO-2023-1571", &["CVE-2022-41723", "GHSA-vvpx-j8f3-3w6h"], VulnSeverity::Unrated, &["0.7.0"]),
            adv("GHSA-vvpx-j8f3-3w6h", &["CVE-2022-41723"], VulnSeverity::High, &["0.7.0", "0.8.0"]),
            adv("GHSA-other", &["CVE-2099-0001"], VulnSeverity::Low, &[]),
        ]);
        assert_eq!(merged.len(), 2, "linked by a shared alias");
        let m = &merged[0];
        assert_eq!(m.id, "GHSA-vvpx-j8f3-3w6h", "the rated record is the face");
        assert_eq!(m.severity, VulnSeverity::High, "highest rating wins");
        assert_eq!(m.cve.as_deref(), Some("CVE-2022-41723"));
        assert!(m.aliases.contains(&"GO-2023-1571".to_string()));
        assert!(!m.aliases.contains(&m.id), "the face is not its own alias");
        assert_eq!(m.fixed, vec!["0.7.0".to_string(), "0.8.0".to_string()], "union, no duplicates");
        assert_eq!(merged[1].id, "GHSA-other");
    }

    #[test]
    fn upgrade_target_is_the_lowest_version_fixing_everything() {
        let a = |fixed: &[&str]| adv("A", &[], VulnSeverity::High, fixed);
        let t = |cur: &str, advs: &[Advisory]| {
            let u = upgrade_target(cur, advs);
            (u.version.unwrap_or_default(), u.major, u.unfixed)
        };
        assert_eq!(t("1.2.0", &[a(&["1.2.5", "2.0.1"]), a(&["1.3.0"])]), ("1.3.0".into(), false, 0));
        assert_eq!(t("1.2.0", &[a(&["2.0.0"])]), ("2.0.0".into(), true, 0));
        assert_eq!(t("0.3.1", &[a(&["0.4.0"])]), ("0.4.0".into(), true, 0), "0.x minor is breaking");
        assert_eq!(t("0.3.1", &[a(&["0.3.9"])]), ("0.3.9".into(), false, 0));
        assert_eq!(t("1.2.0", &[a(&["1.3.0"]), a(&[])]), ("1.3.0".into(), false, 1), "an unfixed advisory doesn't hide the rest");
        assert_eq!(t("2.0.0", &[a(&["1.9.9"])]), (String::new(), false, 1), "only older lines fixed");
        assert_eq!(t("v1.2.0", &[a(&["v1.10.0"])]), ("v1.10.0".into(), false, 0));
        assert_eq!(upgrade_target("1.2.0", &[a(&["1.3.0"]), a(&[])]).label(), "→ 1.3.0 (1 advisory has no fix yet)");
        assert_eq!(upgrade_target("1.2.0", &[a(&["2.0.0"]), a(&[]), a(&[])]).label(), "→ 2.0.0 (major upgrade; 2 advisories have no fix yet)");
        assert_eq!(upgrade_target("1.2.0", &[a(&[])]).label(), "no fix yet");
    }

    #[test]
    fn versions_compare_numerically_with_prereleases_first() {
        use std::cmp::Ordering::*;
        assert_eq!(compare_versions("1.10.0", "1.9.9"), Greater);
        assert_eq!(compare_versions("1.0", "1.0.0"), Equal);
        assert_eq!(compare_versions("1.0.0-rc1", "1.0.0"), Less);
        assert_eq!(compare_versions("v2", "1.99"), Greater);
    }

    #[test]
    fn go_mod_next_to_go_sum_wins() {
        let paths: Vec<String> = ["go.sum", "go.mod", "tools/go.sum", "web/package-lock.json", "vendor/x/go.mod"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let picked: Vec<String> = select_lockfiles(&paths).into_iter().map(|(p, _)| p).collect();
        assert_eq!(picked, vec!["go.mod", "tools/go.sum", "web/package-lock.json"]);
    }

    #[test]
    fn go_mod_reads_requires_and_honours_replace() {
        let text = "module git.example.com/acme/svc\n\ngo 1.22\n\nrequire (\n\tgolang.org/x/net v0.7.0 // indirect\n\tgithub.com/pkg/old v1.0.0\n\tgit.example.com/acme/lib v0.3.0\n\tgithub.com/local/fork v1.0.0\n)\n\nrequire github.com/x/y v2.1.0+incompatible\n\nreplace github.com/pkg/old => github.com/pkg/new v1.2.0\nreplace github.com/local/fork => ../fork\n";
        let p = parse_lockfile(LockKind::GoMod, text, "git.example.com");
        let got: Vec<(String, String)> = p.deps.iter().map(|d| (d.name.clone(), d.version.clone())).collect();
        assert_eq!(
            got,
            vec![
                ("golang.org/x/net".into(), "0.7.0".into()),
                ("github.com/pkg/new".into(), "1.2.0".into()),
                ("github.com/x/y".into(), "2.1.0".into()),
            ]
        );
        assert_eq!(p.internal_skipped, 2, "internal module and local replacement");
    }

    #[test]
    fn swift_package_resolved_v1_and_v2() {
        let v2 = json!({"pins": [
            {"identity": "alamofire", "location": "https://github.com/Alamofire/Alamofire.git", "state": {"version": "5.6.1", "revision": "abc"}},
            {"identity": "lib", "location": "https://git.example.com/acme/lib.git", "state": {"version": "1.0.0"}},
            {"identity": "edge", "location": "https://github.com/acme/edge", "state": {"branch": "main", "revision": "def"}},
            {"identity": "local", "location": "../local", "state": {"version": "1.0.0"}},
        ], "version": 2})
        .to_string();
        let p = parse_lockfile(LockKind::SwiftResolved, &v2, "git.example.com");
        assert_eq!(p.deps.len(), 1);
        assert_eq!((p.deps[0].name.as_str(), p.deps[0].version.as_str()), ("github.com/Alamofire/Alamofire", "5.6.1"));
        assert_eq!(p.deps[0].ecosystem.osv_name(), "SwiftURL");
        assert_eq!((p.internal_skipped, p.unversioned), (2, 1));
        let v1 = json!({"object": {"pins": [{"package": "Kingfisher", "repositoryURL": "git@github.com:onevcat/Kingfisher.git", "state": {"version": "7.0.0"}}]}, "version": 1}).to_string();
        let p = parse_lockfile(LockKind::SwiftResolved, &v1, "git.example.com");
        assert_eq!(p.deps[0].name, "github.com/onevcat/Kingfisher");
    }

    #[test]
    fn gradle_catalog_resolves_refs_and_skips_ranges() {
        let text = "[versions]\nokhttp = \"4.9.0\"\nkotlin = { strictly = \"1.9.0\" }\ndyn = \"1.+\"\n\n[libraries]\nokhttp = { module = \"com.squareup.okhttp3:okhttp\", version.ref = \"okhttp\" }\nstdlib = { group = \"org.jetbrains.kotlin\", name = \"kotlin-stdlib\", version.ref = \"kotlin\" }\ngson = \"com.google.code.gson:gson:2.8.5\"\nwild = { module = \"a.b:c\", version.ref = \"dyn\" }\nbom = { module = \"a.b:managed\" }\n";
        let p = parse_lockfile(LockKind::GradleCatalog, text, "");
        let mut got: Vec<String> = p.deps.iter().map(|d| format!("{}@{}", d.name, d.version)).collect();
        got.sort();
        assert_eq!(got, vec!["com.google.code.gson:gson@2.8.5", "com.squareup.okhttp3:okhttp@4.9.0", "org.jetbrains.kotlin:kotlin-stdlib@1.9.0"]);
        assert_eq!(p.unversioned, 2, "a dynamic range and a BOM-managed library");
    }

    #[test]
    fn dev_dependencies_are_flagged() {
        let lock = json!({"lockfileVersion": 3, "packages": {
            "": {"name": "app"},
            "node_modules/express": {"version": "4.18.0"},
            "node_modules/jest": {"version": "29.0.0", "dev": true},
        }})
        .to_string();
        let p = parse_lockfile(LockKind::PackageLock, &lock, "");
        let flags: Vec<(String, bool)> = p.deps.iter().zip(&p.dev).map(|(d, f)| (d.name.clone(), *f)).collect();
        assert!(flags.contains(&("express".into(), false)));
        assert!(flags.contains(&("jest".into(), true)));
        let composer = json!({"packages": [{"name": "laravel/framework", "version": "v5.6.40"}], "packages-dev": [{"name": "phpunit/phpunit", "version": "7.0.0"}]}).to_string();
        let p = parse_lockfile(LockKind::ComposerLock, &composer, "");
        assert_eq!(p.dev, vec![false, true]);
    }

    #[test]
    fn npm_roles_need_roots_from_the_lockfile_or_its_package_json() {
        // package-lock v1 records no roots: the package.json beside it supplies them.
        let v1 = json!({"lockfileVersion": 1, "dependencies": {
            "webpack": {"version": "4.0.0", "requires": {"loader-utils": "^1"}},
            "loader-utils": {"version": "1.4.0"},
            "swiper": {"version": "6.5.0"},
        }})
        .to_string();
        assert!(npm_roles(LockKind::PackageLock, &v1, None).is_none(), "no roots: lockfile flags stand");
        let pj = json!({"dependencies": {"webpack": "^4", "swiper": "^6"}}).to_string();
        let roles = npm_roles(LockKind::PackageLock, &v1, Some(&pj)).unwrap();
        assert_eq!(roles["swiper"], Role::Runtime, "bundled UI library ships to the browser");
        assert_eq!(roles["loader-utils"], Role::Build, "only webpack reaches it");
        assert!(npm_roles(LockKind::CargoLock, "", None).is_none());
    }

    #[test]
    fn terraform_lock_providers_become_go_modules() {
        let lock = "# This file is maintained automatically by \"terraform init\".\nprovider \"registry.terraform.io/hashicorp/aws\" {\n  version     = \"5.31.0\"\n  constraints = \"~> 5.0\"\n  hashes = [\n    \"h1:abc=\",\n  ]\n}\n\nprovider \"registry.terraform.io/cloudflare/cloudflare\" {\n  version = \"4.20.0\"\n}\n\nprovider \"git.example.com/acme/internal\" {\n  version = \"1.0.0\"\n}\n";
        assert_eq!(lockfile_kind("infra/prod/.terraform.lock.hcl"), Some(LockKind::TerraformLock));
        let p = parse_lockfile(LockKind::TerraformLock, lock, HOST);
        assert_eq!(names(&p), vec!["github.com/hashicorp/terraform-provider-aws@5.31.0", "github.com/cloudflare/terraform-provider-cloudflare@4.20.0"]);
        assert!(p.deps.iter().all(|d| d.ecosystem == Ecosystem::Go));
        assert_eq!(p.internal_skipped, 1, "a provider from the internal registry stays in");
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
