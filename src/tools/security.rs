//! CI security audit: configuration-level exposure checks across a project or group.
//!
//! Every detector here exists because the corresponding failure happened, not because
//! it appeared on a checklist. Each carries the shape of the incident it came from.
//!
//! **This tool never reports a secret's value.** It reports the *name* and the
//! *location* of a setting that would fail to protect one. A scanner that prints the
//! credentials it finds into a logged transcript has reproduced the bug it hunts.

use crate::client::GitLabClient;
use crate::error::Result;
use std::fmt::Write as _;
use serde_json::Value;

/// Key substrings that mark a variable as holding a secret.
const SECRETISH: &[&str] = &[
    "SECRET",
    "TOKEN",
    "PASSWORD",
    "PASSWD",
    "PASS",
    "KEY",
    "CREDENTIAL",
    "PRIVATE",
    "AUTH",
    "CERT",
    "CRT",
    "PEM",
    "SALT",
    "SIGNATURE",
    "WEBHOOK",
    "DSN",
];

/// Keys whose value is structurally unmaskable, so "turn masking on" is wrong advice.
///
/// GitLab requires a masked value to be single-line, at least 8 characters, and drawn
/// from a restricted alphabet. A PEM block or an SSH key fails all three: the API
/// returns 400, and anyone following "just tick masked" concludes the tool is broken.
/// The real remedy is `variable_type: file`, which also keeps the value off any
/// command line where `set -x` could echo it.
fn is_structurally_unmaskable(key: &str) -> bool {
    let k = key.to_ascii_uppercase();
    [
        "PRIV_KEY",
        "PRIVATE_KEY",
        "SSH_KEY",
        "SSH_KEYS",
        "_PEM",
        "CERT",
        "_CRT",
    ]
    .iter()
    .any(|m| k.contains(m))
}

/// Finding severity. Declaration order is report order: `Ord` puts High first.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) enum Severity {
    High,
    Medium,
}

impl std::fmt::Display for Severity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::High => "HIGH",
            Self::Medium => "MEDIUM",
        })
    }
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Finding {
    pub severity: Severity,
    pub code: &'static str,
    pub where_: String,
    pub detail: String,
    pub fix: String,
}

/// D1 — a secret-shaped variable that GitLab would not redact from a job log.
///
/// This is the mechanism, not a hypothetical: an unmasked variable echoed by a script
/// running under `set -x` lands in the trace in cleartext and stays there.
/// Names whose last word says what they describe, not that they hold a secret:
/// where (`STORAGE_PRIVATE_BUCKET`), who (`REGISTRY_TOKEN_USER`), or a setting
/// (`TOKEN_TTL`). The last word decides, so `DB_USER_PASSWORD` is still a password.
fn descriptive_key(key: &str) -> bool {
    let k = key.to_ascii_uppercase().replace(['-', '.'], "_");
    let last = k.rsplit('_').find(|w| !w.is_empty()).unwrap_or("");
    [
        "BUCKET", "ENDPOINT", "REGION", "HOST", "HOSTNAME", "PORT", "DIR", "USER", "USERNAME", "LOGIN", "EMAIL",
        "NAME", "TYPE", "TTL", "EXPIRES", "EXPIRY", "LENGTH", "HEADER", "SCOPE", "SCOPES", "FILE", "PATH",
        "VERSION", "ENABLED", "DISABLED", "MODE", "FORMAT", "PREFIX",
    ]
    .contains(&last)
}

/// How an unmasked secret variable holds its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VarKind {
    /// A single-line secret GitLab could mask.
    Secret,
    /// Multi-line or PEM-armoured: masking is refused, file type is the answer.
    KeyMaterial,
    /// JSON (`DOCKER_AUTH_CONFIG`): GitLab cannot mask it at all.
    Json,
}

