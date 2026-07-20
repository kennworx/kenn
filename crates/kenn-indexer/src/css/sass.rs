//! dart-sass integration for the stylesheet producer.
//!
//! `.scss`/`.sass` have no robust off-the-shelf Rust parser (see the kenn
//! directives on `crates/kenn-indexer/src/css`), so they are compiled to CSS by
//! the dart-sass compiler and the output is parsed by lightningcss like a `.css`
//! file. This module locates the compiler from the natural places a JS/Sass
//! project keeps it; compilation + source-map back-mapping land in later steps.
//!
//! The compiler is invoked via its stable CLI, NOT the `sass-embedded` Rust
//! crate (proven protocol-stale against current dart-sass).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;

use kenn_config::SassConfig;
use kenn_model::id::css::{module_id, selector_id};
use kenn_model::{
    DefRecord, EdgeProperties, EdgeRecord, FileRecord, Kind, Language, ShortId, SymbolDocsRecord,
    SymbolRecord,
};

use super::discover::DiscoveredStylesheet;
use super::parse::{collect_atoms, def, kind_of, preceding_comment, selector_text, symbol, CssIds};

#[derive(Debug, thiserror::Error)]
pub enum SassError {
    #[error("sass compile failed for `{entry}`: {message}")]
    Compile { entry: String, message: String },
    #[error("sass output io for `{entry}`: {source}")]
    Io {
        entry: String,
        source: std::io::Error,
    },
}

/// Locate the `sass` compiler, in priority order:
/// 1. a configured override (`[language.css.sass] compiler`);
/// 2. the project's `node_modules/.bin/sass` shim;
/// 3. the dart-sass binary inside a `node_modules/sass-embedded-<platform>/`
///    package;
/// 4. `sass` on `PATH`;
/// 5. a kenn-bundled `build/kenn-sass`.
///
/// Returns `None` when none is found (the caller leaves `.scss`/`.sass`
/// unindexed with a clear log rather than failing the run).
#[must_use]
pub fn discover_sass_compiler(config: &SassConfig, workspace_root: &Path) -> Option<PathBuf> {
    // 1. Configured override (absolute, or relative to the workspace).
    if let Some(p) = &config.compiler {
        let abs = if p.is_absolute() {
            p.clone()
        } else {
            workspace_root.join(p)
        };
        if abs.is_file() {
            return Some(abs);
        }
    }

    let node_modules = workspace_root.join("node_modules");

    // 2. The project's dev-dependency shim (both `sass` and `sass-embedded`
    //    npm packages install a `.bin/sass`).
    let shim = node_modules.join(".bin").join("sass");
    if shim.is_file() {
        return Some(shim);
    }

    // 3. The raw dart-sass binary inside a platform-specific embedded package.
    if let Some(p) = dart_sass_in_packages(&node_modules) {
        return Some(p);
    }

    // 4. `sass` on PATH.
    if let Some(p) = on_path("sass") {
        return Some(p);
    }

    // 5. A kenn-bundled binary (built by a `build-indexer-sass` recipe).
    let bundled = workspace_root.join("build").join("kenn-sass");
    if bundled.is_file() {
        return Some(bundled);
    }

    None
}

/// Whether a Sass file is a compile **entry point** rather than a partial.
/// Sass partials are named `_name.scss`/`_name.sass` and are pulled in by
/// entries via `@use`/`@import`; only entries are compiled directly (a partial's
/// selectors reach the registry through the entry's compiled output, attributed
/// back to the partial by the source map).
#[must_use]
pub fn is_sass_entry(relpath: &str) -> bool {
    let base = relpath.rsplit('/').next().unwrap_or(relpath);
    !base.starts_with('_')
}

