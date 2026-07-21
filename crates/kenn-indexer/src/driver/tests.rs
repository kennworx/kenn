use super::*;

use std::path::PathBuf;

use crate::canonicalize::Workspace;
use crate::report::{RunReport, RunStatus};

#[test]
fn kenn_dotnet_resolves_sln_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("MyApp.sln"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("nested/deep")).unwrap();
    std::fs::write(dir.path().join("nested/deep/Other.sln"), "").unwrap();

    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = KennDotnet::default();
    let projects = driver.resolve_projects(&ws).unwrap();
    let names: Vec<_> = projects
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    assert!(names.iter().any(|n| n == "MyApp.sln"));
    assert!(names.iter().any(|n| n == "Other.sln"));
    assert_eq!(projects.len(), 2);
}

#[test]
fn kenn_dotnet_falls_back_to_csproj() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Solo.csproj"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = KennDotnet::default();
    let projects = driver.resolve_projects(&ws).unwrap();
    assert_eq!(projects.len(), 1);
    assert!(projects[0].file_name().unwrap() == "Solo.csproj");
}

/// A `.slnx`-only repo (Newtonsoft.Json) must resolve to the SOLUTION, not to
/// its loose `.csproj`s. Passing the nested csprojs made the sidecar run a bare
/// `dotnet restore` from the workspace root, which fails — 0 files indexed.
#[test]
fn kenn_dotnet_prefers_slnx_over_loose_csproj() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("Src/Lib")).unwrap();
    std::fs::write(dir.path().join("Src/App.slnx"), "").unwrap();
    std::fs::write(dir.path().join("Src/Lib/Lib.csproj"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let projects = KennDotnet::default().resolve_projects(&ws).unwrap();
    assert_eq!(
        projects.len(),
        1,
        "the solution, not the csproj: {projects:?}"
    );
    assert_eq!(projects[0].extension().unwrap(), "slnx");
}

/// Discovery parity: explicit `projects` list and walk-based fallback
/// produce the same set of `.sln` paths.
#[test]
fn kenn_dotnet_discovery_parity() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("A.sln"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("nested")).unwrap();
    std::fs::write(dir.path().join("nested/B.sln"), "").unwrap();

    let ws = Workspace::new(dir.path(), &[]).unwrap();

    let auto = KennDotnet::default();
    let mut auto_paths = auto.resolve_projects(&ws).unwrap();
    auto_paths.sort();

    let explicit = KennDotnet {
        projects: vec![PathBuf::from("A.sln"), PathBuf::from("nested/B.sln")],
        ..Default::default()
    };
    let mut explicit_paths = explicit.resolve_projects(&ws).unwrap();
    explicit_paths.sort();

    assert_eq!(auto_paths, explicit_paths);
}

#[test]
fn rust_analyzer_discovers_one_unit_when_cargo_toml_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = RustAnalyzer::default();
    let units = driver.discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1);
    // The workspace-root crate keeps the bare `rust` slug.
    assert_eq!(units[0].identifier, "rust");
    assert_eq!(units[0].path, ws.root());
}

#[test]
fn rust_analyzer_discovers_no_units_without_cargo_toml() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    assert!(RustAnalyzer::default()
        .discover_units(&ws)
        .unwrap()
        .is_empty());
}

/// #4: a crate NESTED in a repo with no root `Cargo.toml` (e.g. a polyglot
/// monorepo) must be indexed, not silently skipped. This is the regression the
/// old root-only `discover_units` had — it returned zero units + no report.
#[test]
fn rust_analyzer_discovers_nested_standalone_crate() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("rust-crate/src")).unwrap();
    std::fs::write(
        dir.path().join("rust-crate/Cargo.toml"),
        "[package]\nname = \"geo\"\nversion = \"0.0.0\"\n",
    )
    .unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let units = RustAnalyzer::default().discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1, "nested crate must yield a unit");
    assert_eq!(units[0].path, ws.root().join("rust-crate"));
    assert_eq!(units[0].identifier, "rust-rust-crate");
}

