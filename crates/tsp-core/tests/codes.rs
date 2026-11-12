//! Grep test: every code literal the Rust source emits is in `codes::CODES`.

use std::path::{Path, PathBuf};
use tsp_core::codes::{CODES, is_known};

/// Places a code literal appears: struct fields, refusal helpers, warnings.
const PREFIXES: &[&str] = &["code: \"", "refuse(\"", "warn(\"", "Refused { code: \""];

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let p = entry.unwrap().path();
        if p.is_dir() {
            rust_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn every_emitted_code_is_in_the_frozen_table() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut files = Vec::new();
    for dir in ["src", "../tsp-win/src", "../tsp-mac/src", "../../src-tauri/src"] {
        rust_files(&root.join(dir), &mut files);
    }
    let mut found = 0;
    for f in &files {
        let text = std::fs::read_to_string(f).unwrap();
        // Test modules may use made-up codes.
        let text = text.split("#[cfg(test)]").next().unwrap();
        for prefix in PREFIXES {
            for (i, _) in text.match_indices(prefix) {
                let rest = &text[i + prefix.len()..];
                let code = &rest[..rest.find('"').unwrap()];
                if code.is_empty() || code == "ok" || code == "cancelled" {
                    continue;
                }
                found += 1;
                assert!(is_known(code), "{}: code \"{code}\" is not in codes::CODES", f.display());
            }
        }
    }
    assert!(found > 20, "only {found} code literals found; did the patterns break?");
}

#[test]
fn codes_are_unique_kebab_case() {
    for (i, c) in CODES.iter().enumerate() {
        assert!(c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-') && c.contains('-'), "{c}");
        assert!(!CODES[i + 1..].contains(c), "duplicate {c}");
    }
}
