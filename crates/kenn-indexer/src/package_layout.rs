//! Discover package boundaries in a workspace by walking for the marker
//! files each language uses (`Cargo.toml`, `package.json`, `go.mod`,
//! `pyproject.toml`, `*.csproj`, `*.sln`).
//!
//! Used by the end-of-run aggregation pass to assign a meaningful
//! **anchor** to each aggregate node. Without this, the path-prefix
//! fallback flattens monorepos: a Rust workspace where every crate
//! lives under `crates/` ends up with one anchor named `"crates"`; a
//! TypeScript app under `server/` ends up with one anchor named
//! `"server"`. Walking once at setup and matching against the
//! deepest-containing marker fixes that — `crates/kenn-indexer/src/foo.rs`
//! resolves to anchor `"kenn-indexer"` (from `crates/kenn-indexer/Cargo.toml`).
//!
//! The walk is bounded: it skips the workspace's excluded directories
//! (which already filter out `node_modules/`, `target/`, etc.) and
//! never descends into nested `.git` or `.kenn` dirs. Marker discovery
//! uses depth-first walking so a deeper marker's directory always wins
//! over a shallower one's. Results are sorted **deepest-first** by path
//! component count so longest-prefix lookup is a single linear scan.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// One discovered package boundary. Anchors a region of the workspace
/// to a human-readable name (the package's declared name when we can
/// parse it, falling back to the containing directory name).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PackageMarker {
    /// Workspace-relative directory containing the marker file, with
    /// `/` as separator. Empty string when the marker is at workspace
    /// root.
    pub rel_dir: String,
    /// Anchor name to associate with files under `rel_dir`.
    pub anchor_name: String,
}

/// Sorted list of package markers. Lookups consult it linearly,
/// matching the deepest prefix that's an ancestor of the queried path.
#[derive(Debug, Clone, Default)]
pub struct PackageLayout {
    markers: Vec<PackageMarker>,
}

impl PackageLayout {
    #[must_use]
    pub fn empty() -> Self {
        Self::default()
    }

    /// Walk `root` for known package-marker filenames, parse each one's
    /// declared name if possible, and return a [`PackageLayout`] sorted
    /// for deepest-prefix lookups. `excluded_dirs` are absolute paths
    /// (typically the workspace's exclude set) that are pruned from the
    /// walk.
    #[must_use]
    pub fn discover(root: &Path, excluded_dirs: &[PathBuf]) -> Self {
        let mut markers: Vec<PackageMarker> = Vec::new();
        let mut go = GoDirs::default();
        walk_for_markers(root, root, excluded_dirs, &mut markers, &mut go, 0);
        markers.extend(go.package_markers());

        // Deduplicate: a directory may have BOTH a Cargo.toml and a
        // pyproject.toml; keep the first-seen (walker order is
        // deterministic enough for our determinism contract).
        markers.sort_by(|a, b| {
            a.rel_dir
                .cmp(&b.rel_dir)
                .then(a.anchor_name.cmp(&b.anchor_name))
        });
        markers.dedup_by(|a, b| a.rel_dir == b.rel_dir);

        // Re-sort deepest first for linear lookup.
        markers.sort_by(|a, b| {
            let da = depth_of(&a.rel_dir);
            let db = depth_of(&b.rel_dir);
            db.cmp(&da).then(a.rel_dir.cmp(&b.rel_dir))
        });

        Self { markers }
    }

    #[must_use]
    pub fn markers(&self) -> &[PackageMarker] {
        &self.markers
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.markers.is_empty()
    }

    /// Find the deepest marker whose `rel_dir` is an ancestor of
    /// `workspace_rel_path` (or equals it). Returns the anchor name on
    /// hit, `None` when no marker covers the path.
    #[must_use]
    pub fn anchor_for(&self, workspace_rel_path: &str) -> Option<&str> {
        for m in &self.markers {
            if path_is_under(workspace_rel_path, &m.rel_dir) {
                return Some(&m.anchor_name);
            }
        }
        None
    }
}

/// Go source directories seen during the walk, and the modules they belong to.
///
/// Go is the one language whose unit of import is the DIRECTORY, not the
/// manifest: a single `go.mod` covers every package in the module. Anchoring by
/// manifest alone therefore collapses them all into the module — `spf13/afero`,
/// with 8 importable packages, resolves to ONE anchor, and every edge between
/// them (`afero/zipfs.File.Truncate` → `afero.File.Truncate`) becomes
/// intra-anchor and vanishes from the package graph. Rust and C# escape this
/// only because Cargo and `MSBuild` happen to put one manifest per unit.
#[derive(Debug, Default)]
struct GoDirs {
    /// `(rel_dir, module anchor name)` for each `go.mod` found.
    modules: Vec<(String, String)>,
    /// Workspace-relative dirs holding at least one `.go` file.
    sources: Vec<String>,
}