/// A workspace root plus its member crates is ONE RA run (RA loads the whole
/// graph); members must not each spawn a redundant, overlapping invocation.
#[test]
fn rust_analyzer_folds_workspace_members_into_root() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[workspace]\nmembers = [\"crates/foo\"]\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.path().join("crates/foo/src")).unwrap();
    std::fs::write(
        dir.path().join("crates/foo/Cargo.toml"),
        "[package]\nname = \"foo\"\nversion = \"0\"\n",
    )
    .unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let units = RustAnalyzer::default().discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1, "member folded into the root workspace run");
    assert_eq!(units[0].path, ws.root());
}

/// Two independent crates with no covering workspace → one RA run each.
#[test]
fn rust_analyzer_discovers_multiple_independent_crates() {
    let dir = tempfile::tempdir().unwrap();
    for name in ["alpha", "beta"] {
        std::fs::create_dir_all(dir.path().join(name)).unwrap();
        std::fs::write(
            dir.path().join(name).join("Cargo.toml"),
            format!("[package]\nname = \"{name}\"\nversion = \"0\"\n"),
        )
        .unwrap();
    }
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let units = RustAnalyzer::default().discover_units(&ws).unwrap();
    assert_eq!(units.len(), 2, "one unit per independent crate");
}

#[test]
fn rust_analyzer_returns_unavailable_when_binary_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Cargo.toml"), "[workspace]\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = RustAnalyzer {
        command: vec!["/nonexistent/rust-analyzer-xyz".into()],
        ..Default::default()
    };
    let units = driver.discover_units(&ws).unwrap();
    let outcome = driver.run_unit(&units[0], &ws).unwrap();
    assert!(matches!(outcome, ScipOutcome::Unavailable { .. }));
}

#[test]
fn kenn_dotnet_returns_unavailable_when_binary_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("MyApp.sln"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = KennDotnet {
        command: vec!["/nonexistent/kenn-dotnet-binary-xyz".into()],
        skip_restore: true,
        projects: Vec::new(),
        test_globs: Vec::new(),
        test_assembly_regexes: Vec::new(),
        provision_sdk: false,
    };
    let outcome = driver.run(&ws).unwrap();
    assert!(matches!(outcome, JsonlOutcome::Unavailable { .. }));
}

#[test]
fn kenn_dotnet_restores_by_default() {
    // `--skip-restore` silently unbinds every NuGet type: package types degrade
    // to bare syntactic names while the run still exits 0 with symbols and no
    // diagnostic. That only looked harmless because a dev machine's `obj/` is
    // already restored by its own builds; a fresh container or clean CI checkout
    // has none. Wired from `[language.csharp] restore`.
    assert!(
        !KennDotnet::default().skip_restore,
        "the default must restore, or NuGet types never bind"
    );
}

#[test]
fn kenn_swift_passes_through_explicit_projects_defers_discovery() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Package.swift"), "").unwrap();
    std::fs::create_dir_all(dir.path().join("App.xcodeproj")).unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();

    // No configured projects → empty (the sidecar discovers both SwiftPM and
    // Xcode projects itself; the Rust file-walk can't see `.xcodeproj` bundles).
    assert!(KennSwift::default()
        .resolve_projects(&ws)
        .unwrap()
        .is_empty());

    // Explicit projects (Package.swift or .xcodeproj) pass through, resolved to
    // absolute paths; a missing one errors so a stale `kenn.toml` is noticed.
    let explicit = KennSwift {
        projects: vec![
            PathBuf::from("Package.swift"),
            PathBuf::from("App.xcodeproj"),
        ],
        ..Default::default()
    };
    assert_eq!(explicit.resolve_projects(&ws).unwrap().len(), 2);

    let missing = KennSwift {
        projects: vec![PathBuf::from("Nope/Package.swift")],
        ..Default::default()
    };
    assert!(matches!(
        missing.resolve_projects(&ws),
        Err(DriverError::Subprocess(_))
    ));
}

