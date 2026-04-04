//! Rule-based commit validation — zero LLM tokens.
//!
//! Loads TOML rules from rules/ directory, matches regex patterns against
//! commit diffs, returns only violations.

use crate::client::GitLabClient;
use crate::error::Result;
use crate::tools::commits::detect_language;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::sync::LazyLock;

// ─── Rule types ───

#[derive(Debug, Deserialize, Clone)]
pub struct RuleFile {
    pub rule: Vec<Rule>,
}

/// Max violations per rule per file before collapsing.
const MAX_VIOLATIONS_PER_RULE_PER_FILE: usize = 3;

/// Files to always skip during linting (data files, generated, binary-like).
const SKIP_FILE_EXTENSIONS: &[&str] = &[
    ".list", ".csv", ".tsv", ".dat", ".log",
    ".lock", ".sum", ".map",
    ".min.js", ".min.css",
    ".png", ".jpg", ".gif", ".ico", ".svg", ".woff", ".woff2", ".ttf",
    ".zip", ".tar", ".gz",
];

const SKIP_FILE_PATTERNS: &[&str] = &[
    "vendor/", "node_modules/", "dist/", "build/",
    "__generated__", ".pb.go",
    "package-lock.json", "yarn.lock", "composer.lock",
    "go.sum", "Cargo.lock",
];

fn should_skip_lint_file(path: &str) -> bool {
    SKIP_FILE_EXTENSIONS.iter().any(|ext| path.ends_with(ext))
        || SKIP_FILE_PATTERNS.iter().any(|pat| path.contains(pat))
}

#[derive(Debug, Deserialize, Clone)]
pub struct Rule {
    pub id: String,
    pub severity: String,
    pub name: String,
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub negative_pattern: String,
    #[serde(default)]
    pub negative_file_pattern: String,
    #[serde(default)]
    pub applies_to: String,
    #[serde(default)]
    pub max_additions: u64,
    pub message: String,
}

#[derive(Debug)]
pub struct Violation {
    pub rule_id: String,
    pub severity: String,
    pub name: String,
    pub file: String,
    pub line: usize,
    pub code: String,
    pub message: String,
}

// ─── Compiled rule with pre-built regexes ───

struct CompiledRule {
    rule: Rule,
    pattern_re: Option<regex::Regex>,
    neg_pattern_re: Option<regex::Regex>,
    neg_file_re: Option<regex::Regex>,
}

impl CompiledRule {
    fn compile(rule: Rule) -> Self {
        let pattern_re = if rule.pattern.is_empty() {
            None
        } else {
            regex::Regex::new(&rule.pattern).ok()
        };
        let neg_pattern_re = if rule.negative_pattern.is_empty() {
            None
        } else {
            regex::Regex::new(&rule.negative_pattern).ok()
        };
        let neg_file_re = if rule.negative_file_pattern.is_empty() {
            None
        } else {
            regex::Regex::new(&rule.negative_file_pattern).ok()
        };
        Self { rule, pattern_re, neg_pattern_re, neg_file_re }
    }
}

// ─── Rule loading (cached, parsed once) ───

fn parse_embedded_rules(content: &str) -> Vec<Rule> {
    toml::from_str::<RuleFile>(content)
        .map(|rf| rf.rule)
        .unwrap_or_default()
}

/// Pre-compiled rules per language, parsed and compiled once at first use.
static COMPILED_RULES: LazyLock<BTreeMap<&'static str, Vec<CompiledRule>>> = LazyLock::new(|| {
    let global = parse_embedded_rules(include_str!("../../rules/global.toml"));

    let lang_sources: &[(&str, &str)] = &[
        ("PHP", include_str!("../../rules/php.toml")),
        ("Kotlin", include_str!("../../rules/kotlin.toml")),
        ("Swift", include_str!("../../rules/swift.toml")),
        ("Go", include_str!("../../rules/go.toml")),
        ("TypeScript", include_str!("../../rules/typescript.toml")),
        ("YAML/Ansible", include_str!("../../rules/ansible.toml")),
    ];

    let mut map: BTreeMap<&str, Vec<CompiledRule>> = BTreeMap::new();

    // Global-only entry
    map.insert("global", global.iter().cloned().map(CompiledRule::compile).collect());

    for &(lang, content) in lang_sources {
        let mut rules = global.clone();
        rules.extend(parse_embedded_rules(content));
        map.insert(lang, rules.into_iter().map(CompiledRule::compile).collect());
    }

    // Aliases
    let alias_map: &[(&str, &str)] = &[
        ("Java", "Kotlin"),
        ("JavaScript", "TypeScript"),
        ("Vue", "TypeScript"),
        ("Jinja2/Ansible", "YAML/Ansible"),
        ("Ansible/Inventory", "YAML/Ansible"),
        ("Shell", "YAML/Ansible"),
    ];
    for &(alias, target) in alias_map {
        if let Some(rules) = map.get(target) {
            let cloned: Vec<CompiledRule> = rules.iter().map(|cr| CompiledRule::compile(cr.rule.clone())).collect();
            map.insert(alias, cloned);
        }
    }

    map
});

