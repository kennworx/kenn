//! Language detection for `kenn init` (tasks 2.1 + 2.2).
//!
//! `kenn init` runs before a `kenn.toml` exists, so it cannot read excludes
//! from config. It walks the workspace once, pruning the union of every
//! language's `DEFAULT_EXCLUDES` (taken from the `kenn_config` constants
//! directly), and reports which languages' marker files are present.
//!
//! Only Rust and Go have marker-shaped discovery in the drivers; the rest of
//! this table is new. It also carries the source globs, test globs, version
//! probe, and install hint each later phase needs, so there is one place per
//! language rather than five.

use std::collections::{BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_config::{
    CsharpConfig, GoConfig, PythonConfig, RustConfig, SwiftConfig, TextConfig, TypescriptConfig,
};

/// Directory patterns dropped from the detection prune set even when a language
/// lists them: `build/` and `dist/` are generic enough to be another language's
/// real source root, so pruning them globally would hide that language's marker.
/// They still apply as per-language excludes at index time.
const AMBIGUOUS_PRUNE: &[&str] = &["build/**", "**/build/**", "dist/**", "**/dist/**"];

/// Cap on walk recursion depth — a backstop against a pathologically deep tree
/// or a symlink cycle that slips past the within-root guard.
const MAX_DEPTH: usize = 64;

// Default images for `kenn init --docker`, pinned to the kenn MINOR line
// (`:v0.2`), NOT a digest. A minor tag decouples operational image fixes (a base
// or dependency patch, a rebuilt sidecar) from a kenn release: republishing the
// `:v0.2` images via `.github/workflows/images.yml` reaches docker users on their
// next index with no re-pin here. The trade-off is that a repushed `:v0.2` can
// change indexing output within a kenn patch — kenn re-indexes on staleness, and
// a user who needs a hard pin can set `image = "…@sha256:…"` in kenn.toml. Bump
// these only on a MINOR release (0.2 → 0.3); patch releases leave them untouched.
const IMG_RUST: &str = "ghcr.io/kennworx/kenn-rust:v0.2";
const IMG_GO: &str = "ghcr.io/kennworx/kenn-go:v0.2";
const IMG_TYPESCRIPT: &str = "ghcr.io/kennworx/kenn-typescript:v0.2";
const IMG_CSHARP: &str = "ghcr.io/kennworx/kenn-csharp:v0.2";
const IMG_PYTHON: &str = "ghcr.io/kennworx/kenn-python:v0.2";
const IMG_SWIFT: &str = "ghcr.io/kennworx/kenn-swift:v0.2";

/// How a language's presence is recognized on disk.
enum Marker {
    /// A file with this exact name at the workspace root only. Rust's driver
    /// keys on the root `Cargo.toml` and rust-analyzer indexes the whole
    /// workspace from it, so a sub-crate manifest is not its own marker.
    RootFile(&'static str),
    /// A file with any of these base names at any (unpruned) depth.
    Basename(&'static [&'static str]),
    /// A file with any of these extensions at any (unpruned) depth. Extensions
    /// include the dot.
    Extension(&'static [&'static str]),
}

/// Everything the init phases need to know about one language, in one place.
pub struct LanguageSpec {
    pub name: &'static str,
    marker: Marker,
    /// Globs that select this language's source files, for the text fallback.
    pub source_globs: &'static [&'static str],
    /// Globs that mark test files, for seeding `[tests] paths`.
    pub test_globs: &'static [&'static str],
    /// Walk-time / text-fallback excludes — the language's `DEFAULT_EXCLUDES`.
    pub excludes: &'static [&'static str],
    /// The config default command for this language's indexer, as a function so
    /// there is one source of truth: the probe is `<command> --version`, and the
    /// index run uses the same tokens. `None` for an in-process producer
    /// (markdown, CSS, HTML) that needs no external tool and is always available.
    probe_command: Option<fn() -> Vec<String>>,
    /// One-line install hint shown when the probe fails. `None` for built-ins.
    pub install_hint: Option<&'static str>,
    /// Digest-pinned default OCI image for `kenn init --docker` (task 4.3): the
    /// published `ghcr.io/kennworx/*` image whose entrypoint is this language's
    /// indexer. `None` for built-ins (markdown/css/html) that need no external
    /// tool, and would stay `None` for any language without a published image.
    pub default_image: Option<&'static str>,
}