#[test]
fn kenn_swift_returns_unavailable_when_binary_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("Package.swift"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = KennSwift {
        command: vec!["/nonexistent/kenn-swift-binary-xyz".into()],
        skip_build: true,
        projects: Vec::new(),
        platform: None,
    };
    let outcome = driver.run(&ws).unwrap();
    assert!(matches!(outcome, JsonlOutcome::Unavailable { .. }));
}

#[test]
fn kenn_ts_returns_unavailable_when_binary_missing() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = KennTs {
        command: vec!["/nonexistent/kenn-ts-binary-xyz".into()],
        projects: Vec::new(),
    };
    let outcome = driver.run(&ws).unwrap();
    assert!(matches!(outcome, JsonlOutcome::Unavailable { .. }));
}

#[test]
fn scip_python_discovers_one_unit_when_py_file_present() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.py"), "x = 1\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let units = ScipPython::default().discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].identifier, "python-0");
    assert_eq!(units[0].path, ws.root());
}

#[test]
fn scip_python_targets_emit_one_unit_per_entry() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/api")).unwrap();
    std::fs::create_dir_all(dir.path().join("src/worker")).unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = ScipPython {
        targets: vec!["src/api".into(), "src/worker".into()],
        ..Default::default()
    };
    let units = driver.discover_units(&ws).unwrap();
    assert_eq!(units.len(), 2);
    assert_eq!(units[0].identifier, "python-0");
    assert_eq!(units[0].path, ws.root().join("src/api"));
    assert_eq!(units[1].identifier, "python-1");
    assert_eq!(units[1].path, ws.root().join("src/worker"));
}

#[test]
fn scip_python_single_target_emits_one_unit() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("src/api")).unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = ScipPython {
        targets: vec!["src/api".into()],
        ..Default::default()
    };
    let units = driver.discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1);
    assert_eq!(units[0].identifier, "python-0");
    assert_eq!(units[0].path, ws.root().join("src/api"));
}

#[test]
fn scip_python_missing_target_fails_discovery() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = ScipPython {
        targets: vec!["missing".into()],
        ..Default::default()
    };
    let err = driver.discover_units(&ws).unwrap_err();
    match err {
        DriverError::Subprocess(msg) => {
            assert!(msg.contains("missing"), "msg={msg}");
        }
        other => panic!("expected Subprocess, got {other:?}"),
    }
}

#[test]
fn scip_python_discovers_no_units_without_py_files() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("README.md"), "# nope\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let units = ScipPython::default().discover_units(&ws).unwrap();
    assert!(units.is_empty());
}

/// `.py` files only under Python's per-language exclude dirs MUST
/// NOT trigger a unit. The patterns come from
/// `kenn_config::PythonConfig::DEFAULT_EXCLUDES` attached via
/// `with_language_excludes(Language::Python, ...)`.
#[test]
fn scip_python_skips_py_files_in_python_excluded_dirs() {
    use kenn_model::Language;
    let dir = tempfile::tempdir().unwrap();
    for excluded in [".venv", "__pycache__"] {
        std::fs::create_dir_all(dir.path().join(excluded)).unwrap();
        std::fs::write(dir.path().join(excluded).join("ignored.py"), "x = 1\n").unwrap();
    }
    let python_defaults: Vec<String> = kenn_config::PythonConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let ws = Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &python_defaults)
        .unwrap();
    let units = ScipPython::default().discover_units(&ws).unwrap();
    assert!(units.is_empty(), "got units: {units:?}");
}