impl GoDirs {
    /// A marker per Go source directory, named `<module>/<path-from-module>` —
    /// the import path a Go developer actually writes, minus the module's
    /// domain prefix (`parse_go_mod_name` already reduces `github.com/spf13/afero`
    /// to `afero`). The module's own directory is skipped: `walk_for_markers`
    /// already emitted its `go.mod` marker, and a duplicate would only be
    /// deduped away.
    fn package_markers(&self) -> Vec<PackageMarker> {
        let mut out = Vec::new();
        for dir in &self.sources {
            // Deepest enclosing module — nested modules are legal in Go.
            let Some((mod_dir, anchor)) = self
                .modules
                .iter()
                .filter(|(m, _)| path_is_under(dir, m))
                .max_by_key(|(m, _)| depth_of(m))
            else {
                continue;
            };
            if dir == mod_dir {
                continue;
            }
            let rel = dir.strip_prefix(mod_dir.as_str()).unwrap_or(dir);
            let rel = rel.trim_start_matches('/');
            if rel.is_empty() {
                continue;
            }
            out.push(PackageMarker {
                rel_dir: dir.clone(),
                anchor_name: format!("{anchor}/{rel}"),
            });
        }
        out
    }
}

/// Pruned recursive walk. Avoids the well-known noisy directories
/// (`.git`, `.kenn`, `node_modules`, `target`, `bin`, `obj`) and any
/// path under `excluded_dirs`. Depth-capped at 12 to avoid runaway
/// recursion in pathological symlink loops — real package marker files
/// live near the top of any repo we care about.
fn walk_for_markers(
    root: &Path,
    cur: &Path,
    excluded_dirs: &[PathBuf],
    out: &mut Vec<PackageMarker>,
    go: &mut GoDirs,
    depth: usize,
) {
    const MAX_DEPTH: usize = 12;
    if depth > MAX_DEPTH {
        return;
    }
    if excluded_dirs.iter().any(|d| cur.starts_with(d)) {
        return;
    }
    let Ok(entries) = std::fs::read_dir(cur) else {
        return;
    };
    let mut has_go = false;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(file_name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_dir() {
            if matches!(
                file_name,
                ".git" | ".kenn" | "node_modules" | "target" | "bin" | "obj"
            ) {
                continue;
            }
            walk_for_markers(root, &path, excluded_dirs, out, go, depth + 1);
            continue;
        }
        if !file_type.is_file() {
            continue;
        }
        let Ok(rel) = cur.strip_prefix(root) else {
            continue;
        };
        let rel_str = rel
            .to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/");
        // This dir is a Go package if it holds any `.go` file, whatever manifest
        // covers it. Flagged rather than pushed inline: the entry loop recurses
        // into subdirectories between files, so an inline push would record the
        // same directory once per `.go` file it contains.
        if std::path::Path::new(file_name)
            .extension()
            .is_some_and(|e| e.eq_ignore_ascii_case("go"))
        {
            has_go = true;
        }
        let Some(anchor_name) = anchor_name_from_marker(&path, file_name) else {
            continue;
        };
        if anchor_name.is_empty() {
            continue;
        }
        if file_name == "go.mod" {
            go.modules.push((rel_str.clone(), anchor_name.clone()));
        }
        out.push(PackageMarker {
            rel_dir: rel_str,
            anchor_name,
        });
    }
    if has_go {
        if let Ok(rel) = cur.strip_prefix(root) {
            go.sources.push(
                rel.to_string_lossy()
                    .replace(std::path::MAIN_SEPARATOR, "/"),
            );
        }
    }
}

fn depth_of(rel_dir: &str) -> usize {
    if rel_dir.is_empty() {
        0
    } else {
        rel_dir.split('/').filter(|s| !s.is_empty()).count()
    }
}

/// True when `path` (workspace-relative, `/`-separated) is inside the
/// directory `dir` (also workspace-relative, `/`-separated). The root
/// `dir == ""` matches every path.
fn path_is_under(path: &str, dir: &str) -> bool {
    if dir.is_empty() {
        return true;
    }
    let Some(after) = path.strip_prefix(dir) else {
        return false;
    };
    // Ensure dir matches at a path boundary, not mid-segment.
    after.is_empty() || after.starts_with('/')
}