impl LanguageSpec {
    /// Whether `rel` (a workspace-relative path) is a marker for this language.
    /// `depth` is the number of path components (a root file has depth 1).
    fn matches(&self, rel: &Path, depth: usize) -> bool {
        let file = rel.file_name().and_then(|s| s.to_str());
        match &self.marker {
            Marker::RootFile(f) => depth == 1 && file == Some(*f),
            Marker::Basename(bases) => file.is_some_and(|n| bases.contains(&n)),
            Marker::Extension(exts) => rel
                .extension()
                .and_then(|s| s.to_str())
                .is_some_and(|e| exts.iter().any(|x| x.trim_start_matches('.') == e)),
        }
    }
}

/// The detection table. Rust and Go mirror their drivers' discovery; the rest
/// is new. Ordered most-common-first only for readable reports.
pub const SPECS: &[LanguageSpec] = &[
    LanguageSpec {
        name: "rust",
        marker: Marker::RootFile("Cargo.toml"),
        source_globs: &["**/*.rs"],
        test_globs: &["**/*_test.rs", "**/tests/**"],
        excludes: RustConfig::DEFAULT_EXCLUDES,
        probe_command: Some(|| RustConfig::default().command),
        install_hint: Some("rustup component add rust-analyzer"),
        default_image: Some(IMG_RUST),
    },
    LanguageSpec {
        name: "go",
        marker: Marker::Basename(&["go.mod"]),
        source_globs: &["**/*.go"],
        test_globs: &["**/*_test.go"],
        excludes: GoConfig::DEFAULT_EXCLUDES,
        probe_command: Some(|| GoConfig::default().command),
        install_hint: Some("go install github.com/scip-code/scip-go/cmd/scip-go@latest"),
        default_image: Some(IMG_GO),
    },
    LanguageSpec {
        name: "typescript",
        // Extension, not tsconfig.json: Deno/bun and some bundler repos have no
        // tsconfig but plenty of `.ts`/`.tsx`.
        marker: Marker::Extension(&[".ts", ".tsx", ".mts", ".cts"]),
        source_globs: &["**/*.ts", "**/*.tsx"],
        test_globs: &["**/*.test.ts", "**/*.spec.ts", "**/*.test.tsx"],
        excludes: TypescriptConfig::DEFAULT_EXCLUDES,
        probe_command: Some(|| TypescriptConfig::default().command),
        install_hint: Some(
            "brew install kennworx/tap/kenn-ts (from source: just build-indexer-ts)",
        ),
        default_image: Some(IMG_TYPESCRIPT),
    },
    LanguageSpec {
        name: "csharp",
        // `.slnx` is the newer XML solution format (MSBuild 17.13+). Real repos
        // ship only it (Newtonsoft.Json), and without it here they were not even
        // detected — `.sln` matches the literal extension, not `.slnx`.
        marker: Marker::Extension(&[".sln", ".slnx", ".csproj"]),
        source_globs: &["**/*.cs"],
        // Match test PROJECTS by directory suffix (`*.Test`/`*.Tests`) so a
        // project flags every file in it — fixtures, `*TestHost.cs`,
        // `*TestBase.cs` — not just `*Test.cs` names. The kenn-dotnet indexer is
        // the primary signal (it marks a project whose assembly references a
        // test framework, or matches `[tests] assembly_regex`), which also
        // catches a bare `Test/` project; these globs are the config-side net.
        test_globs: &[
            "**/*Test.cs",
            "**/*Tests.cs",
            "**/*.Test.cs",
            "**/*.Tests.cs",
            "**/*.Test/**",
            "**/*.Tests/**",
        ],
        excludes: CsharpConfig::DEFAULT_EXCLUDES,
        probe_command: Some(|| CsharpConfig::default().command),
        install_hint: Some(
            "brew install kennworx/tap/kenn-dotnet (from source: just build-indexer-dotnet)",
        ),
        default_image: Some(IMG_CSHARP),
    },
    LanguageSpec {
        name: "python",
        // Strong markers only. `requirements.txt` alone fires on repos with
        // Python docs/CI tooling but no Python source to index — `.python-version`
        // (pyenv) is included because it pins the interpreter for a source tree
        // and rarely appears without one, unlike `requirements.txt`.
        marker: Marker::Basename(&["pyproject.toml", "setup.py", ".python-version"]),
        source_globs: &["**/*.py"],
        test_globs: &["**/test_*.py", "**/*_test.py"],
        excludes: PythonConfig::DEFAULT_EXCLUDES,
        probe_command: Some(|| PythonConfig::default().command),
        install_hint: Some("npm install -g @sourcegraph/scip-python"),
        default_image: Some(IMG_PYTHON),
    },
    LanguageSpec {
        name: "swift",
        marker: Marker::Basename(&["Package.swift"]),
        source_globs: &["**/*.swift"],
        test_globs: &["**/*Tests.swift", "**/*Test.swift"],
        excludes: SwiftConfig::DEFAULT_EXCLUDES,
        probe_command: Some(|| SwiftConfig::default().command),
        install_hint: Some(
            "brew install kennworx/tap/kenn-swift (from source: just build-indexer-swift)",
        ),
        default_image: Some(IMG_SWIFT),
    },
    LanguageSpec {
        name: "markdown",
        marker: Marker::Extension(&[".md"]),
        source_globs: &["**/*.md"],
        test_globs: &[],
        // No defaults: the markdown walk inherits the workspace + per-language
        // excludes at wire-up (see `markdown_with_inherited_excludes`); the
        // starter lists only repo-specific dirs, which the user adds.
        excludes: &[],
        probe_command: None,
        install_hint: None,
        default_image: None,
    },
    LanguageSpec {
        name: "css",
        marker: Marker::Extension(&[".css", ".scss", ".sass"]),
        source_globs: &["**/*.css", "**/*.scss", "**/*.sass"],
        test_globs: &[],
        // CSS has no per-language DEFAULT_EXCLUDES const; reuse text's.
        excludes: TextConfig::DEFAULT_EXCLUDES,
        probe_command: None,
        install_hint: None,
        default_image: None,
    },
    LanguageSpec {
        name: "html",
        marker: Marker::Extension(&[".html", ".htm"]),
        source_globs: &["**/*.html", "**/*.htm"],
        test_globs: &[],
        // HTML/CSS have no per-language DEFAULT_EXCLUDES const (they mix build
        // denies in at runtime); text's defaults are the right walk-prune set.
        excludes: TextConfig::DEFAULT_EXCLUDES,
        probe_command: None,
        install_hint: None,
        default_image: None,
    },
];

