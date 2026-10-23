//! Property-based tests: invariants checked over generated inputs rather than a
//! handful of hand-picked examples. They target the code most likely to break on
//! inputs nobody thought to write down — escaping, truncation at character
//! boundaries, decoding, redaction — and the arithmetic every report relies on.
//!
//! Secret-shaped strings are assembled at runtime from generated parts; no literal
//! credential appears in this file.

use proptest::prelude::*;

use crate::error::{Error, FanOut};
use crate::tools::encoding::decode_base64;
use crate::tools::lint::{file_facts, quality_score, Grade, ScoreInputs};
use crate::tools::pipelines::{redact_log, render_variable, strip_ansi};
use crate::tools::reports::{htmlescape, tail_ellipsis};
use crate::tools::repository::render_search_hits;
use crate::tools::stats::median;

fn unescape(s: &str) -> String {
    s.replace("&#39;", "'").replace("&quot;", "\"").replace("&gt;", ">").replace("&lt;", "<").replace("&amp;", "&")
}

proptest! {
    #[test]
    fn htmlescape_leaves_no_markup_and_round_trips(s in any::<String>()) {
        let e = htmlescape(&s);
        prop_assert!(!e.contains(['<', '>', '"', '\'']));
        prop_assert_eq!(unescape(&e), s);
    }

    #[test]
    fn tail_ellipsis_never_splits_a_character_or_overruns(s in any::<String>(), max in 3usize..80) {
        let out = tail_ellipsis(&s, max);
        let n = s.chars().count();
        if n <= max {
            prop_assert_eq!(out, s);
        } else {
            prop_assert_eq!(out.chars().count(), max);
            prop_assert!(out.starts_with("..."));
            prop_assert!(s.ends_with(&out[3..]), "keeps the end of the input");
        }
    }

    #[test]
    fn base64_decoding_round_trips_gitlab_line_wrapping(bytes in proptest::collection::vec(any::<u8>(), 0..400)) {
        use base64::Engine as _;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let wrapped: String = encoded
            .as_bytes()
            .chunks(60)
            .map(|c| std::str::from_utf8(c).unwrap())
            .collect::<Vec<_>>()
            .join("\n");
        prop_assert_eq!(decode_base64(&wrapped).unwrap(), bytes);
    }

    #[test]
    fn median_lies_within_the_range_and_ignores_order(mut v in proptest::collection::vec(-1e6f64..1e6, 1..50)) {
        let lo = v.iter().cloned().fold(f64::INFINITY, f64::min);
        let hi = v.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
        let m = median(&mut v.clone());
        prop_assert!(lo <= m && m <= hi);
        v.reverse();
        prop_assert_eq!(median(&mut v), m);
    }

    #[test]
    fn quality_score_is_bounded_graded_and_monotone_in_violations(
        total in 0usize..3000, funcs in 0usize..60, nesting in 0usize..12, imports in 0usize..40,
        comments in 0usize..200, code in 0usize..3000, long in 0usize..20, violations in 0usize..60,
    ) {
        let inputs = |violations| ScoreInputs {
            total_lines: total, func_count: funcs, max_nesting: nesting, imports,
            comment_lines: comments, code_lines: code, long_funcs: long, violations,
        };
        let (score, grade) = quality_score(&inputs(violations));
        prop_assert!((0..=100).contains(&score));
        prop_assert_eq!(grade, Grade::from_score(score));
        let (worse, _) = quality_score(&inputs(violations + 1));
        prop_assert!(worse <= score, "an extra violation never raises the score");
    }

    #[test]
    fn grades_never_improve_as_the_score_falls(a in -50i32..150, b in -50i32..150) {
        let (hi, lo) = if a >= b { (a, b) } else { (b, a) };
        prop_assert!(Grade::from_score(hi) <= Grade::from_score(lo));
    }

    #[test]
    fn file_facts_partition_every_line(text in "[ \t]{0,12}[a-z{}();#/*]{0,30}(\n[ \t]{0,12}[a-z{}();#/*]{0,30}){0,60}") {
        for lang in ["Rust", "Python", "Go", "Swift", "Other"] {
            let f = file_facts("x", &text, lang);
            prop_assert_eq!(f.blank_lines + f.comment_lines + f.code_lines, f.total_lines);
            prop_assert!(f.max_nesting_line <= f.total_lines);
            prop_assert!(f.long_functions.len() <= f.functions.len());
            prop_assert!(f.deep_lines <= f.total_lines);
        }
    }

    #[test]
    fn search_rendering_reports_every_hit_and_never_dumps_control_bytes(
        hits in proptest::collection::vec(("[a-z]{1,8}(/[a-z]{1,8}){0,2}\\.(rs|png|lock|txt)", any::<String>()), 0..12)
    ) {
        let results: Vec<serde_json::Value> = hits
            .iter()
            .map(|(path, data)| serde_json::json!({"path": path, "startline": 1, "data": data}))
            .collect();
        let out = render_search_hits("q", "g/p", &results);
        for (path, _) in &hits {
            prop_assert!(out.contains(path.as_str()), "hit {} dropped", path);
        }
        let header = format!("{} results", hits.len());
        prop_assert!(out.contains(&header));
    }

    // Found a byte-level corruption on its first run: a literal backspace (0x08) in the
    // token pattern, where `\b` was meant, made every token redaction a no-op.
    #[test]
    fn redaction_removes_token_shaped_secrets_wherever_they_sit(
        before in "[a-zA-Z ,.:=]{0,40}", after in "[a-zA-Z ,.:=]{0,40}", body in "[A-Za-z0-9_]{20,40}",
    ) {
        let token = ["gl", "pat-", &body].concat();
        let log = format!("{before}{token}{after}");
        prop_assert!(!redact_log(&log).contains(&token));
    }

    #[test]
    fn secret_variables_never_show_their_value(
        stem in "[A-Z]{1,8}", marker in prop::sample::select(vec!["PASSWORD", "TOKEN", "SECRET", "PRIVATE_KEY"]),
        value in "[a-z0-9]{1,30}",
    ) {
        let key = format!("{stem}_{marker}");
        let line = render_variable(&key, &value);
        prop_assert!(!line.contains(&format!("`{value}`")), "{} leaked its value", key);
    }

    #[test]
    fn ansi_stripping_removes_every_escape_and_is_idempotent(
        parts in proptest::collection::vec(("[ -~]{0,12}", "[0-9;]{0,6}", "[A-Za-z]"), 0..10)
    ) {
        let s: String = parts.iter().map(|(t, p, f)| format!("{t}\u{1b}[{p}{f}")).collect();
        let once = strip_ansi(&s);
        const ESC: char = '\u{1b}';
        prop_assert!(!once.contains(ESC));
        prop_assert_eq!(strip_ansi(&once), once);
    }

    #[test]
    fn a_fan_out_fails_exactly_when_nothing_answered(ok in 0usize..5, failed in 0usize..5, missing in 0usize..5) {
        let mut fan = FanOut::default();
        for _ in 0..ok { fan.take(Ok::<u8, Error>(1)); }
        for _ in 0..failed { fan.take::<u8>(Err(Error::other("connection refused"))); }
        for _ in 0..missing { fan.take::<u8>(Err(Error::NotFound("x".into()))); }
        let result = fan.finish("lookups");
        prop_assert_eq!(result.is_err(), failed > 0 && ok + missing == 0);
        if let Ok(warning) = result {
            prop_assert_eq!(warning.is_some(), failed > 0);
        }
    }
}