/// A CI/CD variable that holds a secret and is not masked. Like a CI-file hit, it
/// keeps no value: name, scope, protection, kind, length and a per-run fingerprint
/// (ADR 077) so one token stored in many projects reads as one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct VarHit {
    pub key: String,
    pub scope: String,
    pub protected: bool,
    pub kind: VarKind,
    pub len: usize,
    pub fingerprint: u64,
}

impl VarHit {
    pub(crate) fn high(&self) -> bool {
        self.kind == VarKind::KeyMaterial
    }
}

/// The variable when it holds a secret GitLab would print in a job log. The value is
/// looked at, never kept: empty values, references to other variables, booleans and
/// paths are not secrets whatever the name says. Without a value (a token that may
/// list variables but not read them) the name decides.
/// A secret-named, unmasked variable whose value is not a secret, and why: shown in
/// the audit so a skip can be checked without anyone seeing the value.
pub(crate) fn skipped_by_value(v: &Value) -> Option<(String, &'static str)> {
    let key = v["key"].as_str().unwrap_or("");
    let masked = v["masked"].as_bool().unwrap_or(false) || v["hidden"].as_bool().unwrap_or(false);
    let upper = key.to_ascii_uppercase();
    if masked || v["variable_type"].as_str() == Some("file") || !SECRETISH.iter().any(|m| upper.contains(m)) {
        return None;
    }
    if super::ci_exposure::identifier_key(key) {
        return Some((key.to_string(), "names a key, signs nothing"));
    }
    if descriptive_key(key) {
        return Some((key.to_string(), "name describes, does not hold, a secret"));
    }
    let t = v["value"].as_str()?.trim();
    if super::ci_exposure::public_keys_only(t) {
        return Some((key.to_string(), "public keys or certificates only"));
    }
    if t.starts_with('{') || t.starts_with('[') || t.contains('\n') {
        return None;
    }
    super::ci_exposure::harmless_reason(t).map(|r| (key.to_string(), r))
}

pub(crate) fn unmasked_secret(v: &Value) -> Option<VarHit> {
    let key = v["key"].as_str().unwrap_or("");
    let masked = v["masked"].as_bool().unwrap_or(false) || v["hidden"].as_bool().unwrap_or(false);
    if masked || v["variable_type"].as_str() == Some("file") || key.eq_ignore_ascii_case("CI_DEBUG_TRACE") {
        return None;
    }
    let upper = key.to_ascii_uppercase();
    let secretish = SECRETISH.iter().any(|m| upper.contains(m));
    if !secretish || super::ci_exposure::identifier_key(key) || descriptive_key(key) {
        return None;
    }
    let value = v["value"].as_str();
    if value.is_some_and(super::ci_exposure::public_keys_only) {
        return None;
    }
    let kind = match value.map(str::trim) {
        Some(t) if t.starts_with('{') || t.starts_with('[') => VarKind::Json,
        Some(t) if t.contains('\n') || t.contains("-----BEGIN") => VarKind::KeyMaterial,
        Some(t) if super::ci_exposure::harmless(t) => return None,
        Some(_) => VarKind::Secret,
        None if is_structurally_unmaskable(key) => VarKind::KeyMaterial,
        None => VarKind::Secret,
    };
    Some(VarHit {
        key: key.to_string(),
        scope: v["environment_scope"].as_str().unwrap_or("*").to_string(),
        protected: v["protected"].as_bool().unwrap_or(false),
        kind,
        len: value.map_or(0, |t| t.trim().chars().count()),
        fingerprint: value.map_or(0, |t| super::ci_exposure::fingerprint(t)),
    })
}