fn get_compiled_rules(lang: &str) -> &'static [CompiledRule] {
    COMPILED_RULES
        .get(lang)
        .map(|v| v.as_slice())
        .unwrap_or_else(|| COMPILED_RULES.get("global").map(|v| v.as_slice()).unwrap_or(&[]))
}

/// Load raw rules for display purposes (list_rules).
fn load_rules_for_language(lang: &str) -> Vec<Rule> {
    get_compiled_rules(lang).iter().map(|cr| cr.rule.clone()).collect()
}

// ─── Pattern matching ───

fn matches_compiled_rule(cr: &CompiledRule, line: &str, file_path: &str) -> bool {
    // Must have a pattern
    let pattern_re = match &cr.pattern_re {
        Some(re) => re,
        None => return false,
    };

    // Skip if file matches negative_file_pattern
    if let Some(ref re) = cr.neg_file_re {
        if re.is_match(file_path) {
            return false;
        }
    }

    // Check main pattern
    if !pattern_re.is_match(line) {
        return false;
    }

    // Check negative pattern (should NOT match)
    if let Some(ref re) = cr.neg_pattern_re {
        if re.is_match(line) {
            return false;
        }
    }

    true
}

// ─── Validation tools ───

/// Validate a single commit against rules. Returns only violations.
pub async fn validate_commit(
    client: &GitLabClient,
    project_id: &str,
    sha: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    // Fetch commit metadata
    let commit: Value = client
        .get(&format!("/projects/{encoded}/repository/commits/{sha}"), &[])
        .await?;

    let message = commit["message"].as_str().unwrap_or("");
    let author = commit["author_name"].as_str().unwrap_or("?");
    let short_sha = commit["short_id"].as_str().unwrap_or(&sha[..8.min(sha.len())]);

    // Fetch diffs
    let diffs: Vec<Value> = client
        .get(&format!("/projects/{encoded}/repository/commits/{sha}/diff"), &[])
        .await?;

    let mut violations: Vec<Violation> = Vec::new();

    // Check commit message rules
    let global_rules = get_compiled_rules("global");
    for cr in global_rules {
        if cr.rule.applies_to == "commit_message" && matches_compiled_rule(cr, message.trim(), "") {
            violations.push(Violation {
                rule_id: cr.rule.id.clone(),
                severity: cr.rule.severity.clone(),
                name: cr.rule.name.clone(),
                file: "(commit message)".into(),
                line: 0,
                code: message.trim().to_string(),
                message: cr.rule.message.clone(),
            });
        }
    }

    // Check each diff file
    // Track per-rule-per-file violation counts for capping
    let mut rule_file_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut suppressed: BTreeMap<(String, String), usize> = BTreeMap::new();

    for diff in &diffs {
        let file_path = diff["new_path"].as_str().unwrap_or("?");
        let diff_text = diff["diff"].as_str().unwrap_or("");

        // Skip data files, generated code, binary-like files
        if should_skip_lint_file(file_path) {
            continue;
        }

        let lang = detect_language(file_path);
        let compiled_rules = get_compiled_rules(lang);

        // Count additions for file_stats rules
        let mut additions: u64 = 0;

        for (i, line) in diff_text.lines().enumerate() {
            // Only check added lines
            if !line.starts_with('+') || line.starts_with("+++") {
                if line.starts_with('+') { additions += 1; }
                continue;
            }
            additions += 1;
            let clean_line = &line[1..]; // strip leading +

            for cr in compiled_rules {
                if cr.rule.applies_to.is_empty() || cr.rule.applies_to == "line" {
                    if matches_compiled_rule(cr, clean_line, file_path) {
                        let key = (cr.rule.id.clone(), file_path.to_string());
                        let count = rule_file_counts.entry(key.clone()).or_insert(0);
                        *count += 1;

                        if *count <= MAX_VIOLATIONS_PER_RULE_PER_FILE {
                            violations.push(Violation {
                                rule_id: cr.rule.id.clone(),
                                severity: cr.rule.severity.clone(),
                                name: cr.rule.name.clone(),
                                file: file_path.to_string(),
                                line: i + 1,
                                code: clean_line.chars().take(120).collect(),
                                message: cr.rule.message.clone(),
                            });
                        } else {
                            *suppressed.entry(key).or_insert(0) += 1;
                        }
                    }
                }
            }

            // EOF check
            if compiled_rules.iter().any(|cr| cr.rule.applies_to == "file_end")
                && line.contains("No newline at end of file")
            {
                violations.push(Violation {
                    rule_id: "PHP012".into(),
                    severity: "info".into(),
                    name: "No newline at EOF".into(),
                    file: file_path.to_string(),
                    line: i + 1,
                    code: String::new(),
                    message: "Missing newline at end of file".into(),
                });
            }
        }

        // File stats rules (e.g., large file)
        for cr in compiled_rules {
            if cr.rule.applies_to == "file_stats" && cr.rule.max_additions > 0 && additions > cr.rule.max_additions {
                violations.push(Violation {
                    rule_id: cr.rule.id.clone(),
                    severity: cr.rule.severity.clone(),
                    name: cr.rule.name.clone(),
                    file: file_path.to_string(),
                    line: 0,
                    code: format!("+{additions} lines"),
                    message: cr.rule.message.clone(),
                });
            }
        }
    }

    // Format output
    if violations.is_empty() {
        return Ok(format!(
            "**{project_id} `{short_sha}`** by {author} — **No violations** ({} files checked)",
            diffs.len()
        ));
    }

    // Group by severity
    let mut by_severity: BTreeMap<String, Vec<&Violation>> = BTreeMap::new();
    for v in &violations {
        by_severity.entry(v.severity.clone()).or_default().push(v);
    }

    let total_suppressed: usize = suppressed.values().sum();
    let total_shown = violations.len();

    let severity_order = ["critical", "warning", "info"];
    let mut lines = vec![
        format!(
            "**{project_id} `{short_sha}`** by {author} — **{} violations** ({} files)",
            total_shown + total_suppressed,
            diffs.len()
        ),
        String::new(),
    ];

    for sev in &severity_order {
        if let Some(sevs) = by_severity.get(*sev) {
            let icon = match *sev {
                "critical" => "🔴",
                "warning" => "🟡",
                "info" => "🔵",
                _ => "⚪",
            };
            lines.push(format!("### {} {} ({})", icon, sev.to_uppercase(), sevs.len()));
            for v in sevs {
                let loc = if v.line > 0 {
                    format!("{}:{}", v.file, v.line)
                } else {
                    v.file.clone()
                };
                let code_preview = if v.code.is_empty() {
                    String::new()
                } else {
                    format!("\n  `{}`", v.code)
                };
                lines.push(format!(
                    "- **[{}]** {} — {}{}\n  {loc}",
                    v.rule_id, v.name, v.message, code_preview
                ));
            }
            lines.push(String::new());
        }
    }

    // Show suppressed violations summary
    if total_suppressed > 0 {
        lines.push(format!("*{total_suppressed} more violations suppressed (max {MAX_VIOLATIONS_PER_RULE_PER_FILE} per rule per file):*"));
        for ((rule_id, file), count) in &suppressed {
            let short_file = file.rsplit('/').next().unwrap_or(file);
            lines.push(format!("- [{rule_id}] {short_file}: +{count} more"));
        }
    }

    Ok(lines.join("\n"))
}

