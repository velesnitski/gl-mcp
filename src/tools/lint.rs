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

pub(crate) fn should_skip_lint_file(path: &str) -> bool {
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

/// Validate MR using the unified changes diff (not individual commits).
/// This catches issues in squashed MRs where commit diffs are minimal.
pub async fn validate_mr_changes(
    client: &GitLabClient,
    project_id: &str,
    mr_iid: u64,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);

    // Fetch MR metadata
    let mr: Value = client
        .get(&format!("/projects/{encoded}/merge_requests/{mr_iid}"), &[])
        .await?;
    let title = mr["title"].as_str().unwrap_or("?");
    let author = mr["author"]["username"].as_str().unwrap_or("?");

    // Fetch unified changes (full diff, not per-commit)
    let mr_detail: Value = client
        .get(
            &format!("/projects/{encoded}/merge_requests/{mr_iid}/changes"),
            &[("access_raw_diffs", "true")],
        )
        .await?;

    let changes = mr_detail["changes"].as_array();
    let diffs = match changes {
        Some(c) if !c.is_empty() => c,
        _ => return Ok(format!(
            "**{project_id} !{mr_iid}** by @{author} — **No changes found**"
        )),
    };

    let mut violations: Vec<Violation> = Vec::new();
    let mut rule_file_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut suppressed: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut files_checked = 0usize;

    for diff in diffs {
        let file_path = diff["new_path"].as_str().unwrap_or("?");
        let diff_text = diff["diff"].as_str().unwrap_or("");

        if should_skip_lint_file(file_path) || diff_text.is_empty() {
            continue;
        }
        files_checked += 1;

        let lang = detect_language(file_path);
        let compiled_rules = get_compiled_rules(lang);

        let mut additions: u64 = 0;

        for (i, line) in diff_text.lines().enumerate() {
            if !line.starts_with('+') || line.starts_with("+++") {
                if line.starts_with('+') { additions += 1; }
                continue;
            }
            additions += 1;
            let clean_line = &line[1..];

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
        }

        // File stats rules
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

    if violations.is_empty() {
        return Ok(format!(
            "**{project_id} !{mr_iid}** \"{}\" by @{author} — **No violations** ({files_checked} files checked)",
            title
        ));
    }

    let mut by_severity: BTreeMap<String, Vec<&Violation>> = BTreeMap::new();
    for v in &violations {
        by_severity.entry(v.severity.clone()).or_default().push(v);
    }

    let total_suppressed: usize = suppressed.values().sum();
    let total_shown = violations.len();

    let severity_order = ["critical", "warning", "info"];
    let mut lines = vec![
        format!(
            "**{project_id} !{mr_iid}** \"{}\" by @{author} — **{} violations** ({files_checked} files)",
            title, total_shown + total_suppressed
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

    if total_suppressed > 0 {
        lines.push(format!("*{total_suppressed} more violations suppressed (max {MAX_VIOLATIONS_PER_RULE_PER_FILE} per rule per file):*"));
        for ((rule_id, file), count) in &suppressed {
            let short_file = file.rsplit('/').next().unwrap_or(file);
            lines.push(format!("- [{rule_id}] {short_file}: +{count} more"));
        }
    }

    Ok(lines.join("\n"))
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

/// Analyze a file's code quality metrics: length, functions, nesting depth, complexity indicators.
/// Fetches the full file content (not diff) for structural analysis.
pub async fn analyze_file(
    client: &GitLabClient,
    project_id: &str,
    file_path: &str,
    ref_name: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let encoded_path = urlencoding::encode(file_path);
    let ref_param = if ref_name.is_empty() { "HEAD" } else { ref_name };

    let file_info: Value = client
        .get(
            &format!("/projects/{encoded}/repository/files/{encoded_path}"),
            &[("ref", ref_param)],
        )
        .await?;

    let content_b64 = file_info["content"].as_str().unwrap_or("");
    let content = base64_decode(content_b64);

    let lang = detect_language(file_path);
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return Ok(format!("**{file_path}** — empty file"));
    }

    // ─── Metrics ───
    let blank_lines = lines.iter().filter(|l| l.trim().is_empty()).count();
    let comment_lines = lines.iter().filter(|l| {
        let t = l.trim();
        t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*')
    }).count();
    let code_lines = total_lines - blank_lines - comment_lines;

    // Function detection
    let func_pattern = match lang {
        "Swift" => r"(?:func|init)\s+",
        "PHP" => r"function\s+\w+",
        "Go" => r"func\s+",
        "Kotlin" | "Java" => r"fun\s+",
        "TypeScript" | "JavaScript" => r"(?:function\s+|(?:const|let|var)\s+\w+\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_]\w*)\s*=>)",
        "Rust" => r"fn\s+",
        "Python" => r"def\s+",
        _ => r"function\s+|func\s+|fn\s+|def\s+",
    };
    let func_re = regex::Regex::new(func_pattern).ok();
    let mut functions: Vec<(usize, String)> = Vec::new();
    if let Some(re) = &func_re {
        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                let name = line.trim().chars().take(80).collect::<String>();
                functions.push((i + 1, name));
            }
        }
    }

    // Nesting depth analysis
    let mut max_nesting: usize = 0;
    let mut max_nesting_line: usize = 0;
    let mut deep_lines: usize = 0; // lines with 4+ nesting levels
    let indent_size: usize = match lang {
        "Python" => 4,
        "Go" | "Rust" => 4, // tabs count as 4
        _ => 4,
    };
    for (i, line) in lines.iter().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let leading = line.len() - line.trim_start().len();
        // Convert tabs to spaces for counting
        let tab_adjusted = line.chars().take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { indent_size } else { 1 })
            .sum::<usize>();
        let nesting = tab_adjusted / indent_size;
        if nesting >= 4 {
            deep_lines += 1;
        }
        if nesting > max_nesting {
            max_nesting = nesting;
            max_nesting_line = i + 1;
        }
    }

    // Long functions (estimate: count lines between function declarations)
    let mut long_functions: Vec<(String, usize)> = Vec::new();
    for i in 0..functions.len() {
        let start = functions[i].0;
        let end = if i + 1 < functions.len() {
            functions[i + 1].0
        } else {
            total_lines
        };
        let length = end - start;
        if length > 50 {
            long_functions.push((functions[i].1.clone(), length));
        }
    }

    // Import count
    let import_pattern = match lang {
        "Swift" => "import ",
        "PHP" => "use ",
        "Go" => "import",
        "TypeScript" | "JavaScript" => "import ",
        "Rust" => "use ",
        "Python" => "import ",
        "Kotlin" | "Java" => "import ",
        _ => "import ",
    };
    let imports = lines.iter().filter(|l| l.trim().starts_with(import_pattern)).count();

    // Lint violations on full file
    let compiled_rules = get_compiled_rules(lang);
    let mut violations: Vec<(String, String, usize)> = Vec::new(); // (rule_id, message, line)
    for (i, line) in lines.iter().enumerate() {
        for cr in compiled_rules {
            if (cr.rule.applies_to.is_empty() || cr.rule.applies_to == "line")
                && matches_compiled_rule(cr, line, file_path)
            {
                violations.push((cr.rule.id.clone(), cr.rule.name.clone(), i + 1));
            }
        }
    }

    // ─── Output ───
    let mut out = vec![
        format!("## {file_path}\n"),
        format!("**Language:** {} | **Branch:** {}\n", lang, ref_param),
        "### Metrics\n".to_string(),
        "| Metric | Value | Assessment |".to_string(),
        "|--------|-------|------------|".to_string(),
    ];

    // Total lines
    let lines_assessment = if total_lines > 500 { "Too long" } else if total_lines > 300 { "Consider splitting" } else { "OK" };
    out.push(format!("| Total lines | {} | {} |", total_lines, lines_assessment));
    out.push(format!("| Code lines | {} | |", code_lines));
    out.push(format!("| Comments | {} ({:.0}%) | {} |", comment_lines, comment_lines as f64 / total_lines as f64 * 100.0,
        if comment_lines == 0 { "No comments" } else { "OK" }));
    out.push(format!("| Blank lines | {} | |", blank_lines));

    // Functions
    let func_assessment = if functions.len() > 20 { "Too many — god class?" } else { "OK" };
    out.push(format!("| Functions | {} | {} |", functions.len(), func_assessment));

    // Imports
    let import_assessment = if imports > 15 { "Many imports — high coupling" } else if imports > 10 { "Moderate" } else { "OK" };
    out.push(format!("| Imports | {} | {} |", imports, import_assessment));

    // Nesting
    let nesting_assessment = if max_nesting >= 6 { "Deeply nested — refactor" } else if max_nesting >= 4 { "Consider flattening" } else { "OK" };
    out.push(format!("| Max nesting depth | {} (line {}) | {} |", max_nesting, max_nesting_line, nesting_assessment));
    if deep_lines > 0 {
        out.push(format!("| Lines at 4+ depth | {} | Complexity indicator |", deep_lines));
    }

    // Long functions
    if !long_functions.is_empty() {
        out.push(String::new());
        out.push("### Long Functions (>50 lines)\n".to_string());
        for (name, length) in &long_functions {
            let short = name.chars().take(60).collect::<String>();
            out.push(format!("- `{short}` — ~{length} lines"));
        }
    }

    // Violations
    if !violations.is_empty() {
        out.push(String::new());
        let unique: std::collections::BTreeMap<String, usize> = violations.iter()
            .fold(std::collections::BTreeMap::new(), |mut acc, (id, _, _)| {
                *acc.entry(id.clone()).or_insert(0) += 1;
                acc
            });
        out.push(format!("### Lint Violations ({})\n", violations.len()));
        for (id, count) in &unique {
            let name = violations.iter().find(|(rid, _, _)| rid == id).map(|(_, n, _)| n.as_str()).unwrap_or("?");
            out.push(format!("- **[{id}]** {name}: {count} occurrences"));
        }
    } else {
        out.push(String::new());
        out.push("### Lint: No violations".to_string());
    }

    // Summary score
    let mut score = 100i32;
    if total_lines > 500 { score -= 20; }
    if total_lines > 300 { score -= 10; }
    if functions.len() > 20 { score -= 15; }
    if max_nesting >= 6 { score -= 20; }
    else if max_nesting >= 4 { score -= 10; }
    if imports > 15 { score -= 10; }
    if comment_lines == 0 && code_lines > 50 { score -= 5; }
    score -= (long_functions.len() as i32) * 5;
    score -= (violations.len() as i32).min(20);
    score = score.max(0);

    let grade = match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    };

    out.push(String::new());
    out.push(format!("### Quality Score: {score}/100 (Grade {grade})"));

    Ok(out.join("\n"))
}

