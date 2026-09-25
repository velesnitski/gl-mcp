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

/// Rule severity. Declaration order is report order: `Ord` puts critical first.
///
/// A typo such as `severity = "warn"` in a rule file is now a parse error caught by
/// the rule-file gate test, instead of a rule whose hits never appear in any report
/// (the reports only printed the three spellings they knew).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum RuleSeverity {
    Critical,
    Warning,
    Info,
}

impl RuleSeverity {
    fn icon(self) -> &'static str {
        match self {
            Self::Critical => "🔴",
            Self::Warning => "🟡",
            Self::Info => "🔵",
        }
    }

    fn heading(self) -> &'static str {
        match self {
            Self::Critical => "CRITICAL",
            Self::Warning => "WARNING",
            Self::Info => "INFO",
        }
    }
}

/// Letter grade for a quality score.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Grade {
    A,
    B,
    C,
    D,
    F,
}

impl Grade {
    pub const ALL: [Grade; 5] = [Grade::A, Grade::B, Grade::C, Grade::D, Grade::F];

    /// The only place the grade bands are defined; the per-file grade and the
    /// project-average grade used to carry separate copies of this match.
    pub fn from_score(score: i32) -> Self {
        match score {
            90.. => Self::A,
            75..=89 => Self::B,
            60..=74 => Self::C,
            40..=59 => Self::D,
            _ => Self::F,
        }
    }

    /// Score range of the band, as printed in reports.
    pub fn band(self) -> &'static str {
        match self {
            Self::A => "90-100",
            Self::B => "75-89",
            Self::C => "60-74",
            Self::D => "40-59",
            Self::F => "<40",
        }
    }
}

impl std::fmt::Display for Grade {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::A => "A",
            Self::B => "B",
            Self::C => "C",
            Self::D => "D",
            Self::F => "F",
        })
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct Rule {
    pub id: String,
    pub severity: RuleSeverity,
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
    pub severity: RuleSeverity,
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
        let pattern_re = compile_rule_regex(&rule.id, "pattern", &rule.pattern);
        let neg_pattern_re = compile_rule_regex(&rule.id, "negative_pattern", &rule.negative_pattern);
        let neg_file_re = compile_rule_regex(&rule.id, "negative_file_pattern", &rule.negative_file_pattern);
        Self { rule, pattern_re, neg_pattern_re, neg_file_re }
    }
}

/// `None` for an empty pattern. An invalid one disables only that rule — and says so:
/// seven rules once shipped with lookarounds the `regex` crate rejects, and were
/// silently dead until `every_rule_file_parses_and_every_pattern_compiles` caught them.
fn compile_rule_regex(rule_id: &str, field: &str, pattern: &str) -> Option<regex::Regex> {
    if pattern.is_empty() {
        return None;
    }
    regex::Regex::new(pattern)
        .inspect_err(|e| tracing::error!("lint rule {rule_id}: invalid {field}, rule disabled: {e}"))
        .ok()
}

// ─── Rule loading (cached, parsed once) ───