pub(crate) fn audit_variables(project: &str, vars: &[Value]) -> Vec<Finding> {
    let mut out = Vec::new();
    for v in vars {
        let key = v["key"].as_str().unwrap_or("");
        let scope = v["environment_scope"].as_str().unwrap_or("*");

        if key.eq_ignore_ascii_case("CI_DEBUG_TRACE") {
            let on = matches!(
                v["value"]
                    .as_str()
                    .unwrap_or("")
                    .trim()
                    .to_ascii_lowercase()
                    .as_str(),
                "true" | "1" | "yes"
            );
            if on {
                out.push(Finding {
                    severity: Severity::High,
                    code: "CI-DEBUG",
                    where_: format!("{project} (scope={scope})"),
                    detail: "CI_DEBUG_TRACE is enabled — GitLab prints every variable, including masked ones, into the job log.".to_string(),
                    fix: "Set it to false; use it only transiently and purge the affected job logs afterwards.".to_string(),
                });
            }
            continue;
        }

        let Some(h) = unmasked_secret(v) else { continue };
        let unprotected = if h.protected {
            ""
        } else {
            " Not protected: the pipeline of every branch can read it."
        };
        let (severity, detail, fix) = match h.kind {
            VarKind::KeyMaterial => (
                Severity::High,
                "Holds key material and is neither masked nor file-type: if any job echoes it, the value is written to the log in full.",
                "Set variable_type=file. Masking cannot apply — GitLab rejects multi-line values, so enabling it will fail with 400.",
            ),
            VarKind::Json => (
                Severity::Medium,
                "Holds JSON, which GitLab cannot mask: if a job prints it, the credentials inside appear in the log.",
                "Keep it protected and environment-scoped, and put a read-only robot credential in it rather than a person's.",
            ),
            VarKind::Secret => (
                Severity::Medium,
                "Secret-shaped variable is not masked: GitLab will not redact it if a job prints it.",
                "Enable masking. If the API refuses it, the value breaks GitLab's masking rules (single line, ≥8 chars, restricted alphabet) — use variable_type=file instead.",
            ),
        };
        out.push(Finding {
            severity,
            code: "VAR-UNMASKED",
            where_: format!("{project} → {key} (scope={scope})"),
            detail: format!("{detail}{unprotected}"),
            fix: fix.to_string(),
        });
    }
    out
}

/// D2/D3 — supply-chain exposure in a CI definition.
///
/// Two distinct failures, both observed. A floating image tag means a rebuild changes
/// the build environment with no diff and no warning — a CI image that silently lost
/// its Python interpreter took an app pipeline down exactly this way. An unpinned
/// remote fetch means whatever that URL serves today is executed inside CI, where the
/// registry credentials and secret-store client secrets live.
pub(crate) fn audit_ci_file(project: &str, path: &str, content: &str) -> Vec<Finding> {
    let mut out = Vec::new();
    for (i, raw) in content.lines().enumerate() {
        let line = raw.trim();
        let n = i + 1;
        if line.starts_with('#') {
            continue;
        }

        if let Some(rest) = line.strip_prefix("image:") {
            let img = rest.trim().trim_matches('"').trim_matches('\'');
            if !img.is_empty() && !img.starts_with('$') {
                let floating =
                    img.ends_with(":latest") || !img.rsplit('/').next().unwrap_or("").contains(':');
                if floating && !img.contains('@') {
                    out.push(Finding {
                        severity: Severity::Medium,
                        code: "IMG-FLOATING",
                        where_: format!("{project} → {path}:{n}"),
                        detail: format!("`{img}` is a floating tag — a rebuild silently changes the build environment."),
                        fix: "Pin a digest (image@sha256:…) or an immutable dated tag.".to_string(),
                    });
                }
            }
        }

        let lower = line.to_ascii_lowercase();
        let piped_to_shell = (lower.contains("curl ") || lower.contains("wget "))
            && (lower.contains("| sh")
                || lower.contains("|sh")
                || lower.contains("| bash")
                || lower.contains("|bash"));
        let latest_release = lower.contains("releases/latest/download");
        if piped_to_shell || latest_release {
            out.push(Finding {
                severity: Severity::High,
                code: "FETCH-UNPINNED",
                where_: format!("{project} → {path}:{n}"),
                detail: if latest_release {
                    "Downloads a `latest` release artifact with no version pin and no checksum, then runs it inside CI.".to_string()
                } else {
                    "Pipes a remote download straight into a shell — whatever that URL serves is executed with CI credentials in scope.".to_string()
                },
                fix: "Pin an exact version, verify a published checksum, or vendor the artifact into your own registry.".to_string(),
            });
        }
    }
    out
}

