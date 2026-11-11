//! Secrets written into CI files (ADR 077).
//!
//! A credential typed into `.gitlab-ci.yml` is readable by everyone who can read the
//! repository, by every fork and clone, and stays in git history after the line is
//! deleted. The variable audit never sees it: it is not a CI variable at all.
//!
//! **No value ever leaves this module.** A hit carries the file, the line, the key it
//! was assigned to, how it was written, whether the line is commented out, and the
//! value's length. A scanner that prints the secrets it finds into a report or a
//! logged transcript has reproduced the leak it reports.

use std::sync::LazyLock;

use regex::Regex;

/// How the secret was written.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum SecretKind {
    /// `PASSWORD: value`, `export TOKEN=value`, `FOO_SECRET=value cmd`.
    Assignment,
    /// `PRIVATE-TOKEN: value`, `Authorization: Bearer value` in a script line.
    Header,
    /// `--password value`, `--token=value`.
    Flag,
    /// `scheme://user:password@host`.
    UrlCredential,
    /// A credential recognisable by its own format, whatever it is assigned to.
    KnownFormat(&'static str),
    /// The initial password of a database service container (`MYSQL_ROOT_PASSWORD`,
    /// `POSTGRES_PASSWORD`, …): usually a throwaway test database, so one level lower.
    ServiceDatabase,
}