fn parse_embedded_rules(content: &str) -> Vec<Rule> {
    toml::from_str::<RuleFile>(content)
        .map(|rf| rf.rule)
        .inspect_err(|e| tracing::error!("embedded lint rule file does not parse, its rules are disabled: {e}"))
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
                severity: cr.rule.severity,
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
                                severity: cr.rule.severity,
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
                    severity: RuleSeverity::Info,
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
                    severity: cr.rule.severity,
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
    let mut by_severity: BTreeMap<RuleSeverity, Vec<&Violation>> = BTreeMap::new();
    for v in &violations {
        by_severity.entry(v.severity).or_default().push(v);
    }

    let total_suppressed: usize = suppressed.values().sum();
    let total_shown = violations.len();

    let mut lines = vec![
        format!(
            "**{project_id} `{short_sha}`** by {author} — **{} violations** ({} files)",
            total_shown + total_suppressed,
            diffs.len()
        ),
        String::new(),
    ];

    for (sev, sevs) in &by_severity {
        {
            lines.push(format!("### {} {} ({})", sev.icon(), sev.heading(), sevs.len()));
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
                                severity: cr.rule.severity,
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
                    severity: cr.rule.severity,
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

    let mut by_severity: BTreeMap<RuleSeverity, Vec<&Violation>> = BTreeMap::new();
    for v in &violations {
        by_severity.entry(v.severity).or_default().push(v);
    }

    let total_suppressed: usize = suppressed.values().sum();
    let total_shown = violations.len();

    let mut lines = vec![
        format!(
            "**{project_id} !{mr_iid}** \"{}\" by @{author} — **{} violations** ({files_checked} files)",
            title, total_shown + total_suppressed
        ),
        String::new(),
    ];

    for (sev, sevs) in &by_severity {
        {
            lines.push(format!("### {} {} ({})", sev.icon(), sev.heading(), sevs.len()));
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

    let mut by_severity: BTreeMap<RuleSeverity, Vec<&Rule>> = BTreeMap::new();
    for r in &rules {
        by_severity.entry(r.severity).or_default().push(r);
    }

    for (sev, sevs) in &by_severity {
        {
            lines.push(format!("### {} {} ({})", sev.icon(), sev.heading(), sevs.len()));
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
/// Inputs to the quality score — the aggregates both file analyses compute.
pub(crate) struct ScoreInputs {
    pub total_lines: usize,
    pub func_count: usize,
    pub max_nesting: usize,
    pub imports: usize,
    pub comment_lines: usize,
    pub code_lines: usize,
    pub long_funcs: usize,
    pub violations: usize,
}

/// Score and grade for a file — the **only** place this is decided.
///
/// This existed twice, in `analyze_file` and in `compute_file_metrics` (which backs
/// `analyze_project`), and the copies had drifted: one lost an `else` and charged a
/// file over 500 lines both the >500 and the >300 penalty, so the same file scored
/// 10 points lower — sometimes a full grade lower — depending on which tool was asked.
/// The tiers are exclusive: a file is either long or very long, never both.
pub(crate) fn quality_score(m: &ScoreInputs) -> (i32, Grade) {
    let mut score = 100i32;
    if m.total_lines > 500 {
        score -= 20;
    } else if m.total_lines > 300 {
        score -= 10;
    }
    if m.func_count > 20 {
        score -= 15;
    }
    if m.max_nesting >= 6 {
        score -= 20;
    } else if m.max_nesting >= 4 {
        score -= 10;
    }
    if m.imports > 15 {
        score -= 10;
    }
    if m.comment_lines == 0 && m.code_lines > 50 {
        score -= 5;
    }
    score -= (m.long_funcs as i32) * 5;
    score -= (m.violations as i32).min(20);
    let score = score.max(0);
    (score, Grade::from_score(score))
}

/// Length in lines of each function, given 0-based start lines in file order.
///
/// A function runs from its start to the next function's start, and the last one to
/// the end of the file. Shared by both analyses: they previously disagreed by one on
/// the last function (one counted from a 1-based start), so a 51-line function at the
/// end of a file was "long" in one tool and not the other.
pub(crate) fn function_lengths(starts: &[usize], total_lines: usize) -> Vec<usize> {
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| starts.get(i + 1).copied().unwrap_or(total_lines) - s)
        .collect()
}

/// Characters of leading whitespace per nesting level. A tab counts as one full level.
const INDENT_WIDTH: usize = 4;

/// Declaration patterns per language; a line matching one starts a function.
const FUNCTION_PATTERNS: &[(&[&str], &str)] = &[
    // `init(`, `init?(`, `init<T>(` — an initializer is never followed by whitespace.
    (&["Swift"], r"\bfunc\s+|\binit[?!]?\s*[(<]"),
    (&["PHP"], r"function\s+\w+"),
    (&["Go"], r"func\s+"),
    (&["Kotlin", "Java"], r"fun\s+"),
    (
        &["TypeScript", "JavaScript"],
        r"(?:function\s+|(?:const|let|var)\s+\w+\s*=\s*(?:async\s+)?(?:\([^)]*\)|[a-zA-Z_]\w*)\s*=>)",
    ),
    (&["Rust"], r"fn\s+"),
    (&["Python"], r"def\s+"),
];
const FALLBACK_FUNCTION_PATTERN: &str = r"function\s+|func\s+|fn\s+|def\s+";

/// Compiled once. The patterns are constants, so a compile failure is a programming
/// error caught by `function_patterns_all_compile`, not a runtime condition.
static FUNCTION_RES: LazyLock<Vec<(&'static [&'static str], regex::Regex)>> = LazyLock::new(|| {
    FUNCTION_PATTERNS
        .iter()
        .map(|&(langs, p)| (langs, regex::Regex::new(p).expect("function pattern is a valid regex")))
        .collect()
});
static FALLBACK_FUNCTION_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(FALLBACK_FUNCTION_PATTERN).expect("fallback function pattern is a valid regex"));

fn function_regex(lang: &str) -> &'static regex::Regex {
    FUNCTION_RES
        .iter()
        .find(|(langs, _)| langs.contains(&lang))
        .map(|(_, re)| re)
        .unwrap_or(&FALLBACK_FUNCTION_RE)
}

/// Line prefix that marks an import in `lang`.
fn import_prefix(lang: &str) -> &'static str {
    match lang {
        "PHP" | "Rust" => "use ",
        "Go" => "import",
        _ => "import ",
    }
}

/// Ticket reference such as `ABC-123`: 2–10 capitals, a dash, digits.
///
/// Shared by commit validation and the reports, which used to disagree — the report
/// accepted any capitals (`A-1`, `UTF-8`), so its ticket rate overstated what commit
/// validation would pass.
static TICKET_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"[A-Z]{2,10}-\d+").expect("ticket pattern is a valid regex"));

pub(crate) fn has_ticket_ref(text: &str) -> bool {
    TICKET_RE.is_match(text)
}

/// A lint rule hit in the file.
pub(crate) struct LineViolation {
    pub rule_id: String,
    pub name: String,
}

/// Everything measured about one file — computed in exactly one place.
///
/// `analyze_file` and `analyze_project` each carried their own copy of this
/// computation; the copies had drifted before (see `quality_score`). Both now
/// render from this struct, so they cannot disagree about a file again.
pub(crate) struct FileFacts {
    pub total_lines: usize,
    pub blank_lines: usize,
    pub comment_lines: usize,
    pub code_lines: usize,
    /// (1-based line, declaration text truncated to 80 chars)
    pub functions: Vec<(usize, String)>,
    pub max_nesting: usize,
    /// 1-based line of the first line reaching `max_nesting`; 0 for an empty file.
    pub max_nesting_line: usize,
    /// Lines nested four or more levels deep.
    pub deep_lines: usize,
    /// (declaration, length) of functions longer than 50 lines.
    pub long_functions: Vec<(String, usize)>,
    pub imports: usize,
    pub violations: Vec<LineViolation>,
}

