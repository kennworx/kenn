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

// Digest-pinned default images for `kenn init --docker` (task 4.3). These are
// the manifest-list digests of the `ghcr.io/kennworx/*` images published by
// `.github/workflows/images.yml`; pinning by digest (not a `:latest` tag) means
// republishing an image can't silently change what an authored config resolves
// to. To refresh after a republish: `docker buildx imagetools inspect
// ghcr.io/kennworx/<name>:latest` and copy the top-level manifest digest.
const IMG_RUST: &str =
    "ghcr.io/kennworx/kenn-rust@sha256:7a96aedc593931746735b180c60195226be5f9dc795ed9049c6a8c5123baf82f";
const IMG_GO: &str =
    "ghcr.io/kennworx/kenn-go@sha256:24ade530dc0e1ab9f12ed194c4d7978dcb3a089cfee55ceda2bddeeb3df0a29a";
const IMG_TYPESCRIPT: &str =
    "ghcr.io/kennworx/kenn-typescript@sha256:fcd0f456f9bb3c61704e3a4867aaf6857d8e5fc7391a5ccf930c8d54d2839beb";
const IMG_CSHARP: &str =
    "ghcr.io/kennworx/kenn-csharp@sha256:5cf430ea2cafeb0d2a69de93b7cf461e0c8b1163c181602a0e2b7337e094dcd0";
const IMG_PYTHON: &str =
    "ghcr.io/kennworx/kenn-python@sha256:4f300609239eed9650868a23a30a2235796b721f18b52f1e207ecad0a2858a5a";
const IMG_SWIFT: &str =
    "ghcr.io/kennworx/kenn-swift@sha256:53b83cfc57b6f208de4bcc1ecd34d01dc7d0d11db0e6ca98575a2ba254604963";

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
        install_hint: Some("install kenn-ts on PATH (just build-indexer-ts)"),
        default_image: Some(IMG_TYPESCRIPT),
    },
    LanguageSpec {
        name: "csharp",
        marker: Marker::Extension(&[".sln", ".csproj"]),
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
        install_hint: Some("install kenn-dotnet on PATH (just build-indexer-dotnet)"),
        default_image: Some(IMG_CSHARP),
    },
    LanguageSpec {
        name: "python",
        // Strong markers only. `requirements.txt` alone fires on repos with
        // Python docs/CI tooling but no Python source to index.
        marker: Marker::Basename(&["pyproject.toml", "setup.py"]),
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
        install_hint: Some("install kenn-swift on PATH (just build-indexer-swift)"),
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
    Degraded { command: String, hint: String },
}

/// A detected language paired with its availability verdict.
pub struct Classified {
    pub spec: &'static LanguageSpec,
    pub availability: Availability,
}

/// Run a version-probe argv and report success. Exit 0 ⇒ true; a non-zero exit
/// (the Homebrew-rustup-shim case: a present binary that fails to run) or a
/// spawn failure ⇒ false. Existence on `PATH` alone is deliberately not enough.
fn probe_ok(argv: &[String]) -> bool {
    let Some((prog, rest)) = argv.split_first() else {
        return false;
    };
    std::process::Command::new(prog)
        .args(rest)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .stdin(std::process::Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
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
    probe: impl Fn(&[String]) -> bool,
    containerize: bool,
) -> Classified {
    let availability = match spec.probe_command {
        None => Availability::Enabled,
        Some(command_fn) => {
            let mut argv = command_fn();
            let tool = argv.first().cloned().unwrap_or_default();
            argv.push("--version".to_string());
            if probe(&argv) {
                Availability::Enabled
            } else if let Some(image) = spec.default_image.filter(|_| containerize) {
                Availability::Containerized {
                    image: image.to_string(),
                }
            } else {
                Availability::Degraded {
                    command: tool,
                    hint: spec.install_hint.unwrap_or("").to_string(),
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
        assert!(probe_ok(&argv(&["true"])), "exit 0 ⇒ available");
        assert!(
            !probe_ok(&argv(&["false"])),
            "non-zero exit ⇒ unavailable (the shim case)"
        );
        assert!(
            !probe_ok(&argv(&["kenn-nonexistent-binary-zzz", "--version"])),
            "spawn failure ⇒ unavailable"
        );
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
        let c = classify_with(spec("rust"), |_| true, false);
        assert_eq!(c.availability, Availability::Enabled);
    }

    #[test]
    fn a_failing_probe_degrades_with_command_and_hint() {
        let c = classify_with(spec("go"), |_| false, false);
        match c.availability {
            Availability::Degraded { command, hint } => {
                assert_eq!(command, "scip-go");
                assert!(hint.contains("scip-go"), "hint names the tool: {hint}");
            }
            other => panic!("a failing probe must degrade: {other:?}"),
        }
    }

    #[test]
    fn a_failing_probe_with_docker_containerizes_to_the_default_image() {
        // `--docker` active + probe fails + a published image ⇒ Containerized
        // with that language's digest-pinned default, not Degraded.
        let c = classify_with(spec("rust"), |_| false, true);
        match c.availability {
            Availability::Containerized { image } => {
                assert_eq!(image, IMG_RUST);
                assert!(
                    image.starts_with("ghcr.io/kennworx/kenn-rust@sha256:"),
                    "digest-pinned kennworx ref: {image}"
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
        let c = classify_with(spec("rust"), |_| true, true);
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
                true
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
