//! Infrastructure versions: container base images, platform versions declared in
//! Terraform, and the GitLab instance itself — checked against endoflife.date.
//!
//! Infrastructure repositories rarely have lockfiles, so the package audit sees
//! nothing in them. Their risk is a different shape: a runtime, OS, database,
//! Kubernetes or GitLab release that no longer gets security fixes. That is decidable
//! from versions alone — no CVE matching — using endoflife.date's per-cycle support
//! dates, which cover the products infrastructure is made of.
//!
//! Parsers and the verdict are pure; fetching lives in `dependencies::collect_audit`.
//! Only public product names go to endoflife.date — never repository content.

use std::collections::BTreeMap;

use chrono::NaiveDate;
use serde_json::Value;

/// A version declared somewhere in infrastructure code.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InfraRef {
    /// endoflife.date product, e.g. "nodejs", "postgresql", "gitlab".
    pub product: String,
    /// Version as declared ("16.20.2", "13", "bookworm").
    pub version: String,
    /// What the line said, for the report ("node:16-alpine", "engine_version").
    pub what: String,
}

/// What an image reference resolved to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ImageRef {
    /// A public image with a version: one ref per product it carries (the image and
    /// an embedded distro such as `-alpine3.18`).
    Versioned(Vec<InfraRef>),
    /// A public image of a product we track, with no usable version (`latest`, none).
    Unpinned(String),
    /// From a private registry: its base shows up through its own Dockerfile.
    Internal,
    /// A template or variable we cannot resolve (`${TAG}`), or a product we don't track.
    Unknown,
}

const PUBLIC_REGISTRIES: &[&str] = &[
    "docker.io", "index.docker.io", "registry-1.docker.io", "quay.io", "ghcr.io", "mcr.microsoft.com",
    "gcr.io", "registry.k8s.io", "k8s.gcr.io", "public.ecr.aws",
];

/// Image name (last path segment, or a well-known full path) → endoflife.date product.
fn product_for_image(path: &str) -> Option<&'static str> {
    let last = path.rsplit('/').next().unwrap_or(path);
    if path.starts_with("dotnet/") {
        return Some("dotnet");
    }
    Some(match last {
        "node" => "nodejs",
        "python" => "python",
        "php" => "php",
        "ruby" => "ruby",
        "golang" => "go",
        "rust" => "rust",
        "eclipse-temurin" | "openjdk" => "eclipse-temurin",
        "alpine" => "alpine-linux",
        "debian" => "debian",
        "ubuntu" => "ubuntu",
        "centos" => "centos",
        "rockylinux" => "rocky-linux",
        "almalinux" => "almalinux",
        "amazonlinux" => "amazon-linux",
        "nginx" => "nginx",
        "haproxy" => "haproxy",
        "traefik" => "traefik",
        "postgres" | "postgresql" => "postgresql",
        "mysql" => "mysql",
        "mariadb" => "mariadb",
        "mongo" | "mongodb" => "mongodb",
        "redis" => "redis",
        "valkey" => "valkey",
        "memcached" => "memcached",
        "rabbitmq" => "rabbitmq",
        "elasticsearch" => "elasticsearch",
        "opensearch" => "opensearch",
        "clickhouse-server" | "clickhouse" => "clickhouse",
        "kafka" => "apache-kafka",
        "zookeeper" => "zookeeper",
        "grafana" => "grafana",
        "prometheus" => "prometheus",
        "keycloak" => "keycloak",
        "consul" => "consul",
        "argocd" => "argo-cd",
        "jenkins" => "jenkins",
        "gitlab-ce" | "gitlab-ee" => "gitlab",
        "wordpress" => "wordpress",
        _ => return None,
    })
}

/// Debian and Ubuntu codenames → release, so `bookworm` and `-jammy` resolve.
fn codename(word: &str) -> Option<(&'static str, &'static str)> {
    Some(match word {
        "stretch" => ("debian", "9"),
        "buster" => ("debian", "10"),
        "bullseye" => ("debian", "11"),
        "bookworm" => ("debian", "12"),
        "trixie" => ("debian", "13"),
        "bionic" => ("ubuntu", "18.04"),
        "focal" => ("ubuntu", "20.04"),
        "jammy" => ("ubuntu", "22.04"),
        "noble" => ("ubuntu", "24.04"),
        _ => return None,
    })
}