/// Validate all commits in an MR.
pub async fn validate_mr(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    // Get MR commits
    let commits: Vec<Value> = client
        .get(
            &format!("/projects/{encoded}/merge_requests/{mr_iid}/commits"),
            &[("per_page", "50")],
        )
        .await?;

    if commits.is_empty() {
        return Ok(format!("No commits in MR !{mr_iid}."));
    }

    let mut all_output = vec![format!(
        "## Validation: {project_id} !{mr_iid} ({} commits)\n",
        commits.len()
    )];

    let mut total_violations = 0u64;
    let mut total_critical = 0u64;

    for commit in &commits {
        let sha = commit["id"].as_str().unwrap_or("?");
        let result = validate_commit(client, project_id, sha).await?;

        // Count violations from result
        if result.contains("No violations") {
            // Skip clean commits in MR report
            continue;
        }

        total_violations += 1;
        if result.contains("CRITICAL") {
            total_critical += 1;
        }

        all_output.push(result);
        all_output.push("---".into());
    }

    if total_violations == 0 {
        return Ok(format!(
            "## {project_id} !{mr_iid} — **All clean** ({} commits, 0 violations)",
            commits.len()
        ));
    }

    // Summary at top
    let summary = format!(
        "**Summary:** {} commits with violations ({} critical)\n",
        total_violations, total_critical
    );
    all_output.insert(1, summary);

    Ok(all_output.join("\n"))
}

/// List all available rules, optionally filtered by language.
pub fn list_rules(language: &str) -> String {
    let rules = if language.is_empty() {
        // All rules
        let mut all = Vec::new();
        for lang in &["global", "PHP", "Kotlin", "Swift", "Go", "TypeScript", "YAML/Ansible", "Ansible/Inventory"] {
            all.extend(load_rules_for_language(lang));
        }
        // Dedup by ID
        let mut seen = std::collections::HashSet::new();
        all.retain(|r| seen.insert(r.id.clone()));
        all
    } else {
        load_rules_for_language(language)
    };

    if rules.is_empty() {
        return format!("No rules found for '{language}'.");
    }

    let mut lines = vec![format!("**Rules: {}**\n", rules.len())];

    let mut by_severity: BTreeMap<String, Vec<&Rule>> = BTreeMap::new();
    for r in &rules {
        by_severity.entry(r.severity.clone()).or_default().push(r);
    }

    for sev in &["critical", "warning", "info"] {
        if let Some(sevs) = by_severity.get(*sev) {
            let icon = match *sev {
                "critical" => "🔴",
                "warning" => "🟡",
                "info" => "🔵",
                _ => "⚪",
            };
            lines.push(format!("### {} {} ({})", icon, sev.to_uppercase(), sevs.len()));
            for r in sevs {
                lines.push(format!("- **[{}]** {} — {}", r.id, r.name, r.message));
            }
            lines.push(String::new());
        }
    }

    lines.join("\n")
}
