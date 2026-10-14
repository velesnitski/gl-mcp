//! Byte-level hygiene of the sources.
//!
//! A literal backspace once sat inside a regex where `\b` was meant: invisible in an
//! editor and in a diff, and it silently disabled token redaction in job logs for
//! several releases. Control characters have no business in source, rule files or
//! fixtures, so their presence fails the build.

use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap().flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, out);
        } else if matches!(
            path.extension().and_then(|e| e.to_str()),
            Some("rs" | "toml" | "md" | "json" | "yml" | "yaml")
        ) {
            out.push(path);
        }
    }
}

#[test]
fn no_control_characters_in_sources() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["src", "tests", "rules", "docs", ".github"] {
        if root.join(dir).exists() {
            walk(&root.join(dir), &mut files);
        }
    }
    files.push(root.join("Cargo.toml"));
    let mut hits = Vec::new();
    for f in &files {
        let bytes = std::fs::read(f).unwrap();
        for (i, b) in bytes.iter().enumerate() {
            if *b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r') {
                let line = bytes[..i].iter().filter(|c| **c == b'\n').count() + 1;
                hits.push(format!("{}:{line}: 0x{b:02x}", f.strip_prefix(root).unwrap().display()));
            }
        }
    }
    assert!(hits.is_empty(), "control characters in source:\n{}", hits.join("\n"));
}