/// Leading dotted number of a tag: "16.20.2-alpine3.18" → "16.20.2".
fn leading_version(tag: &str) -> Option<String> {
    let t = tag.trim_start_matches(['v', 'V']);
    let end = t.find(|c: char| !(c.is_ascii_digit() || c == '.')).unwrap_or(t.len());
    let v = t[..end].trim_end_matches('.');
    (!v.is_empty() && v.chars().next().is_some_and(|c| c.is_ascii_digit())).then(|| v.to_string())
}

/// Parse one image reference ("registry/path/name:tag@digest").
pub(crate) fn parse_image(raw: &str, internal_host: &str) -> ImageRef {
    let s = raw.trim().trim_matches(['"', '\'']);
    if s.is_empty() || s.contains('$') || s.contains("{{") {
        return ImageRef::Unknown;
    }
    let s = s.split('@').next().unwrap_or(s); // a digest pins bytes; the tag still names the version
    let (registry, path) = match s.split_once('/') {
        Some((first, rest)) if first.contains('.') || first.contains(':') || first == "localhost" => (Some(first), rest),
        _ => (None, s),
    };
    if let Some(r) = registry {
        let host = r.split(':').next().unwrap_or(r);
        if (!internal_host.is_empty() && host == internal_host) || !PUBLIC_REGISTRIES.contains(&host) {
            return ImageRef::Internal;
        }
    }
    let (path, tag) = match path.rsplit_once(':') {
        Some((p, t)) => (p, Some(t)),
        None => (path, None),
    };
    let path = path.trim_start_matches("library/");
    let product = product_for_image(path);
    let mut refs = Vec::new();
    let tag_l = tag.map(str::to_ascii_lowercase).unwrap_or_default();
    if let Some(prod) = product {
        let version = leading_version(&tag_l).or_else(|| codename(&tag_l).filter(|(p, _)| *p == prod).map(|(_, v)| v.to_string()));
        if let Some(v) = version {
            refs.push(InfraRef { product: prod.into(), version: v, what: s.to_string() });
        }
    }
    // An embedded distro: "-alpine3.18", "-bookworm", "-slim-bullseye", "-jammy".
    for part in tag_l.split(['-', '_']).skip(1) {
        if let Some(v) = part.strip_prefix("alpine").filter(|v| !v.is_empty()) {
            refs.push(InfraRef { product: "alpine-linux".into(), version: v.into(), what: s.to_string() });
        } else if let Some((p, v)) = codename(part) {
            refs.push(InfraRef { product: p.into(), version: v.into(), what: s.to_string() });
        }
    }
    match (refs.is_empty(), product) {
        (false, _) => ImageRef::Versioned(refs),
        (true, Some(_)) if tag.is_none() || tag_l == "latest" || tag_l == "stable" || tag_l == "lts" => {
            ImageRef::Unpinned(s.to_string())
        }
        _ => ImageRef::Unknown,
    }
}

/// `FROM` images in a Dockerfile, with `ARG` defaults substituted. Stage aliases
/// (`FROM build AS x` … `FROM x`) are not images.
pub(crate) fn dockerfile_images(text: &str) -> Vec<String> {
    let mut args: BTreeMap<String, String> = BTreeMap::new();
    let mut stages: Vec<String> = Vec::new();
    let mut out = Vec::new();
    for line in text.lines() {
        let t = line.trim();
        let upper = t.to_ascii_uppercase();
        if let Some(rest) = upper.strip_prefix("ARG ").map(|_| t[4..].trim()) {
            if let Some((k, v)) = rest.split_once('=') {
                args.insert(k.trim().to_string(), v.trim().trim_matches(['"', '\'']).to_string());
            }
        } else if upper.starts_with("FROM ") {
            let words: Vec<&str> = t[5..].split_whitespace().filter(|w| !w.starts_with("--")).collect();
            let Some(image) = words.first() else { continue };
            if let (Some(kw), Some(alias)) = (words.get(1), words.get(2))
                && kw.eq_ignore_ascii_case("as")
            {
                stages.push(alias.to_ascii_lowercase());
            }
            let mut img = image.to_string();
            for (k, v) in &args {
                img = img.replace(&format!("${{{k}}}"), v).replace(&format!("${k}"), v);
            }
            if !stages.contains(&img.to_ascii_lowercase()) {
                out.push(img);
            }
        }
    }
    out
}

