//! Regression for `index-producer-parity`: workspace construction is built by a
//! single shared `build_workspace`, called by both the CLI (`kenn index`) and
//! the workflow / MCP `index_workspace` path.
//!
//! Previously the two paths had duplicate workspace builders and the workflow
//! copy dropped the **Swift** language-excludes, so an MCP-triggered index
//! indexed Swift sources the CLI excluded. This asserts the shared builder
//! applies the Swift excludes (the dropped one) — using `is_excluded`, so no
//! Swift toolchain is needed.

use kenn_config::Config;
use kenn_model::Language;
use tempfile::TempDir;

#[test]
fn build_workspace_applies_swift_language_excludes() {
    let dir = TempDir::new().unwrap();
    std::fs::write(
        dir.path().join("kenn.toml"),
        "[language.swift]\nenabled = true\nexcludes = [\"Vendor/**\"]\n",
    )
    .unwrap();
    let config = Config::load_from_path(&dir.path().join("kenn.toml")).unwrap();

    let ws = kenn_indexer::build_workspace(dir.path(), &config).expect("build_workspace");

    // The drift the fix closes: Swift excludes must be applied on every path.
    assert!(
        ws.is_excluded(Language::Swift, "Vendor/Networking.swift"),
        "shared build_workspace must apply the Swift language-excludes",
    );
    // A non-excluded Swift path is still included.
    assert!(!ws.is_excluded(Language::Swift, "Sources/App.swift"));
}