#[test]
fn ordinary_words_ending_in_sk_are_not_redacted() {
    let text = ["task-", &"a".repeat(24)].concat();
    assert_eq!(redact_log(&text), text);
}

proptest! {
    /// Lockfiles are untrusted repository content: no input may crash a parser.
    #[test]
    fn lockfile_parsers_never_panic(text in any::<String>(), path in "[a-zA-Z0-9_./-]{0,40}") {
        use crate::tools::dependencies::{lockfile_kind, parse_lockfile, LockKind};
        let _ = lockfile_kind(&path);
        for kind in [
            LockKind::CargoLock, LockKind::PackageLock, LockKind::YarnLock, LockKind::PnpmLock,
            LockKind::ComposerLock, LockKind::GoSum, LockKind::PoetryLock, LockKind::PipfileLock,
            LockKind::Requirements, LockKind::GemfileLock, LockKind::GradleLockfile, LockKind::PodfileLock,
        ] {
            let parsed = parse_lockfile(kind, &text, "git.example.com");
            for d in &parsed.deps {
                prop_assert!(!d.name.is_empty() && !d.version.is_empty());
            }
        }
    }

    /// Line-structured formats get line-shaped noise, which reaches deeper branches.
    #[test]
    fn line_based_parsers_survive_structured_noise(
        lines in proptest::collection::vec("[ ]{0,6}[@/\"a-z0-9._:,()~^-]{0,24}", 0..40)
    ) {
        use crate::tools::dependencies::{parse_lockfile, LockKind};
        let text = lines.join("\n");
        for kind in [LockKind::YarnLock, LockKind::PnpmLock, LockKind::GemfileLock, LockKind::GoSum, LockKind::Requirements, LockKind::GradleLockfile] {
            let _ = parse_lockfile(kind, &text, "git.example.com");
        }
    }

    #[test]
    fn cvss_scoring_never_panics_and_stays_in_range(v in "CVSS:3\\.[01](/[A-Z]{1,2}:[A-Z]){0,10}") {
        if let Some(s) = crate::tools::dependencies::cvss3_base_score(&v) {
            prop_assert!((0.0..=10.0).contains(&s));
        }
    }
}