/// Images named in YAML: `image: x` (compose, CI, k8s manifests) and Helm-style
/// `repository: x` + `tag: y` pairs.
pub(crate) fn yaml_images(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut repository: Option<(String, usize)> = None;
    for line in text.lines() {
        let indent = line.len() - line.trim_start().len();
        let t = line.trim().trim_start_matches("- ");
        let Some((key, value)) = t.split_once(':') else { continue };
        let value = value.split(" #").next().unwrap_or("").trim().trim_matches(['"', '\'']);
        match key.trim() {
            "image" if !value.is_empty() => out.push(value.to_string()),
            "repository" if !value.is_empty() => repository = Some((value.to_string(), indent)),
            "tag" => {
                if let Some((repo, at)) = repository.take()
                    && at == indent
                    && !value.is_empty()
                {
                    out.push(format!("{repo}:{value}"));
                }
            }
            _ => {}
        }
    }
    out
}

/// Platform versions declared in Terraform: Kubernetes, managed database engines.
pub(crate) fn terraform_versions(text: &str) -> Vec<InfraRef> {
    let mut out = Vec::new();
    let mut engine: Option<String> = None;
    for line in text.lines() {
        let t = line.trim();
        let Some((key, value)) = t.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        let Some(v) = value.strip_prefix('"').and_then(|v| v.split('"').next()) else { continue };
        if v.contains("${") || v.is_empty() {
            continue;
        }
        let push = |out: &mut Vec<InfraRef>, product: &str, version: &str| {
            out.push(InfraRef { product: product.into(), version: version.into(), what: key.to_string() })
        };
        match key {
            "kubernetes_version" | "cluster_version" | "k8s_version" | "min_master_version" => {
                if let Some(ver) = leading_version(v) {
                    push(&mut out, "kubernetes", &ver);
                }
            }
            "engine" => engine = Some(v.to_ascii_lowercase()),
            "engine_version" => {
                let product = match engine.as_deref() {
                    Some("postgres" | "aurora-postgresql") => Some("postgresql"),
                    Some("mysql" | "aurora-mysql") => Some("mysql"),
                    Some("mariadb") => Some("mariadb"),
                    Some("redis") => Some("redis"),
                    Some("valkey") => Some("valkey"),
                    _ => None,
                };
                if let (Some(p), Some(ver)) = (product, leading_version(v)) {
                    push(&mut out, p, &ver);
                }
            }
            // Cloud SQL: "POSTGRES_14", "MYSQL_8_0".
            "database_version" => {
                if let Some((db, ver)) = v.split_once('_') {
                    let product = match db {
                        "POSTGRES" => "postgresql",
                        "MYSQL" => "mysql",
                        _ => continue,
                    };
                    push(&mut out, product, &ver.replace('_', "."));
                }
            }
            _ => {}
        }
    }
    out
}

/// Which infrastructure file a path is, if any.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum InfraKind {
    Dockerfile,
    Yaml,
    Terraform,
}

pub(crate) fn infra_kind(path: &str) -> Option<InfraKind> {
    if path.split('/').any(|seg| matches!(seg, "node_modules" | "vendor" | ".terraform" | "third_party")) {
        return None;
    }
    let name = path.rsplit('/').next().unwrap_or(path).to_ascii_lowercase();
    if name == "dockerfile" || name.starts_with("dockerfile.") || name.ends_with(".dockerfile") {
        return Some(InfraKind::Dockerfile);
    }
    if name.ends_with(".tf") {
        return Some(InfraKind::Terraform);
    }
    let yaml = name.ends_with(".yml") || name.ends_with(".yaml");
    let named = name == ".gitlab-ci.yml"
        || name.starts_with("docker-compose")
        || name.starts_with("compose.")
        || name.starts_with("values")
        || ["deployment", "statefulset", "daemonset", "cronjob", "job.", "pod."].iter().any(|k| name.contains(k));
    let k8s_dir = path.split('/').any(|seg| matches!(seg, "k8s" | "kubernetes" | "manifests" | "helm" | "charts" | "gitops"));
    (yaml && (named || k8s_dir)).then_some(InfraKind::Yaml)
}

// ─── endoflife.date ───

/// One release cycle of a product.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Cycle {
    pub cycle: String,
    /// `None` = still supported; `Some(None)` = EOL, date unknown.
    pub eol: Option<Option<NaiveDate>>,
    pub latest: Option<String>,
    pub codename: Option<String>,
}