/// Analyze all source files in a project: fetch tree, fetch file contents concurrently,
/// compute quality metrics, return aggregate report.
pub async fn analyze_project(
    client: &GitLabClient,
    project_id: &str,
    ref_name: &str,
    max_files: usize,
    summary_only: bool,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let ref_param = if ref_name.is_empty() { "HEAD" } else { ref_name };

    // 1. Fetch recursive tree
    let entries: Vec<Value> = client
        .get_all_pages(
            &format!("/projects/{encoded}/repository/tree"),
            &[("recursive", "true"), ("ref", ref_param)],
            5,
        )
        .await?;

    // 2. Filter to source files only
    let skip_extensions: &[&str] = &[
        ".xcframework", ".framework", ".a", ".dylib", ".so",
        ".png", ".jpg", ".jpeg", ".gif", ".ico", ".svg", ".bmp", ".tiff",
        ".woff", ".woff2", ".ttf", ".eot", ".otf",
        ".lock", ".sum", ".map",
        ".min.js", ".min.css",
        ".pb.go",
        ".xcassets", ".plist",
        ".zip", ".tar", ".gz", ".rar", ".7z",
        ".pdf", ".doc", ".docx", ".xls", ".xlsx",
        ".mp3", ".mp4", ".wav", ".avi", ".mov",
        ".o", ".obj", ".exe", ".dll", ".class", ".jar",
        ".dat", ".bin", ".db", ".sqlite",
    ];
    let skip_dirs: &[&str] = &[
        "vendor/", "node_modules/", "dist/", "build/",
        ".xcframework/", ".framework/",
        "__generated__", "Pods/",
    ];

    let source_files: Vec<&str> = entries
        .iter()
        .filter_map(|e| {
            if e["type"].as_str() != Some("blob") {
                return None;
            }
            let path = e["path"].as_str()?;
            // Allow Info.plist but skip other .plist
            if path.ends_with(".plist") && !path.ends_with("Info.plist") {
                return None;
            }
            if skip_extensions.iter().any(|ext| path.ends_with(ext)) {
                return None;
            }
            if skip_dirs.iter().any(|dir| path.contains(dir)) {
                return None;
            }
            Some(path)
        })
        .collect();

    let total_source = source_files.len();
    let files_to_analyze: Vec<&str> = source_files.into_iter().take(max_files).collect();

    if files_to_analyze.is_empty() {
        return Ok(format!("No source files found in {project_id} at {ref_param}."));
    }

    // 3. Fetch file contents concurrently in batches of 10
    struct FileMetrics {
        path: String,
        total_lines: usize,
        functions: usize,
        max_nesting: usize,
        violations: usize,
        score: i32,
        grade: &'static str,
        violation_details: Vec<(String, String)>, // (rule_id, name)
    }

    let mut all_metrics: Vec<FileMetrics> = Vec::new();

    for chunk in files_to_analyze.chunks(10) {
        let futs: Vec<_> = chunk
            .iter()
            .map(|&path| {
                let client = client.clone();
                let encoded = urlencoding::encode(project_id).to_string();
                let encoded_path = urlencoding::encode(path).to_string();
                let ref_p = ref_param.to_string();
                let file_path = path.to_string();
                async move {
                    let result: std::result::Result<Value, _> = client
                        .get(
                            &format!("/projects/{encoded}/repository/files/{encoded_path}"),
                            &[("ref", ref_p.as_str())],
                        )
                        .await;
                    (file_path, result)
                }
            })
            .collect();

        let results = futures::future::join_all(futs).await;

        for (file_path, result) in results {
            let file_info = match result {
                Ok(v) => v,
                Err(_) => continue,
            };

            let content_b64 = file_info["content"].as_str().unwrap_or("");
            let content = base64_decode(content_b64);
            let lang = detect_language(&file_path);
            let lines: Vec<&str> = content.lines().collect();
            let total_lines = lines.len();

            if total_lines == 0 {
                all_metrics.push(FileMetrics {
                    path: file_path,
                    total_lines: 0,
                    functions: 0,
                    max_nesting: 0,
                    violations: 0,
                    score: 100,
                    grade: "A",
                    violation_details: Vec::new(),
                });
                continue;
            }

            // Function detection
            let func_pattern = match lang {
                "Swift" => r"(?:func|init)\s+",
                "PHP" => r"function\s+\w+",
                "Go" => r"func\s+",
                "Kotlin" | "Java" => r"fun\s+",
                "TypeScript" | "JavaScript" => r"(?:function\s+|(?:const|let|var)\s+\w+\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_]\w*)\s*=>)",
                "Rust" => r"fn\s+",
                "Python" => r"def\s+",
                _ => r"function\s+|func\s+|fn\s+|def\s+",
            };
            let func_re = regex::Regex::new(func_pattern).ok();
            let mut func_count = 0usize;
            let mut func_starts: Vec<usize> = Vec::new();
            if let Some(re) = &func_re {
                for (i, line) in lines.iter().enumerate() {
                    if re.is_match(line) {
                        func_count += 1;
                        func_starts.push(i);
                    }
                }
            }

            // Nesting depth
            let mut max_nesting: usize = 0;
            let indent_size: usize = 4;
            for line in &lines {
                if line.trim().is_empty() {
                    continue;
                }
                let tab_adjusted: usize = line
                    .chars()
                    .take_while(|c| c.is_whitespace())
                    .map(|c| if c == '\t' { indent_size } else { 1 })
                    .sum();
                let nesting = tab_adjusted / indent_size;
                if nesting > max_nesting {
                    max_nesting = nesting;
                }
            }

            // Long functions
            let mut long_func_count = 0usize;
            for i in 0..func_starts.len() {
                let start = func_starts[i];
                let end = if i + 1 < func_starts.len() {
                    func_starts[i + 1]
                } else {
                    total_lines
                };
                if end - start > 50 {
                    long_func_count += 1;
                }
            }

            // Lint violations
            let compiled_rules = get_compiled_rules(lang);
            let mut violation_details: Vec<(String, String)> = Vec::new();
            let mut violation_count = 0usize;
            for line in &lines {
                for cr in compiled_rules {
                    if (cr.rule.applies_to.is_empty() || cr.rule.applies_to == "line")
                        && matches_compiled_rule(cr, line, &file_path)
                    {
                        violation_count += 1;
                        violation_details.push((cr.rule.id.clone(), cr.rule.name.clone()));
                    }
                }
            }

            // Imports
            let import_pattern = match lang {
                "Swift" => "import ",
                "PHP" => "use ",
                "Go" => "import",
                "TypeScript" | "JavaScript" => "import ",
                "Rust" => "use ",
                "Python" => "import ",
                "Kotlin" | "Java" => "import ",
                _ => "import ",
            };
            let imports = lines.iter().filter(|l| l.trim().starts_with(import_pattern)).count();
            let comment_lines = lines.iter().filter(|l| {
                let t = l.trim();
                t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*')
            }).count();
            let code_lines = total_lines - lines.iter().filter(|l| l.trim().is_empty()).count() - comment_lines;

            // Score
            let mut score = 100i32;
            if total_lines > 500 { score -= 20; }
            else if total_lines > 300 { score -= 10; }
            if func_count > 20 { score -= 15; }
            if max_nesting >= 6 { score -= 20; }
            else if max_nesting >= 4 { score -= 10; }
            if imports > 15 { score -= 10; }
            if comment_lines == 0 && code_lines > 50 { score -= 5; }
            score -= (long_func_count as i32) * 5;
            score -= (violation_count as i32).min(20);
            score = score.max(0);

            let grade = match score {
                90..=100 => "A",
                75..=89 => "B",
                60..=74 => "C",
                40..=59 => "D",
                _ => "F",
            };

            all_metrics.push(FileMetrics {
                path: file_path,
                total_lines,
                functions: func_count,
                max_nesting,
                violations: violation_count,
                score,
                grade,
                violation_details,
            });
        }
    }

    if all_metrics.is_empty() {
        return Ok(format!("Could not analyze any files in {project_id}."));
    }

    // 4. Aggregate and format
    // Sort by score ascending (worst first for the table, but we show sorted)
    all_metrics.sort_by(|a, b| a.score.cmp(&b.score));

    // Grade counts
    let mut grade_counts: BTreeMap<&str, usize> = BTreeMap::new();
    for m in &all_metrics {
        *grade_counts.entry(m.grade).or_insert(0) += 1;
    }

    let total_analyzed = all_metrics.len();
    let avg_score: f64 =
        all_metrics.iter().map(|m| m.score as f64).sum::<f64>() / total_analyzed as f64;
    let avg_grade = match avg_score as i32 {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    };

    if summary_only {
        let grade_a = grade_counts.get("A").copied().unwrap_or(0);
        let grade_b = grade_counts.get("B").copied().unwrap_or(0);
        let grade_c = grade_counts.get("C").copied().unwrap_or(0);
        let grade_d = grade_counts.get("D").copied().unwrap_or(0);
        let grade_f = grade_counts.get("F").copied().unwrap_or(0);

        // Collect top issues
        let mut issue_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
        for m in &all_metrics {
            for (rule_id, name) in &m.violation_details {
                *issue_counts.entry((rule_id.clone(), name.clone())).or_insert(0) += 1;
            }
        }
        let mut sorted_issues: Vec<_> = issue_counts.into_iter().collect();
        sorted_issues.sort_by(|a, b| b.1.cmp(&a.1));
        let top_issues: Vec<String> = sorted_issues.iter().take(3)
            .map(|((_, name), count)| format!("{name} ({count})"))
            .collect();
        let issues_str = if top_issues.is_empty() { "none".to_string() } else { top_issues.join(", ") };

        return Ok(format!(
            "{project_id}: {total_analyzed} files, avg {:.0}/100 ({avg_grade}). A:{grade_a} B:{grade_b} C:{grade_c} D:{grade_d} F:{grade_f}. Top issues: {issues_str}",
            avg_score
        ));
    }

    let mut out = vec![
        format!("## Project Quality: {project_id}\n"),
        format!(
            "**Files analyzed:** {} of {} source files | **Branch:** {}\n",
            total_analyzed, total_source, ref_param
        ),
        "### Summary".to_string(),
        "| Grade | Files | % |".to_string(),
        "|-------|-------|---|".to_string(),
    ];

    let grade_labels = [("A", "90-100"), ("B", "75-89"), ("C", "60-74"), ("D", "40-59"), ("F", "<40")];
    for (g, range) in &grade_labels {
        let count = grade_counts.get(g).copied().unwrap_or(0);
        let pct = if total_analyzed > 0 {
            count as f64 / total_analyzed as f64 * 100.0
        } else {
            0.0
        };
        out.push(format!("| {} ({}) | {} | {:.0}% |", g, range, count, pct));
    }

    out.push(format!(
        "\n**Average score:** {:.0}/100 ({})\n",
        avg_score, avg_grade
    ));

    // Files table (sorted by score ascending – worst at bottom)
    out.push("### Files by Score".to_string());
    out.push("| File | Lines | Functions | Max Nesting | Violations | Score | Grade |".to_string());
    out.push("|------|-------|-----------|-------------|------------|-------|-------|".to_string());

    // Show sorted best-to-worst
    let mut sorted_best_first = all_metrics.iter().collect::<Vec<_>>();
    sorted_best_first.sort_by(|a, b| b.score.cmp(&a.score));

    for m in &sorted_best_first {
        let short_path = m.path.rsplit('/').next().unwrap_or(&m.path);
        out.push(format!(
            "| {} | {} | {} | {} | {} | {} | {} |",
            short_path, m.total_lines, m.functions, m.max_nesting, m.violations, m.score, m.grade
        ));
    }

    // Top issues across all files
    let mut issue_counts: BTreeMap<(String, String), usize> = BTreeMap::new();
    let mut issue_file_counts: BTreeMap<String, usize> = BTreeMap::new();
    for m in &all_metrics {
        let mut seen_rules: std::collections::HashSet<String> = std::collections::HashSet::new();
        for (rule_id, name) in &m.violation_details {
            *issue_counts.entry((rule_id.clone(), name.clone())).or_insert(0) += 1;
            if seen_rules.insert(rule_id.clone()) {
                *issue_file_counts.entry(rule_id.clone()).or_insert(0) += 1;
            }
        }
    }

    if !issue_counts.is_empty() {
        let mut sorted_issues: Vec<_> = issue_counts.iter().collect();
        sorted_issues.sort_by(|a, b| b.1.cmp(a.1));

        out.push("\n### Top Issues (across all files)".to_string());
        for ((rule_id, name), count) in sorted_issues.iter().take(10) {
            let files = issue_file_counts.get(rule_id.as_str()).copied().unwrap_or(0);
            out.push(format!("- **{name}** [{rule_id}]: {files} files, {count} occurrences"));
        }
    }

    // Recommendations
    let bad_files: Vec<_> = all_metrics
        .iter()
        .filter(|m| m.score < 60)
        .collect();
    if !bad_files.is_empty() {
        out.push("\n### Recommendations".to_string());
        for m in &bad_files {
            let short_path = m.path.rsplit('/').next().unwrap_or(&m.path);
            let reason = if m.total_lines > 300 {
                format!("Grade {}, {} lines – needs splitting", m.grade, m.total_lines)
            } else {
                format!("Grade {}, {} violations", m.grade, m.violations)
            };
            out.push(format!("- **{short_path}** — {reason}"));
        }
    }

    Ok(out.join("\n"))
}