impl SecretKind {
    pub(crate) fn describe(self) -> String {
        match self {
            Self::Assignment => "literal value assigned".into(),
            Self::Header => "literal credential in an HTTP header".into(),
            Self::Flag => "literal value passed on the command line".into(),
            Self::UrlCredential => "password embedded in a URL".into(),
            Self::KnownFormat(what) => format!("{what} in the file"),
            Self::ServiceDatabase => "literal password for a database service container (fine for a throwaway test database; never reuse it)".into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SecretHit {
    pub path: String,
    pub line: usize,
    /// The variable, header or flag name; "-" for a bare credential.
    pub key: String,
    pub kind: SecretKind,
    /// The line is commented out: no longer used, still in the file and in history.
    pub commented: bool,
    /// Length of the value. The value itself is never kept.
    pub len: usize,
}

/// CI definition files: the root file and the conventional homes of included files.
pub(crate) fn is_ci_file(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let yaml = name.ends_with(".yml") || name.ends_with(".yaml");
    name == ".gitlab-ci.yml"
        || (yaml && (name.ends_with(".gitlab-ci.yml") || name.ends_with(".gitlab-ci.yaml")))
        || (yaml && (lower.starts_with(".gitlab/") || lower.starts_with("ci/") || lower.contains("/ci/")))
}

/// Key fragments that name a secret. Bare `KEY` is deliberately absent: GitLab's own
/// `cache: key:` would match it on nearly every pipeline.
const SECRET_KEY: &[&str] = &[
    "PASSWORD", "PASSWD", "PASSPHRASE", "PWD", "SECRET", "TOKEN", "API_KEY", "APIKEY",
    "ACCESS_KEY", "PRIVATE_KEY", "SIGNING_KEY", "CREDENTIAL", "WEBHOOK", "DSN",
];

/// Fragments that make a secret-sounding key something else: who, where, how long.
const NOT_SECRET: &[&str] = &[
    "USER", "LOGIN", "EMAIL", "_ID", "_PATH", "_FILE", "_NAME", "_TYPE", "_LENGTH",
    "_TTL", "_EXPIR", "_HEADER", "_SCOPE", "_ENABLED", "_DISABLED", "_REQUIRED",
];

/// Initialisation variables of the official database images: they set the password
/// of the service container the job starts, not of a real database.
fn service_database_key(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    ["MYSQL_", "MARIADB_", "POSTGRES_", "MONGO_INITDB_"].iter().any(|p| k.starts_with(p))
}

impl SecretHit {
    /// HIGH for a live credential; MEDIUM when commented out or a service-container
    /// password.
    pub(crate) fn high(&self) -> bool {
        !self.commented && self.kind != SecretKind::ServiceDatabase
    }
}

pub(crate) fn secret_key(key: &str) -> bool {
    let k = key.to_ascii_uppercase().replace(['-', '.'], "_");
    SECRET_KEY.iter().any(|m| k.contains(m)) && !NOT_SECRET.iter().any(|m| k.contains(m))
}

/// A value that holds no credential: a reference resolved at run time, a
/// placeholder, a flag value or a path.
fn harmless(value: &str) -> bool {
    let v = unquote(value);
    if v.is_empty() || REFERENCE.is_match(v) {
        return true; // `$VAR`, `${VAR}`, `Bearer $TOKEN`, `$(cat file)`
    }
    let lower = v.to_ascii_lowercase();
    let placeholder = v.chars().all(|c| matches!(c, '*' | 'x' | 'X' | '.' | '-' | '_'))
        || (v.starts_with('<') && v.ends_with('>'))
        || v.starts_with("{{") // a template; the pattern stops at its first space
        || ["changeme", "change_me", "redacted", "todo", "none", "null", "~", "your_", "example"]
            .iter()
            .any(|p| lower == *p || lower.starts_with(p));
    let flag = ["true", "false", "yes", "no", "on", "off", "|", ">", "|-", ">-"].contains(&lower.as_str());
    let file_like = [".json", ".pem", ".key", ".p12", ".pfx", ".crt", ".cer", ".jks", ".keystore", ".env", ".txt", ".yml", ".yaml"]
        .iter()
        .any(|ext| lower.ends_with(ext));
    let path = v.starts_with('/') || v.starts_with("./") || v.starts_with("~/") || v.starts_with('.') || (file_like && !v.contains("://"));
    let too_short = v.chars().count() < 3; // no real credential is this short
    placeholder || flag || path || too_short
}

/// The value without its quotes, escaped quotes (`\"…\"` inside an `echo`) included.
fn unquote(value: &str) -> &str {
    value.trim().trim_matches(|c| matches!(c, '"' | '\'' | '\\')).trim()
}

/// A run-time reference: `$NAME`, `${NAME}`, `$(command)`. A `$` followed by anything
/// else is part of a literal (a password may contain one), not a reference.
static REFERENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\$(?:\{?[A-Za-z_]|\()").unwrap());

fn value_len(value: &str) -> usize {
    unquote(value).chars().count()
}

/// A value: double-quoted (escapes allowed, `\"…\"` too), single-quoted, or bare.
const VALUE: &str = r#"(\\?"(?:[^"\\]|\\.)*\\?"|\\?'[^']*'|[^\s"'#;]+)"#;

/// YAML `KEY: value` — the whole line, so `token: …` inside a sentence in an `echo`
/// or a comment is prose, not an assignment.
static YAML_KV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"^(?:-\s+)?["']?([A-Za-z_][A-Za-z0-9_.-]*)["']?\s*:\s+{VALUE}\s*(?:#.*)?$"#)).unwrap()
});
/// Shell `KEY=value`, anywhere on the line (`export`, inline env, form fields).
static SHELL_KV: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(&format!(r#"(?:^|[\s;&("'])(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)={VALUE}"#)).unwrap()
});
static HEADER: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)\b(private-token|job-token|deploy-token|x-api-key|api-key|authorization)\s*:\s*(?:(?:bearer|basic|token)\s+)?([^\s"'\\]+)"#).unwrap()
});
static FLAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"(?i)(--(?:password|passwd|pass|token|api-key|apikey|secret|client-secret|auth-token))(?:=|\s+)("[^"]*"|'[^']*'|[^\s"'\\]+)"#).unwrap()
});
static URL_CRED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"[A-Za-z][A-Za-z0-9+.-]*://([^/\s:@"']+):([^@\s/"']+)@"#).unwrap()
});

/// Credential formats recognisable without a key. Each pattern is specific enough
/// that a match is the credential itself, not something that resembles one.
static KNOWN: LazyLock<Vec<(&'static str, Regex)>> = LazyLock::new(|| {
    [
        ("a GitLab access token", r"\bgl(?:pat|dt|cbt|ptt|rt|oas)-[A-Za-z0-9_-]{20,}"),
        ("a GitHub token", r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{50,})"),
        ("a Slack token", r"\bxox[abposr]-[A-Za-z0-9-]{10,}"),
        ("an AWS access key id", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
        ("a Google API key", r"\bAIza[0-9A-Za-z_-]{35}\b"),
        ("a Stripe live key", r"\b[sr]k_live_[0-9A-Za-z]{20,}"),
    ]
    .into_iter()
    .map(|(what, re)| (what, Regex::new(re).unwrap()))
    // The PEM armour is assembled, so this file never carries a key-shaped line.
    .chain(std::iter::once((
        "a private key block",
        Regex::new(&format!("-----BEGIN (?:[A-Z]+ )?{}-----", ["PRIVATE", "KEY"].join(" "))).unwrap(),
    )))
    .collect()
});