/// `walk_for_language` MUST prune at directory recursion time —
/// the iterator never yields paths under an excluded directory and
/// never calls `read_dir` on it. We can't directly observe
/// `read_dir` calls without instrumentation, but writing many
/// files under `.venv/` and asserting they don't appear (plus the
/// `src/` file does) is a tight proxy: if the walker were
/// recursing-then-filtering at the file level via
/// `walk_skipping`'s `dir_skip` closure, the produced iterator
/// would still drop them — but the closure rejection happens
/// BEFORE `read_dir`, which is the contract we want to lock.
#[test]
fn walk_for_language_does_not_recurse_into_excluded_dir() {
    use kenn_model::Language;
    let dir = tempfile::tempdir().unwrap();
    // Populate `.venv/` with several files to make a "fully walked"
    // outcome distinguishable from "pruned at the root".
    let venv = dir.path().join(".venv/lib/site-packages");
    std::fs::create_dir_all(&venv).unwrap();
    for name in ["a.py", "b.py", "c.py"] {
        std::fs::write(venv.join(name), "").unwrap();
    }
    // An in-scope file the iterator MUST yield.
    std::fs::create_dir_all(dir.path().join("src")).unwrap();
    std::fs::write(dir.path().join("src/main.py"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &[".venv/**".to_string()])
        .unwrap();
    let yielded: Vec<_> = walk_for_language(&ws, Language::Python)
        .filter_map(Result::ok)
        .collect();
    assert!(
        yielded.iter().any(|p| p.ends_with("src/main.py")),
        "in-scope file must be yielded",
    );
    assert!(
        !yielded.iter().any(|p| p
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(".venv"))),
        "yielded paths inside .venv/ — pruning failed: {yielded:?}",
    );
}

/// Calling `walk_for_language` for a DIFFERENT language does not
/// prune the first language's excluded dirs — proves per-language
/// scoping at the walker level.
#[test]
fn walk_for_language_other_language_does_not_prune_python_excludes() {
    use kenn_model::Language;
    let dir = tempfile::tempdir().unwrap();
    let venv = dir.path().join(".venv");
    std::fs::create_dir_all(&venv).unwrap();
    std::fs::write(venv.join("ignored.cs"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Python, &[".venv/**".to_string()])
        .unwrap();
    // No C# excludes attached; the C# walker walks `.venv/` normally.
    let yielded: Vec<_> = walk_for_language(&ws, Language::Csharp)
        .filter_map(Result::ok)
        .collect();
    assert!(
        yielded.iter().any(|p| p.ends_with(".venv/ignored.cs")),
        "C# walker pruned Python's exclude — leak detected. Yielded: {yielded:?}",
    );
}

/// Workspace-level excludes (e.g., `.git/**`) prune for every
/// language — they're the cross-language gate.
#[test]
fn walk_for_language_prunes_workspace_excluded_dir() {
    use kenn_model::Language;
    let dir = tempfile::tempdir().unwrap();
    let git_objs = dir.path().join(".git/objects");
    std::fs::create_dir_all(&git_objs).unwrap();
    std::fs::write(git_objs.join("abc"), "").unwrap();
    std::fs::write(dir.path().join("real.py"), "").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let yielded: Vec<_> = walk_for_language(&ws, Language::Python)
        .filter_map(Result::ok)
        .collect();
    assert!(yielded.iter().any(|p| p.ends_with("real.py")));
    assert!(
        !yielded.iter().any(|p| p
            .components()
            .any(|c| c.as_os_str() == std::ffi::OsStr::new(".git"))),
        ".git/ leaked into walker output: {yielded:?}",
    );
}

#[test]
fn scip_python_returns_unavailable_when_binary_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("hello.py"), "x = 1\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = ScipPython {
        command: vec!["/nonexistent/scip-python-xyz".into()],
        ..Default::default()
    };
    let units = driver.discover_units(&ws).unwrap();
    let outcome = driver.run_unit(&units[0], &ws).unwrap();
    assert!(matches!(outcome, ScipOutcome::Unavailable { .. }));
}