/// Whether a language's indexer is usable here (2.4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Availability {
    /// Marker found and the indexer's version probe succeeded (or it is a
    /// built-in that needs none).
    Enabled,
    /// `kenn init --docker` chose a container fallback: the local probe failed,
    /// but `docker` is runnable and this language has a published default image.
    /// Authored as `enabled = true` + `runtime = "docker"` + `image` (task 5.1).
    Containerized { image: String },
    /// Marker found but the probe failed. The language falls back to text.
    ///
    /// `hint` is the static per-language install advice; `reason` is what the
    /// failing process (or the loader that refused to start it) actually said,
    /// empty when it could not be executed or said nothing. Both are kept:
    /// `reason` is specific but only exists sometimes, and every third-party
    /// indexer has nothing but `hint`.
    Degraded {
        command: String,
        hint: String,
        reason: String,
        /// The command could not be executed at all, rather than running and
        /// failing. Different fix, so it reads differently.
        not_executable: bool,
    },
}

/// A detected language paired with its availability verdict.
pub struct Classified {
    pub spec: &'static LanguageSpec,
    pub availability: Availability,
}

/// Run a version-probe argv and report success. Exit 0 ⇒ true; a non-zero exit
/// (the Homebrew-rustup-shim case: a present binary that fails to run) or a
/// spawn failure ⇒ false. Existence on `PATH` alone is deliberately not enough.
/// What a probe learned. A bool cannot express the distinction that matters:
/// an indexer that could not be executed needs installing, while one that ran
/// and failed is present but missing something it needs — and only the second
/// has an explanation worth showing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Probe {
    /// Ran and exited 0.
    Ok,
    /// Could not be executed at all — not found, or not executable.
    NotExecutable,
    /// Ran and exited non-zero. `stderr` is what it said, which may be empty.
    Failed { stderr: String },
}