pub(crate) fn parse_cycles(doc: &Value) -> Vec<Cycle> {
    doc.as_array()
        .into_iter()
        .flatten()
        .filter_map(|c| {
            let cycle = match &c["cycle"] {
                Value::String(s) => s.clone(),
                Value::Number(n) => n.to_string(),
                _ => return None,
            };
            let eol = match &c["eol"] {
                Value::Bool(false) | Value::Null => None,
                Value::Bool(true) => Some(None),
                Value::String(d) => Some(NaiveDate::parse_from_str(d, "%Y-%m-%d").ok()),
                _ => None,
            };
            let latest = c["latest"].as_str().map(str::to_string);
            let codename = c["codename"].as_str().map(|s| s.to_ascii_lowercase());
            Some(Cycle { cycle, eol, latest, codename })
        })
        .collect()
}

/// The cycle a version belongs to: the longest dotted prefix that names a cycle
/// ("16.20.2" → "16"; "3.18.4" → "3.18"), or a codename match.
pub(crate) fn match_cycle<'a>(cycles: &'a [Cycle], version: &str) -> Option<&'a Cycle> {
    let v = version.to_ascii_lowercase();
    if let Some(c) = cycles.iter().find(|c| c.codename.as_deref() == Some(v.as_str())) {
        return Some(c);
    }
    let parts: Vec<&str> = v.split('.').collect();
    (1..=parts.len()).rev().find_map(|n| {
        let prefix = parts[..n].join(".");
        cycles.iter().find(|c| c.cycle == prefix)
    })
}

/// Verdict for one declared version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Verdict {
    /// "CRITICAL" | "HIGH" | "MEDIUM" | "LOW".
    pub severity: &'static str,
    pub note: String,
    pub fix: String,
}

fn supported(c: &Cycle, today: NaiveDate) -> bool {
    match c.eol {
        None => true,
        Some(Some(d)) => d > today,
        Some(None) => false,
    }
}

/// End-of-life verdict, or `None` when the version is supported (or unknown to the
/// product's data — never guessed). `patch_lag` additionally flags a supported cycle
/// that is behind its latest patch: right for GitLab, whose patch releases are
/// security releases; noise for image tags, which usually name a minor.
pub(crate) fn verdict(cycles: &[Cycle], version: &str, today: NaiveDate, patch_lag: bool) -> Option<Verdict> {
    let cycle = match_cycle(cycles, version)?;
    // Cycles come newest first from endoflife.date.
    let target = cycles.iter().find(|c| supported(c, today)).map(|c| match c.eol {
        Some(Some(d)) => format!("→ {} (supported until {d})", c.cycle),
        _ => format!("→ {} (supported)", c.cycle),
    });
    let fix = target.unwrap_or_else(|| "no supported release".to_string());
    match cycle.eol {
        Some(Some(d)) if d <= today => Some(Verdict {
            severity: "HIGH",
            note: format!("{} reached end of life on {d}: no security fixes since", cycle.cycle),
            fix,
        }),
        Some(None) => Some(Verdict { severity: "HIGH", note: format!("{} is end of life", cycle.cycle), fix }),
        Some(Some(d)) if (d - today).num_days() <= 90 => Some(Verdict {
            severity: "MEDIUM",
            note: format!("{} reaches end of life on {d}", cycle.cycle),
            fix,
        }),
        _ if patch_lag => {
            let latest = cycle.latest.as_deref()?;
            (super::dependencies::compare_versions(latest, version) == std::cmp::Ordering::Greater).then(|| Verdict {
                severity: "HIGH",
                note: format!("behind the latest {} patch release ({latest}); patch releases carry security fixes", cycle.cycle),
                fix: format!("→ {latest}"),
            })
        }
        _ => None,
    }
}

/// An end-of-life finding, located.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct InfraFinding {
    /// Repository path, or the platform ("GitLab instance", "GitLab runner …").
    pub repo: String,
    pub files: Vec<String>,
    pub product: String,
    pub version: String,
    /// The declaration as written, e.g. "node:16-alpine" or "engine_version".
    pub what: String,
    pub verdict: Verdict,
}

/// endoflife.date client. `ENDOFLIFE_API_URL` points at a mirror or, in tests, at a
/// closed port.
pub(crate) struct EndOfLife {
    http: reqwest::Client,
    base: String,
}

impl EndOfLife {
    pub(crate) fn from_env() -> crate::error::Result<Self> {
        let base = std::env::var("ENDOFLIFE_API_URL").unwrap_or_else(|_| "https://endoflife.date/api".to_string());
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(20))
            .build()
            .map_err(|e| crate::error::Error::config(format!("endoflife.date client: {e}")))?;
        Ok(Self { http, base: base.trim_end_matches('/').to_string() })
    }

    /// Release cycles of one product, newest first.
    pub(crate) async fn cycles(&self, product: &str) -> crate::error::Result<Vec<Cycle>> {
        let resp = self.http.get(format!("{}/{}.json", self.base, urlencoding::encode(product))).send().await?;
        if !resp.status().is_success() {
            return Err(crate::error::Error::other(format!("endoflife.date {product}: HTTP {}", resp.status())));
        }
        Ok(parse_cycles(&resp.json().await?))
    }
}