/// The dart-sass binary inside a `sass-embedded-<platform>` package under
/// `node_modules`, if present (`…/dart-sass/sass`).
fn dart_sass_in_packages(node_modules: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(node_modules).ok()?;
    for entry in entries.flatten() {
        if entry
            .file_name()
            .to_string_lossy()
            .starts_with("sass-embedded-")
        {
            let candidate = entry.path().join("dart-sass").join("sass");
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// Accumulates Sass nodes across all compiled entries, deduping origin files
/// (a partial `@use`d by several entries appears once) and selectors by `pub_id`.
#[derive(Default)]
pub(crate) struct SassExtract {
    pub files: Vec<FileRecord>,
    pub symbols: Vec<SymbolRecord>,
    pub defs: Vec<DefRecord>,
    pub edges: Vec<EdgeRecord>,
    pub docs: Vec<SymbolDocsRecord>,
    module_of: HashMap<String, ShortId>,
    selector_of: HashMap<String, ShortId>,
    content_of: HashMap<String, String>,
}

impl SassExtract {
    /// Ensure the origin file's `module` node + `FileRecord` exist (deduped by
    /// relpath); returns `(file_id, module_sym)`. Caches the file's source text
    /// for comment extraction.
    fn ensure_origin(
        &mut self,
        relpath: &str,
        workspace_root: &Path,
        ids: &mut CssIds,
    ) -> (ShortId, ShortId) {
        if let Some(&module_sym) = self.module_of.get(relpath) {
            // The file id is the module's `contains` target — but we only need
            // it again to attach defs; recompute via the cached map.
            let file_id = self
                .files
                .iter()
                .find(|f| f.path == relpath)
                .map_or(0, |f| f.id);
            return (file_id, module_sym);
        }
        let content = std::fs::read_to_string(workspace_root.join(relpath)).unwrap_or_default();
        let total = u32::try_from(content.lines().count())
            .unwrap_or(u32::MAX)
            .max(1);
        let file_id = ids.file_id(Language::Sass);
        let module_sym = ids.symbol_id(Language::Sass);
        self.files.push(FileRecord {
            id: file_id,
            path: relpath.to_string(),
            language: Language::Sass,
            test: false,
            external: false,
            content_hash: xxhash_rust::xxh3::xxh3_64(content.as_bytes()),
        });
        self.symbols.push(symbol(
            module_sym,
            crate::pubid::floor(&module_id(Language::Sass, relpath).into_string()),
            Language::Sass,
            Kind::Module,
            relpath.rsplit('/').next().unwrap_or(relpath).to_string(),
            0,
        ));
        self.defs.push(def(module_sym, file_id, 1, total));
        self.edges.push(EdgeRecord {
            src_id: module_sym,
            target_id: file_id,
            properties: EdgeProperties::Contains,
        });
        self.module_of.insert(relpath.to_string(), module_sym);
        self.content_of.insert(relpath.to_string(), content);
        (file_id, module_sym)
    }

    /// Add a selector node for `(relpath, kind, name)` if new, with its def at
    /// the origin `src_line` (0-based) and a comment doc when present.
    fn add_selector(
        &mut self,
        relpath: &str,
        kind: kenn_model::id::css::SelectorKind,
        name: &str,
        src_line: u32,
        origin: (ShortId, ShortId),
        ids: &mut CssIds,
    ) {
        let (file_id, module_sym) = origin;
        let pub_id =
            crate::pubid::floor(&selector_id(Language::Sass, relpath, kind, name).into_string());
        if self.selector_of.contains_key(&pub_id) {
            return;
        }
        let sym = ids.symbol_id(Language::Sass);
        self.selector_of.insert(pub_id.clone(), sym);

        // Comment doc from the cached origin source (borrow ends before push).
        let doc = self.content_of.get(relpath).map_or(String::new(), |c| {
            let lines: Vec<&str> = c.lines().collect();
            preceding_comment(&lines, src_line as usize)
        });
        let line = src_line.saturating_add(1); // 0-based → 1-based def line
        if !doc.is_empty() {
            self.docs.push(SymbolDocsRecord {
                sym_id: sym,
                sig: selector_text(kind, name),
                doc,
            });
        }
        self.symbols.push(symbol(
            sym,
            pub_id,
            Language::Sass,
            kind_of(kind),
            name.to_string(),
            module_sym,
        ));
        self.defs.push(def(sym, file_id, line, line));
        self.edges.push(EdgeRecord {
            src_id: sym,
            target_id: module_sym,
            properties: EdgeProperties::DefinedIn,
        });
    }

    /// Distinct origin files indexed so far (for the run report file count).
    pub(crate) fn file_count(&self) -> u64 {
        self.files.len() as u64
    }

    /// The `relpath → module id` map for every origin file (for CSS-internal
    /// import resolution).
    pub(crate) fn module_map(&self) -> std::collections::HashMap<String, ShortId> {
        self.module_of.clone()
    }

    /// Ensure a `module` node exists for `relpath` even if it contributed no
    /// selectors (a barrel entry that only `@import`s, or a functions-only
    /// partial) — so it can be the source/target of `imports` edges.
    pub(crate) fn ensure_module(&mut self, relpath: &str, workspace_root: &Path, ids: &mut CssIds) {
        let _ = self.ensure_origin(relpath, workspace_root, ids);
    }
}

/// Compile one Sass `entry` with dart-sass and extract its (and its partials')
/// selectors into `acc`, attributing each to its origin `.scss`/`.sass` file via
/// the source map. dart-sass handles both `.scss` and indented `.sass`.
pub(crate) fn compile_and_extract(
    compiler: &Path,
    entry_abs: &Path,
    load_paths: &[String],
    workspace_root: &Path,
    ids: &mut CssIds,
    acc: &mut SassExtract,
) -> Result<(), SassError> {
    let entry_label = entry_abs.to_string_lossy().into_owned();
    let out_dir = workspace_root.join(".kenn/local/sass-build");
    std::fs::create_dir_all(&out_dir).map_err(|source| SassError::Io {
        entry: entry_label.clone(),
        source,
    })?;
    let (out_css, out_map) = out_paths(&out_dir, entry_abs);

    let mut cmd = sass_command(compiler, load_paths, workspace_root);
    cmd.arg(format!("{}:{}", entry_abs.display(), out_css.display()));
    let output = cmd.output().map_err(|source| SassError::Io {
        entry: entry_label.clone(),
        source,
    })?;
    if !output.status.success() {
        return Err(SassError::Compile {
            entry: entry_label,
            message: String::from_utf8_lossy(&output.stderr)
                .lines()
                .take(3)
                .collect::<Vec<_>>()
                .join(" | "),
        });
    }
    let ws_canon = workspace_root
        .canonicalize()
        .unwrap_or_else(|_| workspace_root.to_path_buf());
    extract_compiled(
        &out_css,
        &out_map,
        &out_dir,
        &ws_canon,
        workspace_root,
        ids,
        acc,
    )
}

/// Compile ALL `entries` in a SINGLE dart-sass invocation (`sass in:out in:out
/// …`) and extract each. Batching amortizes the per-spawn startup across every
/// entry (Bulma: 64 spawns → 1). If the batch fails — one bad entry sinks the
/// whole invocation and dart-sass doesn't tell us which — fall back to per-entry
/// compiles so a single broken file doesn't lose the rest. Per-entry failures
/// are logged.
pub(crate) fn compile_and_extract_batch(
    compiler: &Path,
    entries: &[&DiscoveredStylesheet],
    load_paths: &[String],
    workspace_root: &Path,
    ids: &mut CssIds,
    acc: &mut SassExtract,
) {
    if entries.is_empty() {
        return;
    }
    let out_dir = workspace_root.join(".kenn/local/sass-build");
    if std::fs::create_dir_all(&out_dir).is_err() {
        return;
    }
    let pairs: Vec<(PathBuf, PathBuf)> = entries
        .iter()
        .map(|e| out_paths(&out_dir, &e.abs_path))
        .collect();

    let mut cmd = sass_command(compiler, load_paths, workspace_root);
    for (entry, (out_css, _)) in entries.iter().zip(&pairs) {
        cmd.arg(format!(
            "{}:{}",
            entry.abs_path.display(),
            out_css.display()
        ));
    }
    let batch_ok = matches!(cmd.output(), Ok(o) if o.status.success());

    if batch_ok {
        let ws_canon = workspace_root
            .canonicalize()
            .unwrap_or_else(|_| workspace_root.to_path_buf());
        for (out_css, out_map) in &pairs {
            if let Err(e) = extract_compiled(
                out_css,
                out_map,
                &out_dir,
                &ws_canon,
                workspace_root,
                ids,
                acc,
            ) {
                tracing::debug!(target: "kenn_indexer::css", error = %e, "sass extract skipped");
            }
        }
        return;
    }

    tracing::warn!(
        target: "kenn_indexer::css",
        count = entries.len(),
        "batch sass compile failed; falling back to per-entry to isolate the broken file"
    );
    for entry in entries {
        if let Err(e) = compile_and_extract(
            compiler,
            &entry.abs_path,
            load_paths,
            workspace_root,
            ids,
            acc,
        ) {
            tracing::warn!(
                target: "kenn_indexer::css",
                path = %entry.relpath,
                error = %e,
                "sass entry failed to compile, skipped"
            );
        }
    }
}

/// A `sass` command with the shared flags (`--style=expanded --quiet
/// --source-map`) and load paths; callers append the `input:output` pair(s).
fn sass_command(compiler: &Path, load_paths: &[String], workspace_root: &Path) -> Command {
    let mut cmd = Command::new(compiler);
    cmd.arg("--style=expanded")
        .arg("--quiet")
        .arg("--source-map");
    for lp in load_paths {
        cmd.arg(format!("--load-path={}", workspace_root.join(lp).display()));
    }
    cmd
}

/// The `(out.css, out.css.map)` scratch paths for one entry, keyed by a hash of
/// its absolute path.
fn out_paths(out_dir: &Path, entry_abs: &Path) -> (PathBuf, PathBuf) {
    let stem = format!(
        "{:016x}",
        xxhash_rust::xxh3::xxh3_64(entry_abs.to_string_lossy().as_bytes())
    );
    (
        out_dir.join(format!("{stem}.css")),
        out_dir.join(format!("{stem}.css.map")),
    )
}

/// Parse a compiled `out.css` + its source map and extract every selector into
/// `acc`, attributing each to its origin file via the map.
fn extract_compiled(
    out_css: &Path,
    out_map: &Path,
    out_dir: &Path,
    ws_canon: &Path,
    workspace_root: &Path,
    ids: &mut CssIds,
    acc: &mut SassExtract,
) -> Result<(), SassError> {
    let label = out_css.to_string_lossy().into_owned();
    let css = std::fs::read_to_string(out_css).map_err(|source| SassError::Io {
        entry: label.clone(),
        source,
    })?;
    let map_bytes = std::fs::read(out_map).map_err(|source| SassError::Io {
        entry: label.clone(),
        source,
    })?;
    let sm = sourcemap::SourceMap::from_slice(&map_bytes).map_err(|e| SassError::Compile {
        entry: label,
        message: format!("source map parse: {e}"),
    })?;
    let Some(atoms) = collect_atoms(&css) else {
        return Ok(()); // empty/garbage output — nothing to extract
    };
    for atom in atoms {
        let Some(token) = sm.lookup_token(atom.line, atom.col) else {
            continue;
        };
        let Some(src) = token.get_source() else {
            continue;
        };
        let Some(relpath) = resolve_source(src, out_dir, ws_canon) else {
            continue; // origin outside the workspace — can't attribute
        };
        let src_line = token.get_src_line();
        let origin = acc.ensure_origin(&relpath, workspace_root, ids);
        acc.add_selector(&relpath, atom.kind, &atom.name, src_line, origin, ids);
    }
    Ok(())
}

/// Resolve a source-map `sources` entry (relative to the `.map` file's dir, or
/// a `file://`/absolute path) to a workspace-relative, `/`-normalized path.
/// `None` when the origin is outside the workspace.
fn resolve_source(src: &str, out_dir: &Path, ws_canon: &Path) -> Option<String> {
    let raw = src.strip_prefix("file://").unwrap_or(src);
    let p = Path::new(raw);
    let abs = if p.is_absolute() {
        p.to_path_buf()
    } else {
        out_dir.join(p)
    };
    let canon = abs.canonicalize().ok()?;
    let rel = canon.strip_prefix(ws_canon).ok()?;
    Some(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )
}

/// First executable named `name` found on `PATH`.
fn on_path(name: &str) -> Option<PathBuf> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(name))
        .find(|p| p.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn touch_exec(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, "#!/bin/sh\n").unwrap();
    }

    fn sass_cfg(compiler: Option<&str>) -> SassConfig {
        SassConfig {
            compiler: compiler.map(PathBuf::from),
            load_paths: vec![],
        }
    }

    #[test]
    fn prefers_node_modules_bin_shim() {
        let ws = TempDir::new().unwrap();
        let shim = ws.path().join("node_modules/.bin/sass");
        touch_exec(&shim);
        let found = discover_sass_compiler(&sass_cfg(None), ws.path());
        assert_eq!(found.as_deref(), Some(shim.as_path()));
    }

    #[test]
    fn configured_override_wins_over_node_modules() {
        let ws = TempDir::new().unwrap();
        touch_exec(&ws.path().join("node_modules/.bin/sass"));
        let custom = ws.path().join("tools/my-sass");
        touch_exec(&custom);
        let found = discover_sass_compiler(&sass_cfg(Some("tools/my-sass")), ws.path());
        assert_eq!(found.as_deref(), Some(custom.as_path()));
    }

    #[test]
    fn falls_back_to_embedded_package_binary() {
        let ws = TempDir::new().unwrap();
        let dart = ws
            .path()
            .join("node_modules/sass-embedded-darwin-arm64/dart-sass/sass");
        touch_exec(&dart);
        let found = discover_sass_compiler(&sass_cfg(None), ws.path());
        assert_eq!(found.as_deref(), Some(dart.as_path()));
    }

    fn write_file(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    /// End-to-end (gated on a discoverable dart-sass): compile an entry that
    /// `@use`s a partial generating classes via `@each`, and confirm the
    /// generated classes are attributed to the PARTIAL (via the source map),
    /// while the entry's own class is attributed to the entry.
    #[test]
    fn compiles_scss_and_attributes_generated_classes_to_origin() {
        let ws = TempDir::new().unwrap();
        write_file(
            ws.path(),
            "src/main.scss",
            "@use 'buttons';\n.app { color: red }\n",
        );
        write_file(
            ws.path(),
            "src/_buttons.scss",
            "$colors: primary, danger;\n@each $c in $colors {\n  .btn-#{$c} { color: blue }\n}\n",
        );
        let Some(compiler) = discover_sass_compiler(&sass_cfg(None), ws.path()) else {
            eprintln!("no dart-sass discoverable; skipping compile e2e");
            return;
        };

        let mut ids = CssIds::new();
        let mut acc = SassExtract::default();
        compile_and_extract(
            &compiler,
            &ws.path().join("src/main.scss"),
            &[],
            ws.path(),
            &mut ids,
            &mut acc,
        )
        .expect("compile+extract");

        // `@each`-generated classes exist and back-map to the partial.
        let btn = acc
            .symbols
            .iter()
            .find(|s| s.name == "btn-primary")
            .expect("btn-primary generated class");
        assert_eq!(btn.kind, Kind::CssClass);
        assert_eq!(btn.language, Language::Sass);
        assert!(
            btn.pub_id.starts_with("sass:src/_buttons.scss#class:"),
            "generated class attributed to its partial, got {}",
            btn.pub_id
        );
        // The entry's own class back-maps to the entry file.
        let app = acc.symbols.iter().find(|s| s.name == "app").expect("app");
        assert!(app.pub_id.starts_with("sass:src/main.scss#class:"));
        // Both origin files got module + FileRecord nodes.
        assert!(acc.files.iter().any(|f| f.path == "src/_buttons.scss"));
        assert!(acc.files.iter().any(|f| f.path == "src/main.scss"));
    }

    /// A syntactically broken entry yields a compile error the caller skips
    /// (gated on a discoverable dart-sass).
    #[test]
    fn broken_entry_returns_compile_error() {
        let ws = TempDir::new().unwrap();
        write_file(ws.path(), "bad.scss", ".x { color: red\n@@@ not sass\n");
        let Some(compiler) = discover_sass_compiler(&sass_cfg(None), ws.path()) else {
            return;
        };
        let mut ids = CssIds::new();
        let mut acc = SassExtract::default();
        let r = compile_and_extract(
            &compiler,
            &ws.path().join("bad.scss"),
            &[],
            ws.path(),
            &mut ids,
            &mut acc,
        );
        assert!(matches!(r, Err(SassError::Compile { .. })));
    }

    #[test]
    fn entry_points_exclude_partials() {
        assert!(is_sass_entry("src/main.scss"));
        assert!(is_sass_entry("bulma.sass"));
        assert!(!is_sass_entry("src/_variables.scss"));
        assert!(!is_sass_entry("_index.scss"));
        assert!(is_sass_entry("a/b/app.scss")); // nested entry
        assert!(!is_sass_entry("a/b/_mixins.scss")); // nested partial
    }

    #[test]
    fn none_when_no_compiler_anywhere() {
        let ws = TempDir::new().unwrap();
        // No node_modules, no override, no bundled. (PATH may or may not have
        // sass; this test only asserts the no-local-compiler branches return
        // None when PATH also lacks it — guard by checking the result shape.)
        let found = discover_sass_compiler(&sass_cfg(None), ws.path());
        if let Some(p) = found {
            // Only PATH could have produced this; it must be a `sass` binary.
            assert_eq!(p.file_name().unwrap(), "sass");
        }
    }
}