impl Probe {
    fn ok(&self) -> bool {
        matches!(self, Probe::Ok)
    }
}

/// Run `<command> --version` and report what happened.
///
/// stderr is CAPTURED rather than discarded: it is the only place the specific
/// reason lives. For `kenn-swift` that reason is often not even the indexer's
/// own words — `libIndexStore` is a hard link dependency, so a failure to
/// resolve it aborts the process in dyld before `main`, and the loader's
/// "Library not loaded" message is the whole diagnostic.
///
/// `output()` rather than a background drain: `--version` writes a few bytes,
/// so there is no pipe to fill. The index-time path streams and does need one.
fn probe_ok(argv: &[String]) -> Probe {
    let Some((prog, rest)) = argv.split_first() else {
        return Probe::NotExecutable;
    };
    let out = std::process::Command::new(prog)
        .args(rest)
        .stdin(std::process::Stdio::null())
        .output();
    match out {
        Ok(o) if o.status.success() => Probe::Ok,
        Ok(o) => Probe::Failed {
            stderr: String::from_utf8_lossy(&o.stderr).trim().to_string(),
        },
        Err(_) => Probe::NotExecutable,
    }
}

/// Classify one detected spec, using `probe` to test its tool. A built-in is
/// always `Enabled`; an external tool is `Enabled` iff `<command> --version`
/// exits 0. The command comes from the same config default the index run uses,
/// so the probe can't drift from what actually runs. When `containerize` is set
/// (`kenn init --docker` with a runnable daemon), a failing probe on a language
/// with a published `default_image` yields `Containerized` instead of
/// `Degraded` — index it in a container rather than dropping to text.
fn classify_with(
    spec: &'static LanguageSpec,
    probe: impl Fn(&[String]) -> Probe,
    containerize: bool,
) -> Classified {
    let availability = match spec.probe_command {
        None => Availability::Enabled,
        Some(command_fn) => {
            let mut argv = command_fn();
            let tool = argv.first().cloned().unwrap_or_default();
            argv.push("--version".to_string());
            let outcome = probe(&argv);
            if outcome.ok() {
                Availability::Enabled
            } else if let Some(image) = spec.default_image.filter(|_| containerize) {
                Availability::Containerized {
                    image: image.to_string(),
                }
            } else {
                let (reason, not_executable) = match outcome {
                    Probe::Failed { stderr } => (stderr, false),
                    _ => (String::new(), true),
                };
                Availability::Degraded {
                    command: tool,
                    hint: spec.install_hint.unwrap_or("").to_string(),
                    reason,
                    not_executable,
                }
            }
        }
    };
    Classified { spec, availability }
}

/// Detect and classify every language in `root`, probing real tools (2.4).
/// `containerize` routes a failing probe to a container image where one exists
/// (`kenn init --docker`); `false` keeps the original degrade-to-text behavior.
#[must_use]
pub fn detect_and_classify(root: &Path, containerize: bool) -> Vec<Classified> {
    detect(root)
        .into_iter()
        .map(|spec| classify_with(spec, probe_ok, containerize))
        .collect()
}

