//! Regression guard for `store-layout`: every store path must come from
//! a `Layout` accessor. No component may join a store path segment —
//! `.kenn`, `local/`, `snapshots/`, `vectors/`, `findings/`, `index.lock`,
//! `scip-*.scip` — on its own, outside the layout module.

use std::path::{Path, PathBuf};

/// Literal join expressions that would hardcode a store path segment.
/// `scip-{` catches the `scip-{slug}.scip` filename construction without
/// flagging unrelated `scip-typescript` identifier strings.
const FORBIDDEN: &[&str] = &[
    r#".join(".kenn")"#,
    r#".join("local")"#,
    r#".join("snapshots")"#,
    r#".join("vectors")"#,
    r#".join("findings")"#,
    r#".join("index.lock")"#,
    "scip-{",
];

/// The workspace `crates/` directory — `CARGO_MANIFEST_DIR` is
/// `.../crates/kenn-store`.
fn crates_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates dir")
        .to_path_buf()
}

/// Collect every `.rs` file under `dir`, recursively.
fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read_dir").flatten() {
        let p = entry.path();
        if p.is_dir() {
            rs_files(&p, out);
        } else if p.extension().is_some_and(|e| e == "rs") {
            out.push(p);
        }
    }
}

#[test]
fn no_store_path_segment_joined_outside_the_layout_module() {
    let crates = crates_dir();
    let layout_dir = crates.join("kenn-store").join("src").join("layout");

    let mut files = Vec::new();
    for entry in std::fs::read_dir(&crates).unwrap().flatten() {
        let src = entry.path().join("src");
        if src.is_dir() {
            rs_files(&src, &mut files);
        }
    }

    let mut violations = Vec::new();
    for file in files {
        if file.starts_with(&layout_dir) {
            continue; // the layout module is the one allowed place.
        }
        // Dedicated test submodule files — `tests.rs`, or the sibling
        // `<name>_tests.rs` a split-out module uses (`#[cfg(test)] #[path =
        // "<name>_tests.rs"] mod tests;` in its parent) — are test-only, the
        // file-based equivalent of the inline `#[cfg(test)]` exclusion below.
        if file
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(|n| n == "tests.rs" || n.ends_with("_tests.rs"))
        {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        // Scan production code only — drop the trailing `#[cfg(test)]`
        // module that files in this workspace conventionally append.
        let prod = text.split("#[cfg(test)]").next().unwrap_or(&text);
        for needle in FORBIDDEN {
            if prod.contains(needle) {
                violations.push(format!("{}  →  {needle}", file.display()));
            }
        }
    }

    assert!(
        violations.is_empty(),
        "store path segments hardcoded outside `layout/` — route them through a `Layout` accessor:\n{}",
        violations.join("\n"),
    );
}
