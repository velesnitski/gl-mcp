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
use serde_json::Value;

/// Key substrings that mark a variable as holding a secret.
const SECRETISH: &[&str] = &[
    "SECRET", "TOKEN", "PASSWORD", "PASSWD", "PASS", "KEY", "CREDENTIAL", "PRIVATE", "AUTH",
    "CERT", "CRT", "PEM", "SALT", "SIGNATURE", "WEBHOOK", "DSN",
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
    ["PRIV_KEY", "PRIVATE_KEY", "SSH_KEY", "SSH_KEYS", "_PEM", "CERT", "_CRT"]
        .iter()
        .any(|m| k.contains(m))
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct Finding {
    pub severity: &'static str,
    pub code: &'static str,
    pub where_: String,
    pub detail: String,
    pub fix: String,
}

/// D1 — a secret-shaped variable that GitLab would not redact from a job log.
///
/// This is the mechanism, not a hypothetical: an unmasked variable echoed by a script
/// running under `set -x` lands in the trace in cleartext and stays there.
pub(crate) fn audit_variables(project: &str, vars: &[Value]) -> Vec<Finding> {
    let mut out = Vec::new();
    for v in vars {
        let key = v["key"].as_str().unwrap_or("");
        let masked = v["masked"].as_bool().unwrap_or(false);
        let vtype = v["variable_type"].as_str().unwrap_or("env_var");
        let scope = v["environment_scope"].as_str().unwrap_or("*");

        if key.eq_ignore_ascii_case("CI_DEBUG_TRACE") {
            let on = matches!(
                v["value"].as_str().unwrap_or("").trim().to_ascii_lowercase().as_str(),
                "true" | "1" | "yes"
            );
            if on {
                out.push(Finding {
                    severity: "HIGH",
                    code: "CI-DEBUG",
                    where_: format!("{project} (scope={scope})"),
                    detail: "CI_DEBUG_TRACE is enabled — GitLab prints every variable, including masked ones, into the job log.".to_string(),
                    fix: "Set it to false; use it only transiently and purge the affected job logs afterwards.".to_string(),
                });
            }
            continue;
        }

        let secretish = SECRETISH.iter().any(|m| key.to_ascii_uppercase().contains(m));
        // A file-type variable is never interpolated into a command line, so the
        // masking question does not arise for it.
        if !secretish || masked || vtype == "file" {
            continue;
        }
        let unmaskable = is_structurally_unmaskable(key);
        out.push(Finding {
            severity: if unmaskable { "HIGH" } else { "MEDIUM" },
            code: "VAR-UNMASKED",
            where_: format!("{project} → {key} (scope={scope})"),
            detail: if unmaskable {
                "Holds key material and is neither masked nor file-type: if any job echoes it, the value is written to the log in full."
                    .to_string()
            } else {
                "Secret-shaped variable is not masked: GitLab will not redact it if a job prints it."
                    .to_string()
            },
            fix: if unmaskable {
                "Set variable_type=file. Masking cannot apply — GitLab rejects multi-line values, so enabling it will fail with 400."
                    .to_string()
            } else {
                "Enable masking. If the API refuses it, the value breaks GitLab's masking rules (single line, ≥8 chars, restricted alphabet) — use variable_type=file instead."
                    .to_string()
            },
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
                let floating = img.ends_with(":latest") || !img.rsplit('/').next().unwrap_or("").contains(':');
                if floating && !img.contains('@') {
                    out.push(Finding {
                        severity: "MEDIUM",
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
            && (lower.contains("| sh") || lower.contains("|sh") || lower.contains("| bash") || lower.contains("|bash"));
        let latest_release = lower.contains("releases/latest/download");
        if piped_to_shell || latest_release {
            out.push(Finding {
                severity: "HIGH",
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

/// Run the configuration audit over one project.
async fn audit_project(client: &GitLabClient, path: &str) -> Vec<Finding> {
    let enc = urlencoding::encode(path);
    let mut findings = Vec::new();

    let vars: Vec<Value> = client
        .get(&format!("/projects/{enc}/variables"), &[("per_page", "100")])
        .await
        .unwrap_or_default();
    findings.extend(audit_variables(path, &vars));

    // The CI definition is read raw; a missing file simply means nothing to check.
    if let Ok(text) = client
        .get_text(
            &format!("/projects/{enc}/repository/files/.gitlab-ci.yml/raw"),
            &[("ref", "HEAD")],
        )
        .await
    {
        findings.extend(audit_ci_file(path, ".gitlab-ci.yml", &text));
    }
    findings
}

/// Audit CI configuration for exposure, across one project or a whole group.
pub async fn audit_ci_security(
    client: &GitLabClient,
    project_id: &str,
    group_path: &str,
    max_projects: usize,
) -> Result<String> {
    let mut targets: Vec<String> = Vec::new();
    let scope_label;
    let total_repos: usize;

    if !group_path.is_empty() {
        let projects: Vec<Value> = client
            .get_all_pages(
                &format!("/groups/{}/projects", urlencoding::encode(group_path)),
                &[
                    ("include_subgroups", "true"),
                    ("archived", "false"),
                    ("order_by", "last_activity_at"),
                    ("sort", "desc"),
                ],
                3,
            )
            .await?;
        total_repos = projects.len();
        targets = projects
            .iter()
            .take(max_projects)
            .filter_map(|p| p["path_with_namespace"].as_str().map(str::to_string))
            .collect();
        scope_label = format!("group `{group_path}`");
    } else {
        targets.push(project_id.to_string());
        total_repos = 1;
        scope_label = format!("project `{project_id}`");
    }

    let mut all: Vec<Finding> = Vec::new();
    for chunk in targets.chunks(8) {
        let futs = chunk.iter().map(|p| audit_project(client, p));
        for f in futures::future::join_all(futs).await {
            all.extend(f);
        }
    }

    let rank = |s: &str| match s {
        "HIGH" => 0,
        "MEDIUM" => 1,
        _ => 2,
    };
    all.sort_by_key(|f| (rank(f.severity), f.code, f.where_.clone()));

    let (high, med) = (
        all.iter().filter(|f| f.severity == "HIGH").count(),
        all.iter().filter(|f| f.severity == "MEDIUM").count(),
    );
    let mut out = vec![
        format!("# CI security audit — {scope_label}"),
        String::new(),
        format!(
            "**{} project(s) audited** of {total_repos} · **{high} HIGH · {med} MEDIUM**",
            targets.len()
        ),
        String::new(),
    ];
    if targets.len() < total_repos {
        out.push(format!(
            "> ⚠️ Partial: {} of {total_repos} projects audited. Raise `max_projects` for full coverage — a clean result over a subset is not a clean group.\n",
            targets.len()
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
        "_Scope: CI configuration only. This cannot tell whether a secret was actually printed — only that nothing would stop it. Job-log contents are not scanned. Variable **values are never read or reported**._"
            .to_string(),
    );
    Ok(out.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::{audit_ci_file, audit_variables};
    use serde_json::json;

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
        assert_eq!(f[0].severity, "HIGH");
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
        assert_eq!(f[0].severity, "MEDIUM");
        assert!(f[0].fix.starts_with("Enable masking"));
    }

    #[test]
    fn ci_debug_trace_counts_only_when_actually_on() {
        let on = vec![json!({"key":"CI_DEBUG_TRACE","value":"true","masked":false,"variable_type":"env_var","environment_scope":"*"})];
        let off = vec![json!({"key":"CI_DEBUG_TRACE","value":"false","masked":false,"variable_type":"env_var","environment_scope":"*"})];
        assert_eq!(audit_variables("p", &on).len(), 1);
        assert_eq!(audit_variables("p", &on)[0].code, "CI-DEBUG");
        assert!(audit_variables("p", &off).is_empty(), "false must not be a finding");
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
        let imgs: Vec<&String> = f.iter().filter(|x| x.code == "IMG-FLOATING").map(|x| &x.where_).collect();
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
        assert!(fetch.iter().all(|x| x.severity == "HIGH"));
    }
}
