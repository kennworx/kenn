use super::*;
use kenn_indexer::Workspace;
use tempfile::TempDir;

fn mk_config(csharp: bool, rust: bool, typescript: bool) -> Config {
    let mut c = Config::default();
    c.language.csharp.enabled = csharp;
    c.language.rust.enabled = rust;
    c.language.typescript.enabled = typescript;
    c
}

/// `should_skip_for_staleness` short-circuits with `force=true` or
/// when `git_aware=false`. Otherwise it consults
/// `decide_startup_state`. The fresh-workspace path returns false
/// (no live snapshot → no skip).
#[test]
fn should_skip_for_staleness_force_overrides() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(Layout::default_for(dir.path())).expect("store");
    // Empty workspace, no live: skip would be false anyway, but
    // force=true must always return false.
    assert!(!should_skip_for_staleness(
        true,
        &store,
        dir.path(),
        true,
        0
    ));
    assert!(!should_skip_for_staleness(
        true,
        &store,
        dir.path(),
        false,
        0
    ));
}

#[test]
fn should_skip_for_staleness_off_returns_false() {
    let dir = TempDir::new().unwrap();
    let store = Store::open(Layout::default_for(dir.path())).expect("store");
    assert!(!should_skip_for_staleness(
        false,
        &store,
        dir.path(),
        false,
        0
    ));
}

#[test]
fn should_skip_for_staleness_fresh_workspace_returns_false() {
    // No `live/` published yet → decide_startup_state returns
    // Reindex (its default), so should_skip is false.
    let dir = TempDir::new().unwrap();
    let store = Store::open(Layout::default_for(dir.path())).expect("store");
    assert!(!should_skip_for_staleness(
        false,
        &store,
        dir.path(),
        true,
        0
    ));
}

/// `configure_runner` adds one driver per enabled language. Cover
/// every combination (no need to inspect the resulting driver
/// internals — we trust `IndexerDriver`'s own tests; just verify
/// the function returns a value for every combo without panic).
#[test]
fn configure_runner_handles_every_language_combo() {
    let dir = TempDir::new().unwrap();
    let make_ws = || {
        Workspace::new(dir.path(), &[])
            .expect("ws")
            .with_test_globs(&[])
            .expect("test globs")
    };
    // All off. `drop` explicitly discards the `#[must_use]` driver — the
    // test only checks that construction doesn't panic.
    drop(configure_runner(make_ws(), &mk_config(false, false, false)));
    // Each one individually.
    drop(configure_runner(make_ws(), &mk_config(true, false, false)));
    drop(configure_runner(make_ws(), &mk_config(false, true, false)));
    drop(configure_runner(make_ws(), &mk_config(false, false, true)));
    // All on.
    drop(configure_runner(make_ws(), &mk_config(true, true, true)));
}

/// index-run-reporting — a failed language among healthy ones gets
/// exactly one summary line and one zero-files warning; healthy
/// languages are not mentioned.
#[test]
fn degraded_lines_name_only_affected_languages() {
    let mut rust = RunReport::started_for(
        kenn_model::Language::Rust,
        "rust-analyzer",
        "?",
        "crates/foo",
    );
    rust.status = RunStatus::Failed;
    rust.failed_projects
        .push("rust-analyzer exited Some(1): boom\nstderr tail".into());
    let mut md = RunReport::started("markdown", "?", "docs");
    md.files_seen = 3;
    let reports = [rust, md];

    let rollups = rollup_by_language(&reports);
    let lines = degraded_language_lines(&rollups);
    assert_eq!(lines.len(), 1);
    assert_eq!(
        lines[0],
        "warning: rust: failed — rust-analyzer exited Some(1): boom"
    );

    let warnings = zero_file_warnings(&rollups);
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].contains(".rs"), "got: {}", warnings[0]);
    assert!(warnings[0].starts_with("warning: rust indexed 0 files"));
}

/// The out-of-root tripwire fires only when documents were dropped AND none
/// survived — never on an honestly-empty run, and never when another
/// producer kept in-root documents.
#[test]
fn all_documents_outside_root_distinguishes_empty_from_all_dropped() {
    let mut dropped = RunReport::started_for(kenn_model::Language::Rust, "rust-analyzer", "?", "u");
    dropped.out_of_root_seen = 3;
    dropped.files_seen = 0;
    assert_eq!(
        all_documents_outside_root(std::slice::from_ref(&dropped)),
        Some(3),
        "drops>0 && kept==0 fires with the dropped count"
    );

    let mut partial = dropped.clone();
    partial.files_seen = 5;
    assert_eq!(
        all_documents_outside_root(&[partial]),
        None,
        "some in-root docs survived ⇒ partial, not failure"
    );

    let mut md = RunReport::started("markdown", "?", "docs");
    md.files_seen = 8;
    assert_eq!(
        all_documents_outside_root(&[dropped.clone(), md]),
        None,
        "another producer kept docs ⇒ run is not empty"
    );

    let empty = RunReport::started("markdown", "?", "docs");
    assert_eq!(
        all_documents_outside_root(&[empty]),
        None,
        "no drops (empty repo) ⇒ never caught by the tripwire"
    );

    // The warning labels by language ("rust"), matching sibling warnings,
    // not by the producer brand ("rust-analyzer").
    let warnings = out_of_root_warnings(std::slice::from_ref(&dropped));
    assert_eq!(warnings.len(), 1);
    assert_eq!(
        warnings[0],
        "warning: rust: 3 document(s) fell outside the workspace root"
    );
}