/// D4 — a credential written into a CI file (ADR 077).
///
/// Not a CI variable at all, so the variable checks never see it: anyone who can read
/// the repository has it, so does every clone, and deleting the line leaves it in git
/// history. The finding names the key and the line; the value is never read out.
pub(crate) fn secret_findings(project: &str, hits: &[super::ci_exposure::SecretHit]) -> Vec<Finding> {
    hits.iter()
        .map(|h| {
            let what = if h.key == "-" { h.kind.describe() } else { format!("`{}`: {}", h.key, h.kind.describe()) };
            let what = match h.evidence.note() {
                Some(n) => format!("{what}; {n}"),
                None => what,
            };
            Finding {
                severity: if h.high() { Severity::High } else { Severity::Medium },
                code: "CI-SECRET",
                where_: format!("{project} → {}:{}", h.path, h.line),
                detail: if h.commented {
                    format!("{what} ({} chars), commented out — no longer used, but still in the file and in git history.", h.len)
                } else {
                    format!("{what} ({} chars) — readable by everyone with access to the repository, and kept in its history.", h.len)
                },
                fix: "Rotate it first: deleting the line does not remove it from history or from clones. Then keep it in a masked, protected CI variable (file-type if multi-line) and reference it as $NAME.".to_string(),
            }
        })
        .collect()
}

/// Local includes followed from the root CI file, at most this many.
const MAX_CI_INCLUDES: usize = 10;