impl FileFacts {
    pub(crate) fn score(&self) -> (i32, Grade) {
        quality_score(&ScoreInputs {
            total_lines: self.total_lines,
            func_count: self.functions.len(),
            max_nesting: self.max_nesting,
            imports: self.imports,
            comment_lines: self.comment_lines,
            code_lines: self.code_lines,
            long_funcs: self.long_functions.len(),
            violations: self.violations.len(),
        })
    }
}

fn is_comment(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*')
}

fn nesting_level(line: &str) -> usize {
    let width: usize = line
        .chars()
        .take_while(|c| c.is_whitespace())
        .map(|c| if c == '\t' { INDENT_WIDTH } else { 1 })
        .sum();
    width / INDENT_WIDTH
}

pub(crate) fn file_facts(file_path: &str, content: &str, lang: &str) -> FileFacts {
    let lines: Vec<&str> = content.lines().collect();
    let total_lines = lines.len();
    let blank_lines = lines.iter().filter(|l| l.trim().is_empty()).count();
    let comment_lines = lines.iter().filter(|l| is_comment(l)).count();

    let func_re = function_regex(lang);
    let functions: Vec<(usize, String)> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| func_re.is_match(l))
        .map(|(i, l)| (i + 1, l.trim().chars().take(80).collect()))
        .collect();

    let (mut max_nesting, mut max_nesting_line, mut deep_lines) = (0, 0, 0);
    for (i, line) in lines.iter().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let nesting = nesting_level(line);
        if nesting >= 4 {
            deep_lines += 1;
        }
        if nesting > max_nesting {
            max_nesting = nesting;
            max_nesting_line = i + 1;
        }
    }

    let starts: Vec<usize> = functions.iter().map(|(line, _)| line - 1).collect();
    let long_functions = function_lengths(&starts, total_lines)
        .into_iter()
        .zip(&functions)
        .filter(|(len, _)| *len > 50)
        .map(|(len, (_, name))| (name.clone(), len))
        .collect();

    let prefix = import_prefix(lang);
    let imports = lines.iter().filter(|l| l.trim().starts_with(prefix)).count();

    let rules = get_compiled_rules(lang);
    let violations = lines
        .iter()
        .flat_map(|line| {
            rules
                .iter()
                .filter(move |cr| {
                    (cr.rule.applies_to.is_empty() || cr.rule.applies_to == "line")
                        && matches_compiled_rule(cr, line, file_path)
                })
                .map(|cr| LineViolation { rule_id: cr.rule.id.clone(), name: cr.rule.name.clone() })
        })
        .collect();

    FileFacts {
        total_lines,
        blank_lines,
        comment_lines,
        code_lines: total_lines - blank_lines - comment_lines,
        functions,
        max_nesting,
        max_nesting_line,
        deep_lines,
        long_functions,
        imports,
        violations,
    }
}

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

    let content = crate::tools::encoding::file_text(&file_info)?;

    Ok(analyze_content(file_path, ref_param, &content))
}