/// Every literal credential in one CI file.
pub(crate) fn find_secrets(path: &str, content: &str) -> Vec<SecretHit> {
    let mut out: Vec<SecretHit> = Vec::new();
    for (i, raw) in content.lines().enumerate() {
        let trimmed = raw.trim_start();
        let commented = trimmed.starts_with('#');
        let line = trimmed.trim_start_matches('#').trim_start();
        let n = i + 1;
        let mut push = |key: &str, kind: SecretKind, len: usize| {
            if !out.iter().any(|h| h.line == n && h.key == key) {
                out.push(SecretHit { path: path.to_string(), line: n, key: key.to_string(), kind, commented, len });
            }
        };
        for c in KNOWN.iter().filter_map(|(what, re)| re.find(line).map(|m| (*what, m.len()))) {
            push("-", SecretKind::KnownFormat(c.0), c.1);
        }
        for c in HEADER.captures_iter(line) {
            if !harmless(&c[2]) {
                push(&c[1], SecretKind::Header, value_len(&c[2]));
            }
        }
        for c in FLAG.captures_iter(line) {
            if !harmless(&c[2]) {
                push(&c[1], SecretKind::Flag, value_len(&c[2]));
            }
        }
        for c in URL_CRED.captures_iter(line) {
            if !harmless(&c[2]) {
                push("url", SecretKind::UrlCredential, value_len(&c[2]));
            }
        }
        for c in YAML_KV.captures_iter(line).chain(SHELL_KV.captures_iter(line)) {
            // A header written as `Authorization: …` is reported once, as a header.
            if secret_key(&c[1]) && !harmless(&c[2]) && !HEADER.is_match(line) {
                let kind = if service_database_key(&c[1]) { SecretKind::ServiceDatabase } else { SecretKind::Assignment };
                push(&c[1], kind, value_len(&c[2]));
            }
        }
    }
    // A bare credential on a line already reported under its key adds nothing.
    let keyed: Vec<usize> = out.iter().filter(|h| h.key != "-").map(|h| h.line).collect();
    out.retain(|h| h.key != "-" || !keyed.contains(&h.line));
    out
}