/// Decide whether `file_name` is a package marker and, if so, return a
/// best-effort anchor name. The name comes from parsing the marker
/// (when cheap and well-defined) or from the marker file's parent
/// directory name as a fallback.
/// Case-insensitive extension match. Avoids the `.ends_with(".csproj")`
/// pedantic warning and trivially handles `Foo.CSPROJ` on case-insensitive
/// filesystems.
fn eq_ext(file_name: &str, ext: &str) -> bool {
    std::path::Path::new(file_name)
        .extension()
        .is_some_and(|e| e.eq_ignore_ascii_case(ext))
}

fn anchor_name_from_marker(full_path: &Path, file_name: &str) -> Option<String> {
    let parent_dir_name = || {
        full_path
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|n| n.to_str())
            .map(str::to_string)
    };
    match file_name {
        "Cargo.toml" => parse_cargo_name(full_path).or_else(parent_dir_name),
        "package.json" => parse_package_json_name(full_path).or_else(parent_dir_name),
        "go.mod" => parse_go_mod_name(full_path).or_else(parent_dir_name),
        "pyproject.toml" => parse_pyproject_name(full_path).or_else(parent_dir_name),
        // C#: .csproj and .sln files. The stem is the standard
        // anchor name (e.g. `Foo.Bar.csproj` → `Foo.Bar`). C#
        // workspaces already get a real `pkg` anchor from the JSONL
        // indexer; this marker is a fallback for the rare case where
        // `pkg = 0` slipped through.
        f if eq_ext(f, "csproj") || eq_ext(f, "sln") => full_path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(str::to_string),
        _ => None,
    }
}

#[derive(Deserialize)]
struct CargoToml {
    package: Option<CargoPackage>,
    workspace: Option<CargoWorkspace>,
}
#[derive(Deserialize)]
struct CargoPackage {
    name: Option<String>,
}
#[derive(Deserialize)]
struct CargoWorkspace {}

fn parse_cargo_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let parsed: CargoToml = toml::from_str(&text).ok()?;
    if let Some(pkg) = parsed.package {
        if let Some(name) = pkg.name {
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    // workspace-only Cargo.toml has no package name — fall through.
    let _ = parsed.workspace;
    None
}

#[derive(Deserialize)]
struct PackageJson {
    name: Option<String>,
}

fn parse_package_json_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let pj: PackageJson = serde_json::from_str(&text).ok()?;
    let name = pj.name?;
    if name.is_empty() {
        None
    } else {
        // The name is returned WHOLE, scope included. It used to strip
        // `@scope/` for readability, and that gave one package two anchor names:
        // a symbol the TypeScript producer attributed to a package carries its
        // `pkg_id`, whose `packages` row holds the full `@nestjs/core`, while a
        // symbol with no `pkg_id` (a markdown doc, say) falls through to this
        // marker and got `core`. Both anchors then appear in the graph, so
        // `kenn packages` listed 26 packages for a 17-package repo — nine of them
        // bare-name duplicates of scoped ones already in the list.
        //
        // Stripping is also lossy: `@a/utils` and `@b/utils` are different
        // packages that collapse onto one `utils` anchor. Concept ids already
        // carry the scope (`packages/typescript_@acme_web`), so keeping it here is
        // what makes the two naming paths agree.
        Some(name)
    }
}

fn parse_go_mod_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("module") {
            let candidate = rest.trim().trim_matches(['"', '\'', ' ']).to_string();
            if !candidate.is_empty() {
                // Prefer the last `/`-segment so `github.com/foo/bar`
                // anchors as `bar`.
                let last = candidate
                    .rsplit('/')
                    .next()
                    .unwrap_or(&candidate)
                    .to_string();
                return Some(last);
            }
        }
    }
    None
}

#[derive(Deserialize)]
struct Pyproject {
    project: Option<PyprojectProject>,
    tool: Option<PyprojectTool>,
}
#[derive(Deserialize)]
struct PyprojectProject {
    name: Option<String>,
}
#[derive(Deserialize)]
struct PyprojectTool {
    poetry: Option<PyprojectPoetry>,
}
#[derive(Deserialize)]
struct PyprojectPoetry {
    name: Option<String>,
}