/// Quality metrics and lint findings for one file's contents.
///
/// Pure: text in, report out. Split from `analyze_file` so every threshold, the
/// score and the grade can be verified on plain strings rather than through a
/// network fetch the analysis has nothing to do with.
pub(crate) fn analyze_content(file_path: &str, ref_param: &str, content: &str) -> String {
    let lang = detect_language(file_path);
    if content.lines().next().is_none() {
        return format!("**{file_path}** — empty file");
    }
    let facts = file_facts(file_path, content, lang);
    let FileFacts {
        total_lines,
        blank_lines,
        comment_lines,
        code_lines,
        ref functions,
        max_nesting,
        max_nesting_line,
        deep_lines,
        ref long_functions,
        imports,
        ref violations,
    } = facts;

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
        for (name, length) in long_functions {
            let short = name.chars().take(60).collect::<String>();
            out.push(format!("- `{short}` — ~{length} lines"));
        }
    }

    // Violations
    if !violations.is_empty() {
        out.push(String::new());
        let mut unique: BTreeMap<&str, (&str, usize)> = BTreeMap::new();
        for v in violations {
            unique.entry(&v.rule_id).or_insert((&v.name, 0)).1 += 1;
        }
        out.push(format!("### Lint Violations ({})\n", violations.len()));
        for (id, (name, count)) in &unique {
            out.push(format!("- **[{id}]** {name}: {count} occurrences"));
        }
    } else {
        out.push(String::new());
        out.push("### Lint: No violations".to_string());
    }

    let (score, grade) = facts.score();

    out.push(String::new());
    out.push(format!("### Quality Score: {score}/100 (Grade {grade})"));

    out.join("\n")
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

    // 3. Fetch file contents concurrently in batches of 10, compute metrics via shared helper.
    let mut all_metrics: Vec<FileMetricsPub> = Vec::new();

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

            // A file that does not decode is skipped, like one that failed to
            // fetch — scoring a corrupted payload would report a fiction.
            let Ok(content) = crate::tools::encoding::file_text(&file_info) else {
                continue;
            };
            let lang = detect_language(&file_path);

            all_metrics.push(compute_file_metrics(&file_path, &content, lang));
        }
    }

    if all_metrics.is_empty() {
        return Ok(format!("Could not analyze any files in {project_id}."));
    }

    // 4. Aggregate and format
    // Sort by score ascending (worst first for the table, but we show sorted)
    all_metrics.sort_by(|a, b| a.score.cmp(&b.score));

    // Grade counts
    let mut grade_counts: BTreeMap<Grade, usize> = BTreeMap::new();
    for m in &all_metrics {
        *grade_counts.entry(m.grade).or_insert(0) += 1;
    }

    let total_analyzed = all_metrics.len();
    let avg_score: f64 =
        all_metrics.iter().map(|m| m.score as f64).sum::<f64>() / total_analyzed as f64;
    let avg_grade = Grade::from_score(avg_score as i32);

    if summary_only {
        let grade_a = grade_counts.get(&Grade::A).copied().unwrap_or(0);
        let grade_b = grade_counts.get(&Grade::B).copied().unwrap_or(0);
        let grade_c = grade_counts.get(&Grade::C).copied().unwrap_or(0);
        let grade_d = grade_counts.get(&Grade::D).copied().unwrap_or(0);
        let grade_f = grade_counts.get(&Grade::F).copied().unwrap_or(0);

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

    for g in Grade::ALL {
        let range = g.band();
        let count = grade_counts.get(&g).copied().unwrap_or(0);
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

    let mut conventional_pass = 0u32;
    let mut ticket_pass = 0u32;
    let mut length_pass = 0u32;
    let mut failing_messages: Vec<(String, String, Vec<String>)> = Vec::new(); // (sha, subject, issues)

    for commit in &non_merge {
        let msg = commit["message"].as_str().unwrap_or("");
        let subject = msg.lines().next().unwrap_or("").trim();
        let short_sha = commit["short_id"]
            .as_str()
            .unwrap_or("???????");

        let report = validate_commit_message(msg);

        if report.has_conventional_prefix { conventional_pass += 1; }
        if report.has_ticket_ref { ticket_pass += 1; }
        if !report.is_too_long { length_pass += 1; }

        if !report.failures.is_empty() {
            failing_messages.push((short_sha.to_string(), subject.to_string(), report.failures));
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

/// Result of validating a single commit message against project conventions.
pub struct CommitMessageReport {
    pub has_conventional_prefix: bool,
    pub has_ticket_ref: bool,
    pub subject_length: usize,
    pub is_too_long: bool, // subject >72 chars
    pub failures: Vec<String>, // human-readable issues
}

/// Conventional Commit prefixes recognized by `validate_commit_message`.
const CONVENTIONAL_PREFIXES: &[&str] = &[
    "feat:", "fix:", "docs:", "build:", "chore:", "refactor:",
    "test:", "ci:", "perf:", "style:", "revert:",
    "feat(", "fix(", "docs(", "build(", "chore(", "refactor(",
    "test(", "ci(", "perf(", "style(", "revert(",
];

/// Validate a commit message against shared project conventions:
/// conventional-commit prefix, ticket reference, subject length <=72.
pub fn validate_commit_message(msg: &str) -> CommitMessageReport {
    let subject = msg.lines().next().unwrap_or("").trim();
    let subject_lower = subject.to_lowercase();

    let has_conventional_prefix = CONVENTIONAL_PREFIXES
        .iter()
        .any(|p| subject_lower.starts_with(&p.to_lowercase()));

    let has_ticket_ref = has_ticket_ref(msg);

    let subject_length = subject.len();
    let is_too_long = subject_length > 72;

    let mut failures: Vec<String> = Vec::new();
    if !has_conventional_prefix {
        failures.push("no conventional prefix".to_string());
    }
    if !has_ticket_ref {
        failures.push("no ticket reference".to_string());
    }
    if is_too_long {
        failures.push("subject >72 chars".to_string());
    }

    CommitMessageReport {
        has_conventional_prefix,
        has_ticket_ref,
        subject_length,
        is_too_long,
        failures,
    }
}

/// Public file metrics struct for cross-module use.
pub struct FileMetricsPub {
    pub path: String,
    pub total_lines: usize,
    pub functions: usize,
    pub max_nesting: usize,
    pub violations: usize,
    pub score: i32,
    pub grade: Grade,
    pub violation_details: Vec<(String, String)>,
}


/// Compute quality metrics for a file given its content and detected language.
/// Reuses the same scoring logic as analyze_project.
pub fn compute_file_metrics(file_path: &str, content: &str, lang: &str) -> FileMetricsPub {
    let facts = file_facts(file_path, content, lang);
    let (score, grade) = facts.score();
    FileMetricsPub {
        path: file_path.to_string(),
        total_lines: facts.total_lines,
        functions: facts.functions.len(),
        max_nesting: facts.max_nesting,
        violations: facts.violations.len(),
        score,
        grade,
        violation_details: facts.violations.into_iter().map(|v| (v.rule_id, v.name)).collect(),
    }
}



#[cfg(test)]
mod tests {
    use super::*;

    // ─── Golden master: pins the analysis output byte-for-byte across refactors ───

    /// Synthetic sources in every language the analyzer distinguishes, built to
    /// cross the thresholds: long functions, deep nesting, comments, imports, and
    /// lines that trip rules. Regenerate with `UPDATE_GOLDEN=1 cargo test golden`.
    fn golden_inputs() -> Vec<(&'static str, String)> {
        let deep = |d: usize| format!("{}x = 1;", "    ".repeat(d));
        let long_body = |n: usize| (0..n).map(|i| format!("    let v{i} = {i};")).collect::<Vec<_>>().join("\n");
        vec![
            ("src/lib.rs", format!("use std::io;\nuse std::fmt;\n// helper\nfn a() {{\n{}\n}}\nfn b() {{\n{}\n{}\n}}\n// TODO fix this\nlet port = 8080;", long_body(55), deep(5), deep(7))),
            ("app/Ctrl.php", "<?php\nuse App\\Models\\User;\nfunction handle() {\n    $x = $request->input('email');\n    // TODO clean up\n    return 4096;\n}\n".to_string()),
            ("App/View.swift", "import UIKit\nfunc load() {\n    print(\"user \\(name)\")\n}\ninit(frame: CGRect) {}\n".to_string()),
            ("cmd/main.go", "package main\nimport \"fmt\"\nfunc main() {\n\tif true {\n\t\tif true {\n\t\t\tif true {\n\t\t\t\tfmt.Println(1)\n\t\t\t}\n\t\t}\n\t}\n}\n".to_string()),
            ("src/Main.kt", "import a.b\nfun main() {\n    val x = 1\n}\nfun other() {}\n".to_string()),
            ("web/app.ts", "import x from 'y';\nfunction f() {}\nconst g = async (a) => a;\nlet h = b => b;\n".to_string()),
            ("tools/run.py", "import os\n# comment\ndef main():\n    pass\n".to_string()),
            ("deploy/site.yml", format!("- hosts: all\n  vars:\n    password: {}\n", ["sample", "value", "42"].concat())),
            ("notes.unknownext", "just some text\nfn looks_like(x)\n".to_string()),
        ]
    }

    fn golden_render(path: &str, content: &str) -> String {
        let lang = crate::tools::commits::detect_language(path);
        let m = compute_file_metrics(path, content, lang);
        format!(
            "{}\n---\ntotal={} functions={} max_nesting={} violations={} score={} grade={} details={:?}\n",
            analyze_content(path, "main", content),
            m.total_lines, m.functions, m.max_nesting, m.violations, m.score, m.grade, m.violation_details
        )
    }

    #[test]
    fn golden_analysis_output_is_unchanged() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/lint");
        let update = std::env::var_os("UPDATE_GOLDEN").is_some();
        let mut drift = Vec::new();
        for (path, content) in golden_inputs() {
            let file = dir.join(format!("{}.txt", path.replace('/', "__")));
            let got = golden_render(path, &content);
            if update {
                std::fs::create_dir_all(&dir).unwrap();
                std::fs::write(&file, &got).unwrap();
                continue;
            }
            let want = std::fs::read_to_string(&file).unwrap_or_else(|_| panic!("missing golden {}", file.display()));
            if got != want {
                drift.push(format!("{path}\n--- want\n{want}\n--- got\n{got}"));
            }
        }
        assert!(drift.is_empty(), "analysis output changed:\n{}", drift.join("\n\n"));
    }

    #[test]
    fn function_patterns_all_compile() {
        // Forces the LazyLock: a bad constant pattern fails here, not in production.
        assert_eq!(FUNCTION_RES.len(), FUNCTION_PATTERNS.len());
        assert!(FALLBACK_FUNCTION_RE.is_match("def x"));
    }

    #[test]
    fn each_language_detects_its_own_declarations_only() {
        let cases: &[(&str, &str, &str)] = &[
            ("Swift", "func load()", "def load():"),
            ("Swift", "init(frame: x)", "fn load()"),
            ("Swift", "convenience init?(x: Int)", "let initial = 1"),
            ("PHP", "function handle()", "function ()"),
            ("Go", "func main()", "def main():"),
            ("Kotlin", "fun main()", "func main()"),
            ("Java", "fun main()", "def main()"),
            ("TypeScript", "const f = async (a) => a;", "const f = 1;"),
            ("JavaScript", "let h = b => b;", "def h():"),
            ("Rust", "fn main()", "def main():"),
            ("Python", "def main():", "fn main()"),
        ];
        for &(lang, yes, no) in cases {
            assert!(function_regex(lang).is_match(yes), "{lang} should detect {yes:?}");
            assert!(!function_regex(lang).is_match(no), "{lang} should not detect {no:?}");
        }
        assert!(function_regex("Unknown").is_match("fn x()"));
        assert!(function_regex("Unknown").is_match("function x()"));
    }

    #[test]
    fn import_prefix_per_language() {
        assert_eq!(import_prefix("PHP"), "use ");
        assert_eq!(import_prefix("Rust"), "use ");
        assert_eq!(import_prefix("Go"), "import");
        assert_eq!(import_prefix("Swift"), "import ");
        assert_eq!(import_prefix("Unknown"), "import ");
    }

    #[test]
    fn nesting_counts_a_tab_as_one_level() {
        assert_eq!(nesting_level("x"), 0);
        assert_eq!(nesting_level("   x"), 0, "three spaces is not a level");
        assert_eq!(nesting_level("    x"), 1);
        assert_eq!(nesting_level("\t\tx"), 2);
        assert_eq!(nesting_level("\t    x"), 2, "mixed tab and spaces");
    }

    #[test]
    fn ticket_reference_needs_two_to_ten_capitals() {
        assert!(has_ticket_ref("fix: PROJ-12 thing"));
        assert!(has_ticket_ref("AB-1"));
        assert!(!has_ticket_ref("A-1"), "single letter is not a project key");
        assert!(!has_ticket_ref("utf-8 handling"));
        assert!(!has_ticket_ref("no ticket"));
    }

    #[test]
    fn file_facts_boundaries() {
        let body = |n: usize| (0..n).map(|_| "  x").collect::<Vec<_>>().join("\n");
        // A function of exactly 50 lines is not long; 51 is.
        let fifty = format!("fn a() {{\n{}", body(49));
        let fifty_one = format!("fn a() {{\n{}", body(50));
        assert!(file_facts("a.rs", &fifty, "Rust").long_functions.is_empty());
        assert_eq!(file_facts("a.rs", &fifty_one, "Rust").long_functions.len(), 1);
        let f = file_facts("a.rs", "use a;\n\n// c\n                x", "Rust");
        assert_eq!((f.total_lines, f.blank_lines, f.comment_lines, f.code_lines), (4, 1, 1, 2));
        assert_eq!((f.imports, f.max_nesting, f.max_nesting_line, f.deep_lines), (1, 4, 4, 1));
        let empty = file_facts("a.rs", "", "Rust");
        assert_eq!(empty.score(), (100, Grade::A));
    }

    // ─── Rule files: the gate that makes a broken rule loud ───

    /// Every embedded rule source, as the binary sees it.
    const RULE_SOURCES: &[(&str, &str)] = &[
        ("global.toml", include_str!("../../rules/global.toml")),
        ("php.toml", include_str!("../../rules/php.toml")),
        ("kotlin.toml", include_str!("../../rules/kotlin.toml")),
        ("swift.toml", include_str!("../../rules/swift.toml")),
        ("go.toml", include_str!("../../rules/go.toml")),
        ("typescript.toml", include_str!("../../rules/typescript.toml")),
        ("ansible.toml", include_str!("../../rules/ansible.toml")),
    ];

    #[test]
    fn every_rule_file_parses_and_every_pattern_compiles() {
        // At runtime both failures are silent by design (a bad rule must not take the
        // server down): a TOML error empties a whole language, and a pattern the
        // linear-time engine rejects — lookaround, an unescaped `{` — disables the
        // rule, or, for a negative_pattern, removes its exclusion so it fires on
        // lines it was meant to skip. Rules are embedded at compile time, so this is
        // where they have to fail instead.
        let mut problems = Vec::new();
        for (file, src) in RULE_SOURCES {
            let parsed: RuleFile = match toml::from_str(src) {
                Ok(p) => p,
                Err(e) => {
                    problems.push(format!("{file}: TOML does not parse: {e}"));
                    continue;
                }
            };
            if parsed.rule.is_empty() {
                problems.push(format!("{file}: parses to zero rules"));
            }
            for r in &parsed.rule {
                for (field, pat) in [
                    ("pattern", &r.pattern),
                    ("negative_pattern", &r.negative_pattern),
                    ("negative_file_pattern", &r.negative_file_pattern),
                ] {
                    if !pat.is_empty() {
                        if let Err(e) = regex::Regex::new(pat) {
                            let why = e.to_string().lines().last().unwrap_or("").to_string();
                            problems.push(format!("{file} {} {field}: {why}", r.id));
                        }
                    }
                }
            }
        }
        assert!(problems.is_empty(), "broken rules:\n  {}", problems.join("\n  "));
    }

    #[test]
    fn rule_ids_are_unique_across_all_files() {
        let mut seen = std::collections::HashMap::new();
        for (file, src) in RULE_SOURCES {
            let parsed: RuleFile = toml::from_str(src).expect("parses");
            for r in parsed.rule {
                if let Some(prev) = seen.insert(r.id.clone(), *file) {
                    panic!("rule id {} defined in both {prev} and {file}", r.id);
                }
            }
        }
    }

    #[test]
    fn language_sets_include_the_global_rules_and_aliases_resolve() {
        let global = get_compiled_rules("global").len();
        assert!(global > 0);
        for lang in ["PHP", "Kotlin", "Swift", "Go", "TypeScript", "YAML/Ansible"] {
            assert!(get_compiled_rules(lang).len() > global, "{lang} must add to the global set");
        }
        assert_eq!(get_compiled_rules("Java").len(), get_compiled_rules("Kotlin").len());
        assert_eq!(get_compiled_rules("Vue").len(), get_compiled_rules("TypeScript").len());
        // An unknown language falls back to the global set rather than to nothing.
        assert_eq!(get_compiled_rules("Cobol").len(), global);
    }

    fn fires(id: &str, lang: &str, line: &str, path: &str) -> bool {
        get_compiled_rules(lang)
            .iter()
            .filter(|cr| cr.rule.id == id)
            .any(|cr| matches_compiled_rule(cr, line, path))
    }

    #[test]
    fn rewritten_rules_fire_where_they_should_and_not_where_they_should_not() {
        // G003 / PHP014 — these never fired before (lookahead).
        assert!(fires("G003", "global", "// TODO: handle retries", "a.rs"));
        assert!(!fires("G003", "global", "// TODO PROJ-12 handle retries", "a.rs"));
        assert!(fires("PHP014", "PHP", "// TODO clean this up", "a.php"));
        assert!(!fires("PHP014", "PHP", "// TODO APP-7 clean this up", "a.php"));
        // PHP009 — raw request input, unless validated on the same line.
        assert!(fires("PHP009", "PHP", "$x = $request->input('email');", "a.php"));
        assert!(!fires("PHP009", "PHP", "$x = $request->input('email')->validate();", "a.php"));
        // ANS001 — its exclusion was dead, so it fired on vaulted values too.
        let line = format!("password: {}", ["sample", "value", "42"].concat());
        assert!(fires("ANS001", "YAML/Ansible", &line, "x.yml"));
        assert!(!fires("ANS001", "YAML/Ansible", "password: \"{{ vault_db_password }}\"", "x.yml"));
        // SW017 — interpolation in a log call; never fired before (unclosed group).
        assert!(fires("SW017", "Swift", r#"print("user \(name) logged in")"#, "a.swift"));
        assert!(!fires("SW017", "Swift", r#"print("static message")"#, "a.swift"));
        // Magic numbers: a bare literal fires, an arithmetic expression does not.
        assert!(fires("G007", "global", "if retries > 4096 {", "a.rs"));
        assert!(!fires("G007", "global", "let buf = 1024 * 1024;", "a.rs"));
        assert!(!fires("G007", "global", "let pi = 3.14159;", "a.rs"));
    }

    #[test]
    fn a_rule_without_a_pattern_never_matches_and_negatives_exclude() {
        let rule = |pattern: &str, neg: &str, neg_file: &str| {
            CompiledRule::compile(Rule {
                id: "T1".into(),
                severity: RuleSeverity::Warning,
                name: "t".into(),
                pattern: pattern.into(),
                negative_pattern: neg.into(),
                negative_file_pattern: neg_file.into(),
                applies_to: String::new(),
                max_additions: 0,
                message: "m".into(),
            })
        };
        assert!(!matches_compiled_rule(&rule("", "", ""), "anything", "a.rs"));
        assert!(matches_compiled_rule(&rule("foo", "", ""), "a foo b", "a.rs"));
        assert!(!matches_compiled_rule(&rule("foo", "", ""), "a bar b", "a.rs"));
        assert!(!matches_compiled_rule(&rule("foo", "ok", ""), "foo ok", "a.rs"));
        assert!(!matches_compiled_rule(&rule("foo", "", r"_test\.rs$"), "foo", "x_test.rs"));
        assert!(matches_compiled_rule(&rule("foo", "", r"_test\.rs$"), "foo", "x.rs"));
    }

    // ─── Scoring: one decision, pinned at every boundary ───

    fn inputs() -> ScoreInputs {
        ScoreInputs {
            total_lines: 100,
            func_count: 5,
            max_nesting: 1,
            imports: 3,
            comment_lines: 10,
            code_lines: 80,
            long_funcs: 0,
            violations: 0,
        }
    }

    #[test]
    fn a_clean_file_scores_a_hundred() {
        assert_eq!(quality_score(&inputs()), (100, Grade::A));
    }

    #[test]
    fn length_penalty_is_tiered_not_cumulative() {
        // The drift bug: one copy charged a >500-line file both tiers (-30).
        let at = |n| quality_score(&ScoreInputs { total_lines: n, ..inputs() }).0;
        assert_eq!(at(300), 100);
        assert_eq!(at(301), 90);
        assert_eq!(at(500), 90);
        assert_eq!(at(501), 80, "very long is -20, not -20 and -10");
    }

    #[test]
    fn every_other_threshold_sits_exactly_where_documented() {
        let s = |m: ScoreInputs| quality_score(&m).0;
        assert_eq!(s(ScoreInputs { func_count: 20, ..inputs() }), 100);
        assert_eq!(s(ScoreInputs { func_count: 21, ..inputs() }), 85);
        assert_eq!(s(ScoreInputs { max_nesting: 3, ..inputs() }), 100);
        assert_eq!(s(ScoreInputs { max_nesting: 4, ..inputs() }), 90);
        assert_eq!(s(ScoreInputs { max_nesting: 5, ..inputs() }), 90);
        assert_eq!(s(ScoreInputs { max_nesting: 6, ..inputs() }), 80, "tiered, not -30");
        assert_eq!(s(ScoreInputs { imports: 15, ..inputs() }), 100);
        assert_eq!(s(ScoreInputs { imports: 16, ..inputs() }), 90);
        // Uncommented code is only penalised past 50 lines of it.
        assert_eq!(s(ScoreInputs { comment_lines: 0, code_lines: 50, ..inputs() }), 100);
        assert_eq!(s(ScoreInputs { comment_lines: 0, code_lines: 51, ..inputs() }), 95);
        assert_eq!(s(ScoreInputs { comment_lines: 1, code_lines: 51, ..inputs() }), 100);
        assert_eq!(s(ScoreInputs { long_funcs: 3, ..inputs() }), 85);
        // Violations cost one point each, capped at twenty.
        assert_eq!(s(ScoreInputs { violations: 7, ..inputs() }), 93);
        assert_eq!(s(ScoreInputs { violations: 20, ..inputs() }), 80);
        assert_eq!(s(ScoreInputs { violations: 500, ..inputs() }), 80);
    }

    #[test]
    fn the_score_floors_at_zero_and_grades_break_where_stated() {
        let worst = ScoreInputs {
            total_lines: 9000,
            func_count: 99,
            max_nesting: 9,
            imports: 99,
            comment_lines: 0,
            code_lines: 9000,
            long_funcs: 40,
            violations: 99,
        };
        assert_eq!(quality_score(&worst), (0, Grade::F));
        let grade = |v: usize| quality_score(&ScoreInputs { violations: v, ..inputs() }).1;
        assert_eq!(grade(10), Grade::A); // 90
        assert_eq!(grade(11), Grade::B); // 89
        // Grades below B need more than violations alone can remove (capped at 20).
        let g = |long| quality_score(&ScoreInputs { long_funcs: long, ..inputs() });
        assert_eq!(g(5), (75, Grade::B));
        assert_eq!(g(6), (70, Grade::C));
        assert_eq!(g(8), (60, Grade::C));
        assert_eq!(g(9), (55, Grade::D));
        assert_eq!(g(12), (40, Grade::D));
        assert_eq!(g(13), (35, Grade::F));
    }

    #[test]
    fn function_lengths_run_to_the_next_start_and_the_last_to_eof() {
        assert_eq!(function_lengths(&[], 10), Vec::<usize>::new());
        assert_eq!(function_lengths(&[0], 10), vec![10]);
        assert_eq!(function_lengths(&[0, 4, 9], 12), vec![4, 5, 3]);
    }

    // ─── The two analyses must agree ───

    fn score_in_report(report: &str) -> i32 {
        let tail = report.split("Quality Score: ").nth(1).expect("report has a score");
        tail.split('/').next().unwrap().trim().parse().expect("numeric score")
    }

    #[test]
    fn analyze_file_and_analyze_project_score_the_same_file_identically() {
        // Regression for the drift: the same file used to score differently depending
        // on which tool was asked. Exercise the paths where the copies disagreed —
        // a very long file, and a long function sitting at the end of the file.
        let body = |n: usize| (0..n).map(|i| format!("    let v{i} = {i};")).collect::<Vec<_>>().join("\n");
        let cases = [
            ("short.rs", format!("fn a() {{\n{}\n}}", body(10))),
            ("long.rs", format!("fn a() {{\n{}\n}}", body(600))),
            ("tail.rs", format!("fn a() {{}}\nfn b() {{\n{}\n}}", body(49))),
            ("mid.rs", format!("fn a() {{\n{}\n}}\nfn b() {{}}", body(320))),
        ];
        for (path, content) in &cases {
            let lang = crate::tools::commits::detect_language(path);
            let project = compute_file_metrics(path, content, lang).score;
            let file = score_in_report(&analyze_content(path, "main", content));
            assert_eq!(file, project, "{path}: analyze_file={file} analyze_project={project}");
        }
    }

    #[test]
    fn analyze_content_reports_metrics_violations_and_the_empty_case() {
        assert_eq!(analyze_content("e.rs", "main", ""), "**e.rs** — empty file");
        let src = "use std::io;\n// helper\nfn main() {\n    let token = \"abcdefghijklmnop\";\n}\n";
        let out = analyze_content("src/main.rs", "dev", src);
        assert!(out.contains("## src/main.rs"));
        assert!(out.contains("**Branch:** dev"));
        assert!(out.contains("| Total lines | 5 | OK |"));
        assert!(out.contains("| Functions | 1 | OK |"));
        assert!(out.contains("| Imports | 1 | OK |"));
        assert!(out.contains("Quality Score:"));
        let deep = format!("fn f() {{\n{}x\n}}", " ".repeat(24));
        assert!(analyze_content("d.rs", "m", &deep).contains("Consider flattening")
            || analyze_content("d.rs", "m", &deep).contains("Deeply nested"));
    }

    #[test]
    fn list_rules_groups_by_severity_and_dedups_across_languages() {
        let all = list_rules("");
        assert!(all.starts_with("**Rules: "));
        assert!(all.contains("CRITICAL"));
        let ids: Vec<&str> = all.lines().filter_map(|l| l.split("**[").nth(1)).map(|t| t.split(']').next().unwrap()).collect();
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(ids.len(), unique.len(), "a rule listed twice");
        assert!(list_rules("PHP").contains("PHP009"));
        assert!(!list_rules("Go").contains("PHP009"));
    }

    #[test]
    fn commit_message_conventions() {
        let ok = validate_commit_message("feat(api): add retries PROJ-12");
        assert!(ok.has_conventional_prefix && ok.has_ticket_ref && !ok.is_too_long);
        assert!(ok.failures.is_empty());

        let bad = validate_commit_message("updated stuff");
        assert!(!bad.has_conventional_prefix && !bad.has_ticket_ref);
        assert_eq!(bad.failures, vec!["no conventional prefix", "no ticket reference"]);

        // The limit is 72: 72 passes, 73 fails.
        let at = |n: usize| validate_commit_message(&format!("fix: {} PROJ-1", "x".repeat(n - 12)));
        assert_eq!(at(72).subject_length, 72);
        assert!(!at(72).is_too_long);
        assert!(at(73).is_too_long);
        assert!(at(73).failures.contains(&"subject >72 chars".to_string()));
        // Prefix match is case-insensitive; only the first line is the subject.
        assert!(validate_commit_message("FIX: thing ABC-1").has_conventional_prefix);
        assert_eq!(validate_commit_message("fix: a\n\nbody ABC-9").subject_length, 6);
        assert!(validate_commit_message("fix: a\n\nrefs ABC-9").has_ticket_ref, "ticket may live in the body");
    }

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