/// Group located refs into findings: one per (repo, product, version), listing every
/// file that declares it. Supported versions produce nothing.
pub(crate) fn findings_for(
    repo: &str,
    refs: &[(String, InfraRef)],
    unpinned: &[(String, String)],
    cycles: &BTreeMap<String, Vec<Cycle>>,
    today: NaiveDate,
) -> Vec<InfraFinding> {
    let mut by_key: BTreeMap<(String, String), InfraFinding> = BTreeMap::new();
    for (file, r) in refs {
        let Some(c) = cycles.get(&r.product) else { continue };
        let Some(v) = verdict(c, &r.version, today, false) else { continue };
        let f = by_key.entry((r.product.clone(), r.version.clone())).or_insert_with(|| InfraFinding {
            repo: repo.to_string(),
            files: Vec::new(),
            product: r.product.clone(),
            version: r.version.clone(),
            what: r.what.clone(),
            verdict: v,
        });
        if !f.files.contains(file) {
            f.files.push(file.clone());
        }
    }
    let mut out: Vec<InfraFinding> = by_key.into_values().collect();
    let mut seen = std::collections::BTreeSet::new();
    for (file, image) in unpinned {
        if seen.insert(image.clone()) {
            out.push(InfraFinding {
                repo: repo.to_string(),
                files: vec![file.clone()],
                product: image.split(':').next().unwrap_or(image).to_string(),
                version: "latest".into(),
                what: image.clone(),
                verdict: Verdict {
                    severity: "LOW",
                    note: "unpinned: each build pulls whatever is newest, so what runs is unknown".into(),
                    fix: "pin a supported version".into(),
                },
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    const HOST: &str = "git.example.com";

    fn versions(r: ImageRef) -> Vec<(String, String)> {
        match r {
            ImageRef::Versioned(v) => v.into_iter().map(|r| (r.product, r.version)).collect(),
            other => panic!("expected versioned, got {other:?}"),
        }
    }

    #[test]
    fn images_resolve_to_products_and_embedded_distros() {
        assert_eq!(versions(parse_image("node:16.20.2-alpine3.18", HOST)), vec![("nodejs".into(), "16.20.2".into()), ("alpine-linux".into(), "3.18".into())]);
        assert_eq!(versions(parse_image("docker.io/library/postgres:13", HOST)), vec![("postgresql".into(), "13".into())]);
        assert_eq!(versions(parse_image("python:3.9-slim-bullseye", HOST)), vec![("python".into(), "3.9".into()), ("debian".into(), "11".into())]);
        assert_eq!(versions(parse_image("debian:bookworm", HOST)), vec![("debian".into(), "12".into())]);
        assert_eq!(versions(parse_image("bitnami/redis:7.2.4", HOST)), vec![("redis".into(), "7.2.4".into())]);
        assert_eq!(versions(parse_image("mcr.microsoft.com/dotnet/aspnet:6.0", HOST)), vec![("dotnet".into(), "6.0".into())]);
        assert_eq!(versions(parse_image("php:7.4-fpm@sha256:abc", HOST)), vec![("php".into(), "7.4".into())]);
    }

    #[test]
    fn private_unpinned_and_templated_images_are_classified_not_guessed() {
        assert_eq!(parse_image("git.example.com:5050/acme/api:1.2", HOST), ImageRef::Internal);
        assert_eq!(parse_image("registry.example.com/acme/web:latest", HOST), ImageRef::Internal);
        assert_eq!(parse_image("nginx:latest", HOST), ImageRef::Unpinned("nginx:latest".into()));
        assert_eq!(parse_image("redis", HOST), ImageRef::Unpinned("redis".into()));
        assert_eq!(parse_image("node:${NODE_VERSION}", HOST), ImageRef::Unknown);
        assert_eq!(parse_image("acme/tool:2.0", HOST), ImageRef::Unknown, "a product we don't track");
    }

    #[test]
    fn dockerfile_resolves_args_and_skips_stage_aliases() {
        let df = "ARG NODE_VERSION=18\nFROM --platform=linux/amd64 node:${NODE_VERSION}-alpine AS build\nRUN npm ci\nFROM build AS test\nFROM nginx:1.24\nCOPY --from=build /app /usr/share/nginx/html\n";
        assert_eq!(dockerfile_images(df), vec!["node:18-alpine", "nginx:1.24"]);
    }

    #[test]
    fn yaml_finds_image_lines_and_helm_repository_tag_pairs() {
        let y = "services:\n  db:\n    image: \"postgres:14.5\"\n  - image: redis:6 # cache\nimage:\n  repository: bitnami/rabbitmq\n  tag: 3.11.2\nother:\n  repository: x\n    tag: wrong-level\n";
        assert_eq!(yaml_images(y), vec!["postgres:14.5", "redis:6", "bitnami/rabbitmq:3.11.2"]);
    }

    #[test]
    fn terraform_platform_versions() {
        let tf = "resource \"aws_db_instance\" \"main\" {\n  engine         = \"postgres\"\n  engine_version = \"12.17\"\n}\nmodule \"eks\" {\n  cluster_version = \"1.27\"\n}\nresource \"google_sql_database_instance\" \"x\" {\n  database_version = \"MYSQL_8_0\"\n}\nvariable \"v\" { default = \"${var.x}\" }\n";
        let got: Vec<(String, String)> = terraform_versions(tf).into_iter().map(|r| (r.product, r.version)).collect();
        assert_eq!(got, vec![("postgresql".into(), "12.17".into()), ("kubernetes".into(), "1.27".into()), ("mysql".into(), "8.0".into())]);
    }

    #[test]
    fn infra_files_are_recognised() {
        for p in ["Dockerfile", "docker/api.Dockerfile", "Dockerfile.prod", "infra/main.tf", ".gitlab-ci.yml", "docker-compose.prod.yml", "charts/api/values.yaml", "k8s/service.yaml", "deploy/api-deployment.yml"] {
            assert!(infra_kind(p).is_some(), "{p}");
        }
        for p in ["src/config.yml", "node_modules/x/Dockerfile", ".terraform/modules/x/main.tf", "README.md"] {
            assert!(infra_kind(p).is_none(), "{p}");
        }
    }

    fn cycles() -> Vec<Cycle> {
        parse_cycles(&json!([
            {"cycle": "22", "eol": "2027-04-30", "latest": "22.11.0"},
            {"cycle": "20", "eol": "2026-04-30", "latest": "20.19.0"},
            {"cycle": "18", "eol": "2025-04-30", "latest": "18.20.8"},
            {"cycle": "3.18", "eol": true, "latest": "3.18.12"},
            {"cycle": 12, "eol": false, "latest": "12.9", "codename": "Bookworm"},
        ]))
    }

    fn day(s: &str) -> NaiveDate {
        NaiveDate::parse_from_str(s, "%Y-%m-%d").unwrap()
    }

    #[test]
    fn cycles_match_by_longest_prefix_and_codename() {
        let c = cycles();
        assert_eq!(match_cycle(&c, "18.19.1").unwrap().cycle, "18");
        assert_eq!(match_cycle(&c, "3.18").unwrap().cycle, "3.18");
        assert_eq!(match_cycle(&c, "bookworm").unwrap().cycle, "12", "numeric cycle and codename");
        assert!(match_cycle(&c, "16").is_none(), "unknown cycle: no verdict, never a guess");
    }

    #[test]
    fn verdicts_eol_soon_supported_and_patch_lag() {
        let c = cycles();
        let today = day("2026-02-15");
        let v = verdict(&c, "18.19.1", today, false).unwrap();
        assert_eq!(v.severity, "HIGH");
        assert!(v.note.contains("2025-04-30"));
        assert_eq!(v.fix, "→ 22 (supported until 2027-04-30)", "newest supported cycle");
        assert_eq!(verdict(&c, "20", today, false).unwrap().severity, "MEDIUM", "EOL within 90 days");
        assert!(verdict(&c, "22.1.0", today, false).is_none(), "supported, no patch-lag check for images");
        let lag = verdict(&c, "22.1.0", today, true).unwrap();
        assert_eq!((lag.severity, lag.fix.as_str()), ("HIGH", "→ 22.11.0"), "GitLab-style patch lag");
        assert!(verdict(&c, "22.11.0", today, true).is_none(), "on the latest patch");
        assert_eq!(verdict(&c, "3.18", today, false).unwrap().note, "3.18 is end of life");
    }
}