fn parse_pyproject_name(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let py: Pyproject = toml::from_str(&text).ok()?;
    if let Some(p) = py.project {
        if let Some(name) = p.name {
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    if let Some(tool) = py.tool {
        if let Some(poetry) = tool.poetry {
            if let Some(name) = poetry.name {
                if !name.is_empty() {
                    return Some(name);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(path: &Path, content: &str) {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, content).unwrap();
    }

    /// Go's unit of import is the directory: one `go.mod` covers every package
    /// in the module, so anchoring by manifest alone collapses them all and every
    /// edge between them becomes intra-anchor and disappears from the package
    /// graph. Layout mirrors spf13/afero. Mutation-checked: dropping the
    /// `go.package_markers()` extend leaves every path anchored to `afero`.
    #[test]
    fn go_source_dirs_anchor_per_package_not_per_module() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(&root.join("go.mod"), "module github.com/spf13/afero\n");
        write(&root.join("afero.go"), "package afero\n");
        write(&root.join("mem/file.go"), "package mem\n");
        write(&root.join("zipfs/zipfs.go"), "package zipfs\n");
        write(&root.join("internal/common/common.go"), "package common\n");
        // A directory with no `.go` file is NOT a package.
        write(&root.join("docs/readme.md"), "# docs\n");

        let layout = PackageLayout::discover(root, &[]);
        assert_eq!(layout.anchor_for("afero.go"), Some("afero"), "module root");
        assert_eq!(layout.anchor_for("mem/file.go"), Some("afero/mem"));
        assert_eq!(layout.anchor_for("zipfs/zipfs.go"), Some("afero/zipfs"));
        assert_eq!(
            layout.anchor_for("internal/common/common.go"),
            Some("afero/internal/common"),
            "nested package keeps its full path"
        );
        // A non-Go directory falls back to the enclosing module.
        assert_eq!(layout.anchor_for("docs/readme.md"), Some("afero"));
    }

    /// The rule is Go-only: a Rust workspace already has one manifest per unit,
    /// and a stray `.go` file must not invent an anchor where no module exists.
    #[test]
    fn go_markers_require_an_enclosing_module() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            &root.join("crates/a/Cargo.toml"),
            "[package]\nname = \"a\"\n",
        );
        write(&root.join("crates/a/src/lib.rs"), "");
        // No go.mod anywhere — this dir gets no synthesized anchor.
        write(&root.join("scripts/tool.go"), "package main\n");

        let layout = PackageLayout::discover(root, &[]);
        assert_eq!(layout.anchor_for("crates/a/src/lib.rs"), Some("a"));
        assert_eq!(
            layout.anchor_for("scripts/tool.go"),
            None,
            "a .go file outside any module anchors nothing"
        );
    }

    #[test]
    fn depth_helper_handles_root_and_nested() {
        assert_eq!(depth_of(""), 0);
        assert_eq!(depth_of("crates"), 1);
        assert_eq!(depth_of("crates/kenn-indexer"), 2);
        assert_eq!(depth_of("a/b/c/d"), 4);
    }

    #[test]
    fn path_is_under_handles_boundaries() {
        assert!(path_is_under(
            "crates/kenn-indexer/src/foo.rs",
            "crates/kenn-indexer"
        ));
        assert!(path_is_under("crates/kenn-indexer", "crates/kenn-indexer"));
        assert!(!path_is_under(
            "crates/kenn-indexer-other/x",
            "crates/kenn-indexer"
        ));
        assert!(path_is_under("anything/at/all", ""));
    }

    #[test]
    fn discovers_cargo_toml_and_uses_declared_name() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            &root.join("crates/foo/Cargo.toml"),
            r#"[package]
name = "foo-pkg"
version = "0.1.0"
"#,
        );
        write(&root.join("crates/foo/src/lib.rs"), "");
        let layout = PackageLayout::discover(root, &[]);
        assert_eq!(layout.anchor_for("crates/foo/src/lib.rs"), Some("foo-pkg"));
    }

    /// A scoped package keeps its scope, because the OTHER path to an anchor name
    /// — a symbol's `pkg_id` → `packages` row — carries the full `@scope/name`.
    /// Stripping here gave one package two anchors, so `kenn packages` reported 26
    /// packages for a 17-package repo: nine bare-name duplicates of scoped ones.
    ///
    /// Mutation-checked: restoring the strip makes this `Some("util")`.
    #[test]
    fn discovers_package_json_keeps_the_scope() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(
            &root.join("packages/util/package.json"),
            r#"{"name":"@myscope/util"}"#,
        );
        let layout = PackageLayout::discover(root, &[]);
        assert_eq!(
            layout.anchor_for("packages/util/src/index.ts"),
            Some("@myscope/util")
        );
    }

    /// Two scopes can publish the same leaf name. Stripping collapsed them onto
    /// one anchor, merging two unrelated packages into a single node.
    #[test]
    fn same_leaf_name_in_two_scopes_stays_two_packages() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        write(&root.join("a/package.json"), r#"{"name":"@alpha/utils"}"#);
        write(&root.join("b/package.json"), r#"{"name":"@beta/utils"}"#);
        let layout = PackageLayout::discover(root, &[]);
        assert_eq!(layout.anchor_for("a/src/index.ts"), Some("@alpha/utils"));
        assert_eq!(layout.anchor_for("b/src/index.ts"), Some("@beta/utils"));
    }

    #[test]
    fn deepest_marker_wins() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        // Root-level package.json shadows the outer name; inner crate Cargo.toml
        // shadows that for files under it.
        write(&root.join("package.json"), r#"{"name":"outer"}"#);
        write(
            &root.join("crates/inner/Cargo.toml"),
            r#"[package]
name = "inner"
version = "0"
"#,
        );
        let layout = PackageLayout::discover(root, &[]);
        assert_eq!(layout.anchor_for("README.md"), Some("outer"));
        assert_eq!(layout.anchor_for("crates/inner/src/lib.rs"), Some("inner"));
        assert_eq!(
            layout.anchor_for("crates/other/src/lib.rs"),
            Some("outer"),
            "no marker at crates/other — falls back to nearest ancestor marker (root)",
        );
    }

    #[test]
    fn workspace_only_cargo_toml_uses_dir_name() {
        let dir = TempDir::new().unwrap();
        let root = dir.path();
        // Workspace-only Cargo.toml has no [package] name.
        write(
            &root.join("Cargo.toml"),
            r#"[workspace]
members = ["crates/*"]
"#,
        );
        write(
            &root.join("crates/foo/Cargo.toml"),
            r#"[package]
name = "foo"
version = "0"
"#,
        );
        let layout = PackageLayout::discover(root, &[]);
        // Workspace-only marker should anchor to root dir name.
        let root_name = root.file_name().unwrap().to_string_lossy().to_string();
        assert_eq!(layout.anchor_for("README.md"), Some(root_name.as_str()));
        assert_eq!(layout.anchor_for("crates/foo/src/lib.rs"), Some("foo"));
    }

    #[test]
    fn missing_layout_returns_none() {
        let dir = TempDir::new().unwrap();
        let layout = PackageLayout::discover(dir.path(), &[]);
        assert!(layout.is_empty());
        assert_eq!(layout.anchor_for("anything/at/all"), None);
    }

    /// `parse_pyproject_name` reads either PEP 621 (`[project] name`)
    /// or Poetry (`[tool.poetry] name`) and falls through to None for
    /// missing, empty, or unreadable files. Cover each branch.
    #[test]
    fn parse_pyproject_name_reads_pep621() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("pyproject.toml");
        fs::write(&p, "[project]\nname = \"my-pkg\"\n").unwrap();
        assert_eq!(parse_pyproject_name(&p).as_deref(), Some("my-pkg"));
    }

    #[test]
    fn parse_pyproject_name_reads_poetry() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("pyproject.toml");
        fs::write(&p, "[tool.poetry]\nname = \"poetry-pkg\"\n").unwrap();
        assert_eq!(parse_pyproject_name(&p).as_deref(), Some("poetry-pkg"));
    }

    #[test]
    fn parse_pyproject_name_prefers_project_over_poetry() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("pyproject.toml");
        fs::write(
            &p,
            "[project]\nname = \"first\"\n[tool.poetry]\nname = \"second\"\n",
        )
        .unwrap();
        assert_eq!(parse_pyproject_name(&p).as_deref(), Some("first"));
    }

    #[test]
    fn parse_pyproject_name_skips_empty_names() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("pyproject.toml");
        fs::write(
            &p,
            "[project]\nname = \"\"\n[tool.poetry]\nname = \"fallback\"\n",
        )
        .unwrap();
        assert_eq!(parse_pyproject_name(&p).as_deref(), Some("fallback"));
    }

    #[test]
    fn parse_pyproject_name_no_name_returns_none() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("pyproject.toml");
        fs::write(&p, "[other]\nfoo = \"bar\"\n").unwrap();
        assert!(parse_pyproject_name(&p).is_none());
    }

    #[test]
    fn parse_pyproject_name_missing_file_returns_none() {
        let dir = TempDir::new().unwrap();
        assert!(parse_pyproject_name(&dir.path().join("nope.toml")).is_none());
    }

    #[test]
    fn parse_pyproject_name_malformed_toml_returns_none() {
        let dir = TempDir::new().unwrap();
        let p = dir.path().join("pyproject.toml");
        fs::write(&p, "this is not [valid toml").unwrap();
        assert!(parse_pyproject_name(&p).is_none());
    }
}