/// A clean run prints no summary lines, and a `Success` 0-files
/// language (JSONL producers always report once, even with no
/// sources in the workspace) triggers no zero-files warning.
#[test]
fn clean_run_prints_nothing() {
    let swift = RunReport::started_for(
        kenn_model::Language::Swift,
        "kenn-swift",
        "?",
        "kenn-swift@/ws",
    );
    let rollups = rollup_by_language(std::slice::from_ref(&swift));
    assert!(degraded_language_lines(&rollups).is_empty());
    assert!(zero_file_warnings(&rollups).is_empty());
}

/// A Partial language that still produced files gets a summary line
/// (with the `+N more` failure tail) but no zero-files warning.
#[test]
fn partial_with_files_warns_status_only() {
    let mut swift = RunReport::started_for(
        kenn_model::Language::Swift,
        "kenn-swift",
        "?",
        "kenn-swift@/ws",
    );
    swift.status = RunStatus::Partial;
    swift
        .failed_projects
        .push("build: /ws/Pkg: `swift build` failed".into());
    swift
        .failed_projects
        .push("store: /ws/Other: no store".into());
    swift.files_seen = 12;
    let reports = [swift];

    let rollups = rollup_by_language(&reports);
    let lines = degraded_language_lines(&rollups);
    assert_eq!(
        lines,
        vec![
            "warning: swift: partial — build: /ws/Pkg: `swift build` failed (+1 more)".to_string()
        ]
    );
    assert!(zero_file_warnings(&rollups).is_empty());
}

/// One language's branded per-unit success and its language-id failure
/// report (discover/finalize paths) roll up together: one summary line,
/// and no false zero-files warning when the language did index files.
#[test]
fn branded_and_language_id_reports_roll_up_together() {
    let mut ok = RunReport::started_for(
        kenn_model::Language::Rust,
        "rust-analyzer",
        "?",
        "crates/foo",
    );
    ok.files_seen = 100;
    let mut failed = RunReport::started("rust", "?", "<finalize>");
    failed.status = RunStatus::Failed;
    failed.failed_projects.push("ingest: io error".into());
    let reports = [ok, failed];

    let rollups = rollup_by_language(&reports);
    assert_eq!(rollups.len(), 1, "both names collapse onto `rust`");
    let lines = degraded_language_lines(&rollups);
    assert_eq!(lines.len(), 1, "one line per language: {lines:?}");
    assert!(lines[0].starts_with("warning: rust: failed"));
    assert!(
        zero_file_warnings(&rollups).is_empty(),
        "100 files indexed — no false zero-files warning"
    );
}

/// Producer warnings print their own per-language line — independent
/// of status, because they exist for degradations that keep the unit
/// `Success` (stale store units kept on a trusted read).
#[test]
fn producer_warnings_print_even_on_success() {
    let mut swift = RunReport::started_for(
        kenn_model::Language::Swift,
        "kenn-swift",
        "?",
        "kenn-swift@/ws",
    );
    swift.files_seen = 10;
    swift
        .warnings
        .push("store: /ws: 3 unit(s) older than their sources (kept: store trusted)".into());
    swift.warnings_overflow = 2;
    let reports = [swift];

    let rollups = rollup_by_language(&reports);
    assert!(
        degraded_language_lines(&rollups).is_empty(),
        "status is Success"
    );
    let lines = producer_warning_lines(&rollups);
    assert_eq!(
        lines,
        vec![
            "warning: swift: store: /ws: 3 unit(s) older than their sources \
                 (kept: store trusted) (+2 more)"
                .to_string()
        ]
    );
}

/// Structured overflow counts toward the summary's `+N more` figure.
#[test]
fn rollup_counts_structured_overflow() {
    let mut r = RunReport::started("csharp", "?", "u");
    r.status = RunStatus::Partial;
    r.failed_projects.push("msbuild: a.sln: load failed".into());
    r.failed_overflow = 8;
    let reports = [r];

    let lines = degraded_language_lines(&rollup_by_language(&reports));
    assert_eq!(
        lines,
        vec!["warning: csharp: partial — msbuild: a.sln: load failed (+8 more)".to_string()]
    );
}

#[test]
fn any_language_enabled_reflects_each_language() {
    assert!(!any_language_enabled(&Config::default()));
    let mut c = Config::default();
    c.language.go.enabled = true;
    assert!(any_language_enabled(&c));
    let mut c = Config::default();
    c.language.rust.enabled = true;
    assert!(any_language_enabled(&c));
}