/// The union of every language's excludes (minus the ambiguous generic dirs)
/// plus the always-pruned VCS/store dirs. The walk descends nothing matching
/// these, so a `go.mod` under `vendor/` or a `Cargo.toml` under `target/` never
/// counts as a marker.
///
/// For each `foo/**` pattern the bare `foo` is added too, so the walk can match
/// a directory's own relative path without allocating a `foo/x` probe per dir.
fn prune_globs() -> GlobSet {
    let mut b = GlobSetBuilder::new();
    let mut add = |pat: &str| {
        if let Ok(g) = Glob::new(pat) {
            b.add(g);
        }
        if let Some(bare) = pat.strip_suffix("/**") {
            if let Ok(g) = Glob::new(bare) {
                b.add(g);
            }
        }
    };
    for pat in [".git/**", "**/.git/**", ".kenn/**", "**/.kenn/**"] {
        add(pat);
    }
    for spec in SPECS {
        for pat in spec.excludes {
            if !AMBIGUOUS_PRUNE.contains(pat) {
                add(pat);
            }
        }
    }
    b.build().unwrap_or_else(|_| globset::GlobSet::empty())
}

/// Walk `root` once, pruning the excluded dirs, and return the specs whose
/// marker is present. Order follows [`SPECS`]. Stops early once every language
/// is found — no marker can change the answer after that.
#[must_use]
pub fn detect(root: &Path) -> Vec<&'static LanguageSpec> {
    let pruned = prune_globs();
    // Symlinked dirs are followed only when they resolve within the repo, and
    // each canonical target is entered at most once — guards cycles and stops
    // detection escaping into an external tree via a symlink.
    let root_canon = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let mut walker = Walker {
        root,
        root_canon,
        pruned: &pruned,
        found: BTreeSet::new(),
        seen_links: HashSet::new(),
    };
    walker.walk(root, 0);
    let found = walker.found;
    SPECS.iter().filter(|s| found.contains(s.name)).collect()
}

/// Mutable state threaded through the recursive walk.
struct Walker<'a> {
    root: &'a Path,
    root_canon: PathBuf,
    pruned: &'a GlobSet,
    found: BTreeSet<&'static str>,
    seen_links: HashSet<PathBuf>,
}