/// Validate recent project commits against message conventions and code rules.
pub async fn validate_project_commits(
    client: &GitLabClient,
    project_id: &str,
    days: u32,
    branch: &str,
) -> Result<String> {
    let encoded = urlencoding::encode(project_id);
    let since = (chrono::Utc::now() - chrono::Duration::days(days as i64))
        .format("%Y-%m-%dT00:00:00Z")
        .to_string();

    let mut params: Vec<(&str, &str)> = vec![
        ("since", &since),
        ("per_page", "100"),
    ];
    if !branch.is_empty() {
        params.push(("ref_name", branch));
    }

    let commits: Vec<Value> = client
        .get_all_pages(
            &format!("/projects/{encoded}/repository/commits"),
            &params,
            3,
        )
        .await?;

    if commits.is_empty() {
        return Ok(format!("No commits in the last {days} days for {project_id}."));
    }

    // Filter out merge commits
    let non_merge: Vec<&Value> = commits
        .iter()
        .filter(|c| {
            let msg = c["message"].as_str().unwrap_or("");
            !msg.starts_with("Merge branch") && !msg.starts_with("Merge remote")
        })
        .collect();

    let total = non_merge.len();
    if total == 0 {
        return Ok(format!("Only merge commits in the last {days} days for {project_id}."));
    }

    // Conventional commit prefixes
    let conventional_prefixes = [
        "feat:", "fix:", "docs:", "build:", "chore:", "refactor:",
        "test:", "ci:", "perf:", "style:", "revert:",
        "feat(", "fix(", "docs(", "build(", "chore(", "refactor(",
        "test(", "ci(", "perf(", "style(", "revert(",
    ];
    let ticket_re = regex::Regex::new(r"[A-Z]{2,10}-\d+").ok();

    let mut conventional_pass = 0u32;
    let mut ticket_pass = 0u32;
    let mut length_pass = 0u32;
    let mut failing_messages: Vec<(String, String, Vec<&str>)> = Vec::new(); // (sha, subject, issues)

    for commit in &non_merge {
        let msg = commit["message"].as_str().unwrap_or("");
        let subject = msg.lines().next().unwrap_or("").trim();
        let short_sha = commit["short_id"]
            .as_str()
            .unwrap_or("???????");

        let mut issues: Vec<&str> = Vec::new();

        // Conventional format check
        let is_conventional = conventional_prefixes
            .iter()
            .any(|p| subject.to_lowercase().starts_with(&p.to_lowercase()));
        if is_conventional {
            conventional_pass += 1;
        } else {
            issues.push("no conventional prefix");
        }

        // Ticket reference check
        let has_ticket = ticket_re
            .as_ref()
            .map(|re| re.is_match(msg))
            .unwrap_or(false);
        if has_ticket {
            ticket_pass += 1;
        } else {
            issues.push("no ticket reference");
        }

        // Subject length check
        if subject.len() <= 72 {
            length_pass += 1;
        } else {
            issues.push("subject >72 chars");
        }

        if !issues.is_empty() {
            failing_messages.push((short_sha.to_string(), subject.to_string(), issues));
        }
    }

    // Also run code validation on commits with diffs (sample up to 10)
    let sample_size = total.min(10);
    let mut commits_with_violations = 0u32;
    let mut critical_violations = 0u32;

    for commit in non_merge.iter().take(sample_size) {
        let sha = commit["id"].as_str().unwrap_or("?");
        let result = validate_commit(client, project_id, sha).await;
        if let Ok(ref text) = result {
            if !text.contains("No violations") {
                commits_with_violations += 1;
                if text.contains("CRITICAL") {
                    critical_violations += 1;
                }
            }
        }
    }

    let branch_label = if branch.is_empty() { "default" } else { branch };
    let total_u32 = total as u32;

    let conv_pct = if total > 0 { conventional_pass as f64 / total as f64 * 100.0 } else { 0.0 };
    let ticket_pct = if total > 0 { ticket_pass as f64 / total as f64 * 100.0 } else { 0.0 };
    let length_pct = if total > 0 { length_pass as f64 / total as f64 * 100.0 } else { 0.0 };

    let mut out = vec![
        format!("## Commit Quality: {project_id} (last {days} days, {total} commits)\n"),
        format!("**Branch:** {branch_label}\n"),
        "### Message Conventions".to_string(),
        "| Check | Pass | Fail | % |".to_string(),
        "|-------|------|------|---|".to_string(),
        format!(
            "| Conventional format | {} | {} | {:.0}% |",
            conventional_pass,
            total_u32 - conventional_pass,
            conv_pct
        ),
        format!(
            "| Ticket reference | {} | {} | {:.0}% |",
            ticket_pass,
            total_u32 - ticket_pass,
            ticket_pct
        ),
        format!(
            "| Subject length <72 | {} | {} | {:.0}% |",
            length_pass,
            total_u32 - length_pass,
            length_pct
        ),
    ];

    if !failing_messages.is_empty() {
        out.push("\n### Failing Messages".to_string());
        for (sha, subject, issues) in failing_messages.iter().take(20) {
            let short_subject: String = subject.chars().take(60).collect();
            out.push(format!(
                "- `{sha}` \"{}\" — {}",
                short_subject,
                issues.join(", ")
            ));
        }
        if failing_messages.len() > 20 {
            out.push(format!("  ...and {} more", failing_messages.len() - 20));
        }
    }

    if sample_size > 0 {
        out.push(format!(
            "\n### Code Violations (from diffs, {} commits sampled)",
            sample_size
        ));
        if commits_with_violations == 0 {
            out.push("No code violations found.".to_string());
        } else {
            out.push(format!(
                "- {} commits with violations, {} critical",
                commits_with_violations, critical_violations
            ));
        }
    }

    Ok(out.join("\n"))
}