/// Local files a CI definition includes (`include: 'path'`, `- local: path`), so the
/// check follows the definition instead of stopping at the root file.
pub(crate) fn local_includes(content: &str) -> Vec<String> {
    static LOCAL: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r#"(?m)^\s*(?:-\s*)?(?:local:|include:)\s*["']?(/?[^\s"'*$]+\.ya?ml)["']?\s*$"#).unwrap());
    let mut out: Vec<String> = LOCAL
        .captures_iter(content)
        .map(|c| c[1].trim_start_matches('/').to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    // Credential-shaped test values are assembled at run time: a key-shaped
    // literal in the repository trips secret scanners and teaches the pattern.
    fn word(n: usize) -> String {
        "s3".repeat(n).chars().take(n).collect()
    }

    #[test]
    fn literal_assignments_are_found_references_are_not() {
        let pw = word(19);
        let ci = format!(
            "variables:\n  DEPLOY_PASSWORD: \"{pw}\"\n  STORE_TOKEN: $STORE_TOKEN\n  REGISTRY_USERNAME: deploy-bot\n\
             build:\n  cache:\n    key: deps-v1\n  script:\n    - export SIGNING_SECRET={pw}\n    - echo ${{API_KEY}}\n\
             # OLD_STORE_PASSWORD: {pw}\n"
        );
        let hits = find_secrets(".gitlab-ci.yml", &ci);
        let keys: Vec<(&str, usize, bool)> = hits.iter().map(|h| (h.key.as_str(), h.line, h.commented)).collect();
        assert_eq!(keys, vec![("DEPLOY_PASSWORD", 2, false), ("SIGNING_SECRET", 9, false), ("OLD_STORE_PASSWORD", 11, true)]);
        assert!(hits.iter().all(|h| h.len == 19), "length is reported, the value is not");
        let debug = format!("{hits:?}");
        assert!(!debug.contains(&pw), "no value is kept anywhere in a hit");
    }

    #[test]
    fn headers_flags_urls_and_known_formats() {
        let tok = word(24);
        let ci = format!(
            "  - curl -H \"PRIVATE-TOKEN: {tok}\" https://gitlab.example.com/api/v4/projects\n\
             \x20 - curl -H \"Authorization: Bearer $CI_JOB_TOKEN\" https://example.com\n\
             \x20 - tool upload --password {tok} --user ci\n\
             \x20 - git clone https://ci:{tok}@git.example.com/g/p.git\n\
             \x20 - echo {}{}\n",
            "glpat-", word(20)
        );
        let hits = find_secrets("ci/deploy.yml", &ci);
        let got: Vec<(usize, SecretKind)> = hits.iter().map(|h| (h.line, h.kind)).collect();
        assert_eq!(
            got,
            vec![
                (1, SecretKind::Header),
                (3, SecretKind::Flag),
                (4, SecretKind::UrlCredential),
                (5, SecretKind::KnownFormat("a GitLab access token")),
            ],
            "Bearer $CI_JOB_TOKEN on line 2 is a reference, not a secret"
        );
    }

    #[test]
    fn placeholders_flags_and_paths_are_not_secrets() {
        let dollar = format!("  DEPLOY_PASSWORD: \"{}$9{}\"\n", word(4), word(4));
        assert_eq!(find_secrets(".gitlab-ci.yml", &dollar).len(), 1, "a $ inside a literal is not a reference");
        for v in ["\"\"", "'***'", "xxxx", "<your-token>", "changeme", "true", "/run/secrets/token", "|", "{{ .Token }}"] {
            let hits = find_secrets(".gitlab-ci.yml", &format!("  DEPLOY_TOKEN: {v}\n"));
            assert!(hits.is_empty(), "{v} flagged");
        }
        assert!(!secret_key("SECRET_DETECTION_DISABLED") && !secret_key("TOKEN_TTL") && !secret_key("key"));
        assert!(secret_key("registry.password") && secret_key("SIGNING_PASSWORD"));
    }

    #[test]
    fn shapes_seen_in_real_pipelines_are_judged_right() {
        let tok = word(30);
        let ci = [
            r#"  - echo "CLOUD_SECRET_ACCESS_KEY=\"${CLOUD_KEY}\"" > .env"#.to_string(),
            r#"  - echo "Missing token: set it in the CI/CD settings?"; exit 1"#.to_string(),
            "  GOOGLE_APPLICATION_CREDENTIALS: .config/service_account.json".to_string(),
            format!("  MYSQL_ROOT_PASSWORD: {}", word(6)),
            format!(r#"  - upload -F token={tok} -F "mode=upload" https://api.example.com/v1/files"#),
            "# secret: don't commit it".to_string(),
        ]
        .join("\n");
        let hits = find_secrets(".gitlab-ci.yml", &ci);
        let got: Vec<(usize, &str, SecretKind, bool)> = hits.iter().map(|h| (h.line, h.key.as_str(), h.kind, h.high())).collect();
        assert_eq!(
            got,
            vec![(4, "MYSQL_ROOT_PASSWORD", SecretKind::ServiceDatabase, false), (5, "token", SecretKind::Assignment, true)],
            "an escaped reference, prose, a key-file path and a commented sentence are not secrets"
        );
    }

    #[test]
    fn ci_files_and_local_includes() {
        for p in [".gitlab-ci.yml", "build.gitlab-ci.yml", ".gitlab/ci/release.yml", "ci/deploy.yaml", "pipelines/ci/x.yml"] {
            assert!(is_ci_file(p), "{p}");
        }
        for p in ["docker-compose.yml", "config/app.yml", ".gitlab-ci.yml.bak", "ci/notes.md"] {
            assert!(!is_ci_file(p), "{p}");
        }
        let root = "include:\n  - local: '/ci/build.yml'\n  - local: ci/release.yml\n  - project: other/p\n    file: x.yml\n";
        assert_eq!(local_includes(root), vec!["ci/build.yml", "ci/release.yml"]);
        assert_eq!(local_includes("include: '.gitlab/ci/all.yml'\n"), vec![".gitlab/ci/all.yml"]);
    }
}