/// One project's audit: its findings, and whether its CI variables could be read.
struct ProjectAudit {
    findings: Vec<Finding>,
    variables_readable: bool,
    /// (key, reason) for secret-named variables whose value is not a secret.
    skipped: Vec<(String, &'static str)>,
}

/// Run the configuration audit over one project.
///
/// A transport failure is an error, never an empty finding list: an audit that could
/// not look must not read as an audit that found nothing. Listing variables needs
/// Maintainer; a refusal there is recorded so the report can say what went unchecked.
async fn audit_project(client: &GitLabClient, path: &str) -> Result<ProjectAudit> {
    let enc = urlencoding::encode(path);
    let mut findings = Vec::new();

    let vars: Option<Vec<Value>> = match client
        .get(&format!("/projects/{enc}/variables"), &[("per_page", "100")])
        .await
    {
        Ok(v) => Some(v),
        Err(e) if e.is_user_error() => None,
        Err(e) => return Err(e),
    };
    let mut skipped = Vec::new();
    if let Some(vars) = &vars {
        findings.extend(audit_variables(path, vars));
        skipped.extend(vars.iter().filter_map(skipped_by_value));
    }

    // The CI definition is read raw, with the local files it includes; a missing
    // file simply means nothing to check.
    let mut queue = vec![".gitlab-ci.yml".to_string()];
    let mut seen = std::collections::BTreeSet::new();
    while let Some(file) = queue.pop() {
        if !seen.insert(file.clone()) || seen.len() > MAX_CI_INCLUDES + 1 {
            continue;
        }
        match client
            .get_text(
                &format!("/projects/{enc}/repository/files/{}/raw", urlencoding::encode(&file)),
                &[("ref", "HEAD")],
            )
            .await
        {
            Ok(text) => {
                findings.extend(audit_ci_file(path, &file, &text));
                findings.extend(secret_findings(path, &super::ci_exposure::find_secrets(&file, &text)));
                queue.extend(super::ci_exposure::local_includes(&text));
            }
            Err(e) if e.is_user_error() => {}
            Err(e) => return Err(e),
        }
    }
    Ok(ProjectAudit { findings, variables_readable: vars.is_some(), skipped })
}

/// Audit CI configuration for exposure, across one project or a whole group.
pub async fn audit_ci_security(
    client: &GitLabClient,
    project_id: &str,
    group_path: &str,
    max_projects: usize,
) -> Result<String> {
    // Same selection as the dependency audit: one group, several (`a,b`), or `*`
    // for every top-level group with `-name` exclusions.
    let (targets, total_repos, scope_label, _) =
        super::dependencies::resolve_targets(client, project_id, group_path, max_projects).await?;

    let mut all: Vec<Finding> = Vec::new();
    let mut unreadable_vars: Vec<&str> = Vec::new();
    let mut skipped: Vec<String> = Vec::new();
    let mut fan = crate::error::FanOut::default();
    for chunk in targets.chunks(8) {
        let futs = chunk.iter().map(|p| async move { (p.as_str(), audit_project(client, p).await) });
        for (path, result) in futures::future::join_all(futs).await {
            if let Some(audit) = fan.take(result) {
                if !audit.variables_readable {
                    unreadable_vars.push(path);
                }
                all.extend(audit.findings);
                skipped.extend(audit.skipped.into_iter().map(|(k, why)| format!("{path} → {k} ({why})")));
            }
        }
    }
    let warning = fan.finish("project audits")?.unwrap_or_default();

    let mut report = render_audit_report(&scope_label, targets.len(), total_repos, all);
    if !skipped.is_empty() {
        let _ = write!(
            report,
            "\n\nSecret-named variables not reported, by what their value is (value not shown): {}",
            skipped.join("; ")
        );
    }
    if !unreadable_vars.is_empty() {
        let _ = write!(
            report,
            "\n\n⚠️ CI variables not readable (needs Maintainer), so not audited: {}",
            unreadable_vars.join(", ")
        );
    }
    Ok(format!("{warning}{report}"))
}

/// Render the audit report from gathered findings.
///
/// Pure, so ordering, counts and the partial-coverage warning — none of which a
/// network-bound test can pin — are verified directly.
pub(crate) fn render_audit_report(
    scope_label: &str,
    audited: usize,
    total_repos: usize,
    findings: Vec<Finding>,
) -> String {
    let mut all = findings;
    all.sort_by(|a, b| (a.severity, a.code, &a.where_).cmp(&(b.severity, b.code, &b.where_)));

    let (high, med) = (
        all.iter().filter(|f| f.severity == Severity::High).count(),
        all.iter()
            .filter(|f| f.severity == Severity::Medium)
            .count(),
    );
    let mut out = vec![
        format!("# CI security audit — {scope_label}"),
        String::new(),
        format!(
            "**{} project(s) audited** of {total_repos} · **{high} HIGH · {med} MEDIUM**",
            audited
        ),
        String::new(),
    ];
    if audited < total_repos {
        out.push(format!(
            "> ⚠️ Partial: {} of {total_repos} projects audited. Raise `max_projects` for full coverage — a clean result over a subset is not a clean group.\n",
            audited
        ));
    }

    if all.is_empty() {
        out.push("No configuration-level exposure found.".to_string());
    } else {
        out.push("| Sev | Code | Where | Finding | Fix |".to_string());
        out.push("|-----|------|-------|---------|-----|".to_string());
        for f in &all {
            out.push(format!(
                "| {} | {} | {} | {} | {} |",
                f.severity, f.code, f.where_, f.detail, f.fix
            ));
        }
    }

    out.push(String::new());
    out.push(
        "_Scope: CI configuration only. This cannot tell whether a secret was actually printed — only that nothing would stop it. Job-log contents are not scanned. Variable **values are inspected to classify them, never kept or reported**._"
            .to_string(),
    );
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{Finding, Severity, audit_ci_file, audit_variables, render_audit_report};
    use serde_json::json;

    fn codes_at(f: &[Finding], code: &str) -> Vec<String> {
        f.iter()
            .filter(|x| x.code == code)
            .map(|x| x.where_.clone())
            .collect()
    }

    #[test]
    fn floating_detection_names_the_exact_lines_not_just_a_count() {
        // A count-only assertion let a mutant through: flagging the pinned image
        // instead of the untagged one keeps the total at two.
        let ci = "\
stages: [build]
job:
  image: registry.example.com/build/runtime:latest
  image: node:20.11.1
  image: registry.example.com/base
  image: registry.example.com/base@sha256:abc123
  image: $CI_REGISTRY_IMAGE
  image: \"\"
";
        let f = audit_ci_file("g/p", ".gitlab-ci.yml", ci);
        assert_eq!(
            codes_at(&f, "IMG-FLOATING"),
            vec!["g/p → .gitlab-ci.yml:3", "g/p → .gitlab-ci.yml:5"],
            "line 3 (:latest) and line 5 (untagged) — never the pinned or digest ones"
        );
        assert!(f[0].detail.contains("runtime:latest"));
    }

    #[test]
    fn every_pipe_to_shell_spelling_is_caught_individually() {
        for (i, line) in [
            "curl -fsSL https://example.com/i.sh | sh",
            "curl -fsSL https://example.com/i.sh |sh",
            "wget -qO- https://example.com/i.sh | bash",
            "wget -qO- https://example.com/i.sh |bash",
        ]
        .iter()
        .enumerate()
        {
            let f = audit_ci_file("g/p", "ci.yml", line);
            assert_eq!(codes_at(&f, "FETCH-UNPINNED").len(), 1, "case {i}: {line}");
            assert!(f[0].detail.contains("Pipes a remote download"), "case {i}");
        }
        // Downloading without piping into a shell, or piping something else, is fine.
        assert!(
            audit_ci_file(
                "g/p",
                "ci.yml",
                "curl -o tool https://example.com/v2.3.1/tool"
            )
            .is_empty()
        );
        assert!(
            audit_ci_file("g/p", "ci.yml", "cat notes | sh").is_empty(),
            "no download involved"
        );
        // The latest-release form carries its own explanation.
        let rel = audit_ci_file(
            "g/p",
            "ci.yml",
            "curl -L https://example.com/releases/latest/download/x -o x",
        );
        assert!(rel[0].detail.contains("latest"));
    }

    fn finding(sev: Severity, code: &'static str, at: &str) -> Finding {
        Finding {
            severity: sev,
            code,
            where_: at.into(),
            detail: "d".into(),
            fix: "f".into(),
        }
    }

    #[test]
    fn the_report_orders_by_severity_and_counts_each_class() {
        let out = render_audit_report(
            "group `g`",
            3,
            3,
            vec![
                finding(Severity::Medium, "IMG-FLOATING", "g/b"),
                finding(Severity::High, "VAR-UNMASKED", "g/c"),
                finding(Severity::Medium, "IMG-FLOATING", "g/a"),
                finding(Severity::High, "FETCH-UNPINNED", "g/a"),
            ],
        );
        assert!(out.starts_with("# CI security audit — group `g`"));
        assert!(out.contains("**3 project(s) audited** of 3 · **2 HIGH · 2 MEDIUM**"));
        let rows: Vec<&str> = out
            .lines()
            .filter(|l| l.starts_with("| HIGH") || l.starts_with("| MEDIUM"))
            .collect();
        // HIGH before MEDIUM; within a severity, by code, then location.
        assert_eq!(rows.len(), 4);
        assert!(rows[0].starts_with("| HIGH | FETCH-UNPINNED | g/a"));
        assert!(rows[1].starts_with("| HIGH | VAR-UNMASKED | g/c"));
        assert!(rows[2].starts_with("| MEDIUM | IMG-FLOATING | g/a"));
        assert!(rows[3].starts_with("| MEDIUM | IMG-FLOATING | g/b"));
        assert!(!out.contains("Partial"), "full coverage must not warn");
        assert!(out.contains("never kept or reported"));
    }

    #[test]
    fn a_partial_sweep_warns_and_a_complete_one_does_not() {
        // The boundary is exact: auditing all of them is not partial.
        let warns =
            |audited, total| render_audit_report("g", audited, total, vec![]).contains("Partial");
        assert!(warns(59, 60));
        assert!(!warns(60, 60));
        assert!(!warns(1, 1));
        let clean = render_audit_report("project `p`", 1, 1, vec![]);
        assert!(clean.contains("No configuration-level exposure found."));
        assert!(clean.contains("**0 HIGH · 0 MEDIUM**"));
    }

    #[test]
    fn an_unmaskable_key_is_told_to_use_file_type_not_masking() {
        // The trap: "just tick masked" fails with 400 on a PEM, and the reader
        // concludes the finding is bogus.
        let vars = vec![json!({
            "key": "DEPLOY_SSH_PRIV_KEY", "masked": false,
            "variable_type": "env_var", "environment_scope": "prod"
        })];
        let f = audit_variables("group/app", &vars);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].severity, Severity::High);
        assert!(f[0].fix.contains("variable_type=file"), "{}", f[0].fix);
        assert!(!f[0].fix.starts_with("Enable masking"), "{}", f[0].fix);
        // The value is never echoed, because it is never read.
        assert!(!f[0].detail.contains("BEGIN"));
    }

    #[test]
    fn an_ordinary_secret_is_told_to_mask_and_a_safe_one_is_left_alone() {
        let vars = vec![
            json!({"key":"DB_PASSWORD","masked":false,"variable_type":"env_var","environment_scope":"*"}),
            json!({"key":"DB_PASSWORD","masked":true,"variable_type":"env_var","environment_scope":"*"}),
            json!({"key":"SSH_PRIV_KEY","masked":false,"variable_type":"file","environment_scope":"*"}),
            json!({"key":"APP_ENV","masked":false,"variable_type":"env_var","environment_scope":"*"}),
        ];
        let f = audit_variables("group/app", &vars);
        assert_eq!(f.len(), 1, "only the unmasked env_var secret: {f:?}");
        assert_eq!(f[0].severity, Severity::Medium);
        assert!(f[0].fix.starts_with("Enable masking"));
    }

    #[test]
    fn the_value_decides_and_is_never_echoed() {
        // Values are assembled at run time: no credential-shaped literal in the tree.
        let token: String = "t0k".repeat(8);
        let pem = format!("-----BEGIN {}-----\nAAAA\n-----END {}-----", ["OPENSSH", "PRIVATE", "KEY"].join(" "), ["OPENSSH", "PRIVATE", "KEY"].join(" "));
        let var = |key: &str, value: &str, protected: bool| json!({"key": key, "value": value, "masked": false, "protected": protected, "variable_type": "env_var", "environment_scope": "*"});
        let vars = vec![
            var("DNS_API_TOKEN", &token, false),
            var("TLS_SERVER_KEY", &pem, true),
            var("DOCKER_AUTH_CONFIG", &format!("{{\"auths\":{{\"r\":{{\"auth\":\"{token}\"}}}}}}"), true),
            var("STORAGE_PRIVATE_BUCKET", "backups", false),
            var("DEPLOY_PUB_KEY", "ssh-ed25519 AAAA", false),
            var("AUTHORIZED_KEYS_LIST", "[\"ssh-ed25519 AAAA a@b\", \"ssh-rsa AAAA c@d\"]", false),
            var("DEPLOY_TOKEN", "$CI_JOB_TOKEN", false),
            var("FEATURE_PASSWORD_LOGIN", "true", false),
        ];
        let f = audit_variables("group/app", &vars);
        let got: Vec<(&str, Severity)> = f.iter().map(|x| (x.where_.split(" → ").nth(1).unwrap(), x.severity)).collect();
        assert_eq!(
            got,
            vec![("DNS_API_TOKEN (scope=*)", Severity::Medium), ("TLS_SERVER_KEY (scope=*)", Severity::High), ("DOCKER_AUTH_CONFIG (scope=*)", Severity::Medium)]
        );
        assert!(f[0].detail.contains("Not protected") && !f[1].detail.contains("Not protected"));
        assert!(f[2].detail.contains("cannot mask"));
        let all = format!("{f:?}");
        assert!(!all.contains(&token) && !all.contains("AAAA"), "no value in a finding");
        let why: Vec<_> = vars.iter().filter_map(super::skipped_by_value).collect();
        assert!(why.contains(&("AUTHORIZED_KEYS_LIST".to_string(), "public keys or certificates only")));
        assert!(why.contains(&("DEPLOY_TOKEN".to_string(), "reference to another variable")));
        assert!(why.contains(&("STORAGE_PRIVATE_BUCKET".to_string(), "name describes, does not hold, a secret")));
        let named = |k: &str| super::unmasked_secret(&var(k, &token, false)).is_some();
        assert!(!named("REGISTRY_TOKEN_USER") && !named("API_TOKEN_TTL") && !named("SIGNING_KEY_PATH"));
        assert!(named("DB_USER_PASSWORD") && named("PIPELINE_TRIGGER_TOKEN"));
        let h = super::unmasked_secret(&vars[0]).unwrap();
        assert_eq!((h.len, h.fingerprint), (24, super::super::ci_exposure::fingerprint(&token)));
    }

    #[test]
    fn ci_debug_trace_counts_only_when_actually_on() {
        let on = vec![
            json!({"key":"CI_DEBUG_TRACE","value":"true","masked":false,"variable_type":"env_var","environment_scope":"*"}),
        ];
        let off = vec![
            json!({"key":"CI_DEBUG_TRACE","value":"false","masked":false,"variable_type":"env_var","environment_scope":"*"}),
        ];
        assert_eq!(audit_variables("p", &on).len(), 1);
        assert_eq!(audit_variables("p", &on)[0].code, "CI-DEBUG");
        assert!(
            audit_variables("p", &off).is_empty(),
            "false must not be a finding"
        );
    }

    #[test]
    fn floating_image_tags_are_caught_and_pinned_ones_are_not() {
        let ci = "\
image: registry.example.com/build/runtime:latest
  image: node:20.11.1
  image: registry.example.com/base
  image: registry.example.com/base@sha256:abc123
  image: $CI_REGISTRY_IMAGE
";
        let f = audit_ci_file("g/p", ".gitlab-ci.yml", ci);
        let imgs: Vec<&String> = f
            .iter()
            .filter(|x| x.code == "IMG-FLOATING")
            .map(|x| &x.where_)
            .collect();
        assert_eq!(imgs.len(), 2, "latest + untagged only: {f:?}");
        // A variable image cannot be judged statically, so it is not guessed at.
        assert!(!format!("{f:?}").contains("CI_REGISTRY_IMAGE"));
    }

    #[test]
    fn unpinned_remote_execution_is_high() {
        let ci = "\
  - curl -fsSL https://example.com/releases/latest/download/tool_linux_amd64 -o tool
  - curl -sSf https://example.com/install.sh | sh
  - wget -qO- https://example.com/x.sh |bash
  - curl -fsSL https://example.com/download/v2.3.1/tool -o tool
# - curl https://example.com/install.sh | sh
";
        let f = audit_ci_file("g/p", ".gitlab-ci.yml", ci);
        let fetch: Vec<_> = f.iter().filter(|x| x.code == "FETCH-UNPINNED").collect();
        assert_eq!(fetch.len(), 3, "pinned version and comment excluded: {f:?}");
        assert!(fetch.iter().all(|x| x.severity == Severity::High));
    }
}