/// Public file metrics struct for cross-module use.
pub struct FileMetricsPub {
    pub path: String,
    pub total_lines: usize,
    pub functions: usize,
    pub max_nesting: usize,
    pub violations: usize,
    pub score: i32,
    pub grade: &'static str,
    pub violation_details: Vec<(String, String)>,
}

/// Public base64 decode for cross-module use.
pub fn base64_decode_pub(input: &str) -> String {
    base64_decode(input)
}

/// Compute quality metrics for a file given its content and detected language.
/// Reuses the same scoring logic as analyze_project.
pub fn compute_file_metrics(file_path: &str, content: &str, lang: &str) -> FileMetricsPub {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();

    if total_lines == 0 {
        return FileMetricsPub {
            path: file_path.to_string(),
            total_lines: 0,
            functions: 0,
            max_nesting: 0,
            violations: 0,
            score: 100,
            grade: "A",
            violation_details: Vec::new(),
        };
    }

    // Function detection
    let func_pattern = match lang {
        "Swift" => r"(?:func|init)\s+",
        "PHP" => r"function\s+\w+",
        "Go" => r"func\s+",
        "Kotlin" | "Java" => r"fun\s+",
        "TypeScript" | "JavaScript" => r"(?:function\s+|(?:const|let|var)\s+\w+\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_]\w*)\s*=>)",
        "Rust" => r"fn\s+",
        "Python" => r"def\s+",
        _ => r"function\s+|func\s+|fn\s+|def\s+",
    };
    let func_re = regex::Regex::new(func_pattern).ok();
    let mut func_count = 0usize;
    let mut func_starts: Vec<usize> = Vec::new();
    if let Some(re) = &func_re {
        for (i, line) in lines.iter().enumerate() {
            if re.is_match(line) {
                func_count += 1;
                func_starts.push(i);
            }
        }
    }

    // Nesting depth
    let mut max_nesting: usize = 0;
    let indent_size: usize = 4;
    for line in &lines {
        if line.trim().is_empty() {
            continue;
        }
        let tab_adjusted: usize = line
            .chars()
            .take_while(|c| c.is_whitespace())
            .map(|c| if c == '\t' { indent_size } else { 1 })
            .sum();
        let nesting = tab_adjusted / indent_size;
        if nesting > max_nesting {
            max_nesting = nesting;
        }
    }

    // Long functions
    let mut long_func_count = 0usize;
    for i in 0..func_starts.len() {
        let start = func_starts[i];
        let end = if i + 1 < func_starts.len() {
            func_starts[i + 1]
        } else {
            total_lines
        };
        if end - start > 50 {
            long_func_count += 1;
        }
    }

    // Lint violations
    let compiled_rules = get_compiled_rules(lang);
    let mut violation_details: Vec<(String, String)> = Vec::new();
    let mut violation_count = 0usize;
    for line in &lines {
        for cr in compiled_rules {
            if (cr.rule.applies_to.is_empty() || cr.rule.applies_to == "line")
                && matches_compiled_rule(cr, line, file_path)
            {
                violation_count += 1;
                violation_details.push((cr.rule.id.clone(), cr.rule.name.clone()));
            }
        }
    }

    // Imports
    let import_pattern = match lang {
        "Swift" => "import ",
        "PHP" => "use ",
        "Go" => "import",
        "TypeScript" | "JavaScript" => "import ",
        "Rust" => "use ",
        "Python" => "import ",
        "Kotlin" | "Java" => "import ",
        _ => "import ",
    };
    let imports = lines.iter().filter(|l| l.trim().starts_with(import_pattern)).count();
    let comment_lines = lines.iter().filter(|l| {
        let t = l.trim();
        t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*')
    }).count();
    let code_lines = total_lines - lines.iter().filter(|l| l.trim().is_empty()).count() - comment_lines;

    // Score
    let mut score = 100i32;
    if total_lines > 500 { score -= 20; }
    else if total_lines > 300 { score -= 10; }
    if func_count > 20 { score -= 15; }
    if max_nesting >= 6 { score -= 20; }
    else if max_nesting >= 4 { score -= 10; }
    if imports > 15 { score -= 10; }
    if comment_lines == 0 && code_lines > 50 { score -= 5; }
    score -= (long_func_count as i32) * 5;
    score -= (violation_count as i32).min(20);
    score = score.max(0);

    let grade = match score {
        90..=100 => "A",
        75..=89 => "B",
        60..=74 => "C",
        40..=59 => "D",
        _ => "F",
    };

    FileMetricsPub {
        path: file_path.to_string(),
        total_lines,
        functions: func_count,
        max_nesting,
        violations: violation_count,
        score,
        grade,
        violation_details,
    }
}