impl Walker<'_> {
    /// Depth-first walk pruning matched directories BEFORE descending, so a
    /// populated `vendor/` or `target/` costs one `is_match`, not a descent.
    /// Mirrors the indexer's own `read_dir` recursion — no walk dependency.
    fn walk(&mut self, dir: &Path, depth: usize) {
        if depth > MAX_DEPTH || self.found.len() == SPECS.len() {
            return;
        }
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            if self.found.len() == SPECS.len() {
                return;
            }
            let path = entry.path();
            let Ok(rel) = path.strip_prefix(self.root) else {
                continue;
            };
            // Bare directory names are in the prune set, so `rel` matches the
            // directory itself without allocating a `rel/x` probe.
            if self.pruned.is_match(rel) {
                continue;
            }
            if self.is_followable_dir(&entry, &path) {
                self.walk(&path, depth + 1);
            } else if entry.file_type().is_ok_and(|t| t.is_file()) {
                self.record(rel);
            }
        }
    }

    /// True when `path` is a directory we should descend into. A real directory
    /// always; a symlinked one only if it canonicalizes within the repo and has
    /// not been entered before (cycle + external-escape guard).
    fn is_followable_dir(&mut self, entry: &std::fs::DirEntry, path: &Path) -> bool {
        let Ok(ft) = entry.file_type() else {
            return false;
        };
        if ft.is_dir() {
            return true;
        }
        if !ft.is_symlink() {
            return false;
        }
        let Ok(target) = path.canonicalize() else {
            return false;
        };
        // Follow only a directory that stays inside the repo and that we have
        // not entered before — guards symlink cycles and external escapes.
        target.is_dir() && target.starts_with(&self.root_canon) && self.seen_links.insert(target)
    }

    /// Note any language whose marker `rel` satisfies.
    fn record(&mut self, rel: &Path) {
        let depth = rel.components().count();
        for spec in SPECS {
            if !self.found.contains(spec.name) && spec.matches(rel, depth) {
                self.found.insert(spec.name);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch(dir: &Path, rel: &str) {
        let p = dir.join(rel);
        if let Some(parent) = p.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(p, "").unwrap();
    }

    fn names(root: &Path) -> Vec<&'static str> {
        detect(root).iter().map(|s| s.name).collect()
    }

    #[test]
    fn bare_directory_detects_nothing() {
        let d = TempDir::new().unwrap();
        assert!(names(d.path()).is_empty());
    }

    #[test]
    fn cargo_and_typescript_detect_both() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "Cargo.toml");
        touch(d.path(), "src/index.ts");
        let got = names(d.path());
        assert!(got.contains(&"rust"), "{got:?}");
        assert!(got.contains(&"typescript"), "{got:?}");
    }

    #[test]
    fn go_mod_in_a_subpackage_is_detected() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "services/api/go.mod");
        assert_eq!(names(d.path()), vec!["go"]);
    }

    #[test]
    fn go_mod_under_vendor_alone_is_not_detected() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "vendor/example.com/dep/go.mod");
        assert!(names(d.path()).is_empty());
    }

    #[test]
    fn cargo_toml_under_target_alone_is_not_detected() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "target/debug/build/x/Cargo.toml");
        assert!(names(d.path()).is_empty());
    }

    #[test]
    fn sub_crate_cargo_toml_is_not_a_marker_without_a_root_one() {
        // Rust keys on the ROOT manifest; a sub-crate manifest alone must not
        // trigger detection.
        let d = TempDir::new().unwrap();
        touch(d.path(), "crates/foo/Cargo.toml");
        assert!(!names(d.path()).contains(&"rust"));
    }

    #[test]
    fn csproj_anywhere_detects_csharp() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "src/App/App.csproj");
        assert_eq!(names(d.path()), vec!["csharp"]);
    }

    /// The newer XML solution format. A repo shipping only `.slnx`
    /// (Newtonsoft.Json) was not detected at all before it was a marker.
    #[test]
    fn slnx_alone_detects_csharp() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "Src/App.slnx");
        assert_eq!(names(d.path()), vec!["csharp"]);
    }

    /// A pyenv-pinned tree with no pyproject/setup.py. The interpreter pin is a
    /// strong-enough signal, unlike `requirements.txt`.
    #[test]
    fn python_version_file_detects_python() {
        let d = TempDir::new().unwrap();
        touch(d.path(), ".python-version");
        touch(d.path(), "app.py");
        assert!(names(d.path()).contains(&"python"), "{:?}", names(d.path()));
    }

    #[test]
    fn markdown_is_detected_and_is_builtin() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "docs/readme.md");
        let specs = detect(d.path());
        assert_eq!(specs.len(), 1);
        assert_eq!(specs[0].name, "markdown");
        assert!(
            specs[0].probe_command.is_none(),
            "markdown needs no external tool"
        );
    }

    #[test]
    fn typescript_is_detected_without_a_tsconfig() {
        // Deno/bun repos have `.ts` but no tsconfig.json.
        let d = TempDir::new().unwrap();
        touch(d.path(), "src/main.ts");
        assert!(
            names(d.path()).contains(&"typescript"),
            "{:?}",
            names(d.path())
        );
    }

    #[test]
    fn requirements_txt_alone_does_not_detect_python() {
        // A repo with only Python docs/CI tooling must not trip Python detection.
        let d = TempDir::new().unwrap();
        touch(d.path(), "requirements.txt");
        assert!(
            !names(d.path()).contains(&"python"),
            "{:?}",
            names(d.path())
        );
    }

    #[test]
    fn pyproject_toml_does_detect_python() {
        let d = TempDir::new().unwrap();
        touch(d.path(), "pyproject.toml");
        assert!(names(d.path()).contains(&"python"));
    }

    #[test]
    fn go_mod_under_a_build_dir_is_still_detected() {
        // `build/` is in Python's excludes but is a generic name; pruning it
        // globally would hide another language's marker (finding: exclude bleed).
        let d = TempDir::new().unwrap();
        touch(d.path(), "build/svc/go.mod");
        assert!(names(d.path()).contains(&"go"), "{:?}", names(d.path()));
    }

    fn spec(name: &str) -> &'static LanguageSpec {
        SPECS.iter().find(|s| s.name == name).unwrap()
    }

    fn argv(parts: &[&str]) -> Vec<String> {
        parts.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn probe_ok_reflects_exit_status() {
        // Real system binaries stand in for the three outcomes, so the test
        // needs no language toolchain installed.
        assert_eq!(probe_ok(&argv(&["true"])), Probe::Ok, "exit 0 ⇒ available");
        assert!(
            matches!(probe_ok(&argv(&["false"])), Probe::Failed { .. }),
            "non-zero exit ⇒ unavailable (the shim case)"
        );
        assert_eq!(
            probe_ok(&argv(&["kenn-nonexistent-binary-zzz", "--version"])),
            Probe::NotExecutable,
            "spawn failure ⇒ unavailable"
        );
    }

    /// A binary that fails must hand back what it said. This is the whole point
    /// of the probe capturing stderr: for `kenn-swift` the specific reason is
    /// often the dynamic loader's "Library not loaded", printed before `main`
    /// ever runs, and it is the only place the missing library is named.
    #[test]
    fn a_failing_probe_captures_what_the_command_said() {
        let argv = argv(&["sh", "-c", "echo 'boom: libIndexStore' >&2; exit 1"]);
        match probe_ok(&argv) {
            Probe::Failed { stderr } => {
                assert!(stderr.contains("libIndexStore"), "captured: {stderr:?}");
            }
            other => panic!("want Failed with stderr, got {other:?}"),
        }
    }

    /// "Not found" and "ran and failed" are different fixes — install it, versus
    /// it is installed but something it needs is not — so they must not collapse
    /// into one verdict.
    #[test]
    fn an_absent_command_is_distinguished_from_a_failing_one() {
        assert_eq!(
            probe_ok(&argv(&["kenn-nonexistent-binary-zzz"])),
            Probe::NotExecutable
        );
        assert!(matches!(probe_ok(&argv(&["false"])), Probe::Failed { .. }));
    }

    #[test]
    fn a_builtin_is_always_enabled() {
        let c = classify_with(
            spec("markdown"),
            |_| panic!("built-ins must not probe"),
            false,
        );
        assert_eq!(c.availability, Availability::Enabled);
    }

    #[test]
    fn an_available_tool_enables_its_language() {
        let c = classify_with(spec("rust"), |_| Probe::Ok, false);
        assert_eq!(c.availability, Availability::Enabled);
    }

    #[test]
    fn a_failing_probe_degrades_with_command_and_hint() {
        let c = classify_with(
            spec("go"),
            |_| Probe::Failed {
                stderr: String::new(),
            },
            false,
        );
        match c.availability {
            Availability::Degraded { command, hint, .. } => {
                assert_eq!(command, "scip-go");
                assert!(hint.contains("scip-go"), "hint names the tool: {hint}");
            }
            other => panic!("a failing probe must degrade: {other:?}"),
        }
    }

    /// The three sidecars kenn ships are installable from its Homebrew tap; a
    /// failing probe must name the formula so a `brew` user knows exactly what
    /// to install. Third-party indexers keep their upstream install hint —
    /// they are not ours to package.
    #[test]
    fn kenn_authored_hints_name_the_homebrew_formula() {
        let fail = |_: &[String]| Probe::Failed {
            stderr: String::new(),
        };
        for (lang, formula) in [
            ("csharp", "kenn-dotnet"),
            ("typescript", "kenn-ts"),
            ("swift", "kenn-swift"),
        ] {
            match classify_with(spec(lang), fail, false).availability {
                Availability::Degraded { hint, .. } => assert!(
                    hint.contains(&format!("brew install kennworx/tap/{formula}")),
                    "{lang} hint must name its formula: {hint}"
                ),
                other => panic!("{lang} failing probe must degrade: {other:?}"),
            }
        }
        // Third-party indexers must NOT be advertised as a kenn tap formula.
        for lang in ["rust", "go", "python"] {
            if let Availability::Degraded { hint, .. } =
                classify_with(spec(lang), fail, false).availability
            {
                assert!(
                    !hint.contains("kennworx/tap"),
                    "{lang} is third-party; hint must not name a kenn formula: {hint}"
                );
            }
        }
    }

    /// The wiring between capture and render, which neither of those tests
    /// covers on its own: the probe's stderr has to REACH `Degraded.reason`.
    /// A capture test passes while classify drops it on the floor, and a render
    /// test that builds `Degraded` by hand passes while nothing ever fills it.
    #[test]
    fn a_failing_probes_stderr_reaches_the_verdict() {
        let c = classify_with(
            spec("swift"),
            |_| Probe::Failed {
                stderr: "dyld: Library not loaded: @rpath/libIndexStore.dylib".to_string(),
            },
            false,
        );
        match c.availability {
            Availability::Degraded {
                reason,
                not_executable,
                ..
            } => {
                assert!(reason.contains("libIndexStore"), "reason: {reason:?}");
                assert!(!not_executable, "it ran and failed, it was not absent");
            }
            other => panic!("want Degraded carrying the reason: {other:?}"),
        }
    }

    /// An absent command carries no reason and must say so, or the report shows
    /// an empty explanation instead of the install hint.
    #[test]
    fn an_unexecutable_probe_carries_no_reason() {
        let c = classify_with(spec("swift"), |_| Probe::NotExecutable, false);
        match c.availability {
            Availability::Degraded {
                reason,
                not_executable,
                ..
            } => {
                assert!(reason.is_empty(), "reason: {reason:?}");
                assert!(not_executable);
            }
            other => panic!("want Degraded: {other:?}"),
        }
    }

    #[test]
    fn a_failing_probe_with_docker_containerizes_to_the_default_image() {
        // `--docker` active + probe fails + a published image ⇒ Containerized
        // with that language's minor-tag-pinned default, not Degraded.
        let c = classify_with(
            spec("rust"),
            |_| Probe::Failed {
                stderr: String::new(),
            },
            true,
        );
        match c.availability {
            Availability::Containerized { image } => {
                assert_eq!(image, IMG_RUST);
                assert!(
                    image.starts_with("ghcr.io/kennworx/kenn-rust:v")
                        && !image.contains("@sha256:"),
                    "minor-tag-pinned kennworx ref: {image}"
                );
            }
            other => panic!("--docker on a failing probe must containerize: {other:?}"),
        }
    }

    #[test]
    fn docker_does_not_containerize_an_available_tool() {
        // Containerization is a *fallback*: an available local tool stays
        // Enabled even under `--docker`, so we don't force a container when the
        // host toolchain works.
        let c = classify_with(spec("rust"), |_| Probe::Ok, true);
        assert_eq!(c.availability, Availability::Enabled);
    }

    #[test]
    fn probe_argv_comes_from_the_config_default_command() {
        // The probe must be the config's default command + --version, not a
        // second hardcoded copy of the tool name. Capture what classify passes.
        let seen = std::cell::RefCell::new(Vec::new());
        classify_with(
            spec("rust"),
            |argv| {
                *seen.borrow_mut() = argv.to_vec();
                Probe::Ok
            },
            false,
        );
        let got = seen.into_inner();
        assert_eq!(
            got,
            kenn_config::RustConfig::default()
                .command
                .into_iter()
                .chain(std::iter::once("--version".to_string()))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    #[cfg(unix)]
    fn a_symlinked_source_dir_within_root_is_followed() {
        use std::os::unix::fs::symlink;
        let d = TempDir::new().unwrap();
        touch(d.path(), "real/pkg/go.mod");
        symlink(d.path().join("real"), d.path().join("linked")).unwrap();
        // go.mod is reachable via real/; detection should find go regardless,
        // and following the symlink must not loop or escape.
        assert!(names(d.path()).contains(&"go"));
    }
}