struct StubScipDriver(&'static str);
impl ScipDriver for StubScipDriver {
    fn language_id(&self) -> &str {
        self.0
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
        Ok(vec![Unit {
            identifier: format!("{}.unit", self.0),
            path: PathBuf::from("/tmp/stub"),
        }])
    }
    fn run_unit(&self, unit: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
        let mut r = RunReport::started(self.0, "stub", &unit.identifier);
        r.symbols_seen = 7;
        r.finalize();
        Ok(ScipOutcome::Scip {
            path: PathBuf::from("/dev/null"),
            report: r,
        })
    }
}

struct StubJsonlIndexer(&'static str);
impl JsonlIndexer for StubJsonlIndexer {
    fn language_id(&self) -> &str {
        self.0
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn run(&self, _: &Workspace) -> Result<JsonlOutcome, DriverError> {
        let mut r = RunReport::started(self.0, "stub", &format!("{}@stub", self.0));
        r.symbols_seen = 11;
        r.finalize();
        Ok(JsonlOutcome::Unavailable { report: r })
    }
}

#[test]
fn orchestrator_collects_reports_across_drivers() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let reports = IndexerDriver::new(ws)
        .with_scip_driver(StubScipDriver("csharp"))
        .with_scip_driver(StubScipDriver("rust"))
        .with_jsonl_driver(StubJsonlIndexer("dotnet"))
        .run_all();
    assert_eq!(reports.len(), 3);
    assert!(reports
        .iter()
        .any(|r| r.indexer_name == "csharp" && r.symbols_seen == 7));
    assert!(reports.iter().any(|r| r.indexer_name == "rust"));
    assert!(reports
        .iter()
        .any(|r| r.indexer_name == "dotnet" && r.symbols_seen == 11));
}

/// `run_all` records a failure report when `discover_units` errors,
/// without aborting the rest of the loop.
struct DiscoverFailScipDriver(&'static str);
impl ScipDriver for DiscoverFailScipDriver {
    fn language_id(&self) -> &str {
        self.0
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
        Err(DriverError::Subprocess("synthetic discover failure".into()))
    }
    fn run_unit(&self, _: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
        // Never reached because discover_units returns Err first.
        // Return a synthetic error rather than `unreachable!` so the
        // clippy `unreachable` lint stays clean for the workspace.
        Err(DriverError::Subprocess("run_unit not expected".into()))
    }
}

#[test]
fn run_all_records_discover_failure_without_aborting() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let reports = IndexerDriver::new(ws)
        .with_scip_driver(DiscoverFailScipDriver("badlang"))
        .with_scip_driver(StubScipDriver("rust"))
        .run_all();
    assert_eq!(reports.len(), 2);
    let bad = reports
        .iter()
        .find(|r| r.indexer_name == "badlang")
        .expect("badlang report");
    assert_eq!(bad.status, RunStatus::Failed);
    assert!(bad.failed_projects.iter().any(|s| s.contains("discover")));
    // Subsequent driver still runs.
    assert!(reports.iter().any(|r| r.indexer_name == "rust"));
}

/// `run_all` records a failure report when `run_unit` errors, and
/// continues into the next unit / driver.
struct UnitFailScipDriver(&'static str);
impl ScipDriver for UnitFailScipDriver {
    fn language_id(&self) -> &str {
        self.0
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn discover_units(&self, _: &Workspace) -> Result<Vec<Unit>, DriverError> {
        Ok(vec![
            Unit {
                identifier: "u1".into(),
                path: PathBuf::from("/tmp/u1"),
            },
            Unit {
                identifier: "u2".into(),
                path: PathBuf::from("/tmp/u2"),
            },
        ])
    }
    fn run_unit(&self, unit: &Unit, _: &Workspace) -> Result<ScipOutcome, DriverError> {
        if unit.identifier == "u1" {
            Err(DriverError::Subprocess("synthetic unit failure".into()))
        } else {
            let mut r = RunReport::started(self.0, "stub", &unit.identifier);
            r.finalize();
            Ok(ScipOutcome::Scip {
                path: PathBuf::from("/dev/null"),
                report: r,
            })
        }
    }
}

#[test]
fn run_all_records_run_unit_failure_then_continues() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let reports = IndexerDriver::new(ws)
        .with_scip_driver(UnitFailScipDriver("ts"))
        .run_all();
    assert_eq!(reports.len(), 2);
    assert!(reports
        .iter()
        .any(|r| r.indexer_name == "ts" && r.status == RunStatus::Failed));
    assert!(reports
        .iter()
        .any(|r| r.indexer_name == "ts" && r.status != RunStatus::Failed));
}

/// `run_all` records a failure report when a JSONL indexer's
/// `run` errors.
struct JsonlFailIndexer(&'static str);
impl JsonlIndexer for JsonlFailIndexer {
    fn language_id(&self) -> &str {
        self.0
    }
    fn command(&self) -> PathBuf {
        PathBuf::from("true")
    }
    fn run(&self, _: &Workspace) -> Result<JsonlOutcome, DriverError> {
        Err(DriverError::Subprocess("synthetic jsonl failure".into()))
    }
}

#[test]
fn run_all_records_jsonl_failure() {
    let dir = tempfile::tempdir().unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let reports = IndexerDriver::new(ws)
        .with_jsonl_driver(JsonlFailIndexer("badjsonl"))
        .run_all();
    assert_eq!(reports.len(), 1);
    assert_eq!(reports[0].status, RunStatus::Failed);
    assert!(reports[0]
        .failed_projects
        .iter()
        .any(|s| s.contains("jsonl failure")));
}

#[test]
fn scip_go_discovers_one_unit_per_gomod() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
    std::fs::create_dir_all(dir.path().join("service")).unwrap();
    std::fs::write(
        dir.path().join("service/go.mod"),
        "module example.com/app/service\n",
    )
    .unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let mut units = ScipGo::default().discover_units(&ws).unwrap();
    units.sort_by(|a, b| a.path.cmp(&b.path));
    assert_eq!(units.len(), 2);
    // Each unit is rooted at the directory that holds its go.mod.
    assert_eq!(units[0].path, ws.root().to_path_buf());
    assert_eq!(units[1].path, ws.root().join("service"));
    // Identifiers are distinct so per-unit `.scip` outputs never collide.
    assert_ne!(units[0].identifier, units[1].identifier);
}

#[test]
fn scip_go_discovers_no_units_without_gomod() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("main.go"), "package main\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let units = ScipGo::default().discover_units(&ws).unwrap();
    assert!(units.is_empty(), "got units: {units:?}");
}

/// A `go.mod` under `vendor/` or `testdata/` MUST NOT become its own
/// unit. The patterns come from `kenn_config::GoConfig::DEFAULT_EXCLUDES`
/// attached via `with_language_excludes(Language::Go, ...)`.
#[test]
fn scip_go_skips_vendor_and_testdata_modules() {
    use kenn_model::Language;
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
    for excluded in ["vendor/dep", "testdata/fixture"] {
        std::fs::create_dir_all(dir.path().join(excluded)).unwrap();
        std::fs::write(
            dir.path().join(excluded).join("go.mod"),
            "module example.com/ignored\n",
        )
        .unwrap();
    }
    let go_defaults: Vec<String> = kenn_config::GoConfig::DEFAULT_EXCLUDES
        .iter()
        .map(|s| (*s).to_string())
        .collect();
    let ws = Workspace::new(dir.path(), &[])
        .unwrap()
        .with_language_excludes(Language::Go, &go_defaults)
        .unwrap();
    let units = ScipGo::default().discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1, "only the root module: {units:?}");
    assert_eq!(units[0].path, ws.root().to_path_buf());
}

#[test]
fn scip_go_returns_unavailable_when_binary_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("go.mod"), "module example.com/app\n").unwrap();
    let ws = Workspace::new(dir.path(), &[]).unwrap();
    let driver = ScipGo {
        command: vec!["/nonexistent/scip-go-binary-xyz".into()],
    };
    let units = driver.discover_units(&ws).unwrap();
    assert_eq!(units.len(), 1);
    let outcome = driver.run_unit(&units[0], &ws).unwrap();
    assert!(matches!(outcome, ScipOutcome::Unavailable { .. }));
}