fn base64_decode(input: &str) -> String {
    // GitLab returns base64-encoded file content
    let cleaned: String = input.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = base64_decode_bytes(&cleaned);
    String::from_utf8_lossy(&bytes).to_string()
}

fn base64_decode_bytes(input: &str) -> Vec<u8> {
    const TABLE: [u8; 256] = {
        let mut t = [255u8; 256];
        let mut i = 0u8;
        while i < 26 { t[(b'A' + i) as usize] = i; i += 1; }
        let mut i = 0u8;
        while i < 26 { t[(b'a' + i) as usize] = 26 + i; i += 1; }
        let mut i = 0u8;
        while i < 10 { t[(b'0' + i) as usize] = 52 + i; i += 1; }
        t[b'+' as usize] = 62;
        t[b'/' as usize] = 63;
        t
    };
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len() * 3 / 4);
    let mut i = 0;
    while i + 3 < bytes.len() {
        let a = TABLE[bytes[i] as usize] as u32;
        let b = TABLE[bytes[i+1] as usize] as u32;
        let c = TABLE[bytes[i+2] as usize] as u32;
        let d = TABLE[bytes[i+3] as usize] as u32;
        if a == 255 || b == 255 { break; }
        let triple = (a << 18) | (b << 12) | (c << 6) | d;
        out.push((triple >> 16) as u8);
        if bytes[i+2] != b'=' { out.push((triple >> 8) as u8); }
        if bytes[i+3] != b'=' { out.push(triple as u8); }
        i += 4;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_should_skip_lint_file() {
        assert!(should_skip_lint_file("package-lock.json"));
        assert!(should_skip_lint_file("vendor/something.php"));
        assert!(should_skip_lint_file("image.png"));
        assert!(should_skip_lint_file("styles.min.css"));
        assert!(!should_skip_lint_file("src/main.rs"));
        assert!(!should_skip_lint_file("app/Controller.php"));
    }
}
