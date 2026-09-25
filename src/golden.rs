//! Golden-master assertions for report renderers (test-only).
//!
//! A renderer's full output is compared byte-for-byte with a checked-in snapshot
//! under `tests/golden/`, so a refactor that changes the report in any way fails
//! loudly. Values that change on their own — today's date and the crate version —
//! are normalised first. Regenerate after an intended change with
//! `UPDATE_GOLDEN=1 cargo test --lib`, then review the snapshot diff.

use std::path::PathBuf;

fn normalise(s: &str) -> String {
    let today = chrono::Utc::now();
    s.replace(&today.format("%A, %d %B %Y").to_string(), "<TODAY>")
        .replace(&today.format("%Y-%m-%d").to_string(), "<TODAY>")
        .replace(concat!("v", env!("CARGO_PKG_VERSION")), "v<VERSION>")
}

#[track_caller]
pub(crate) fn assert_golden(name: &str, actual: &str) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "golden", name].iter().collect();
    let actual = normalise(actual);
    if std::env::var_os("UPDATE_GOLDEN").is_some() {
        std::fs::create_dir_all(path.parent().expect("golden path has a parent")).expect("create golden dir");
        std::fs::write(&path, &actual).expect("write golden");
        return;
    }
    let expected = std::fs::read_to_string(&path)
        .unwrap_or_else(|_| panic!("missing snapshot {} — run with UPDATE_GOLDEN=1", path.display()));
    if actual != expected {
        let line = actual.lines().zip(expected.lines()).position(|(a, e)| a != e).map_or(
            format!("length differs ({} vs {} lines)", actual.lines().count(), expected.lines().count()),
            |i| {
                format!(
                    "first difference at line {}:\n  want: {}\n  got:  {}",
                    i + 1,
                    expected.lines().nth(i).unwrap_or(""),
                    actual.lines().nth(i).unwrap_or("")
                )
            },
        );
        panic!("{name} differs from its snapshot — {line}");
    }
}
