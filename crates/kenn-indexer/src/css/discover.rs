//! Stylesheet corpus discovery.
//!
//! Expands the configured [`CssConfig`] roots into a concrete set of
//! `.css`/`.scss`/`.sass` files, applying the (always-on build/vendor +
//! user) exclude globs. Roots are matched as **globs**: a root naming an
//! existing directory is expanded to `<dir>/**` (everything beneath it), and a
//! root that is a glob pattern (`src/**/*.{css,scss}`) is used verbatim. Each
//! file carries its workspace-relative path and the [`Language`] its extension
//! implies (`.css` → `Css`, `.scss`/`.sass` → `Sass`).
//!
//! Stylesheets own their own discovery (like markdown): they do not route
//! through `Workspace::is_excluded`.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_config::CssConfig;
use kenn_model::Language;

/// One discovered stylesheet file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredStylesheet {
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Workspace-relative, `/`-normalized path — the `<relpath>` of the node id.
    pub relpath: String,
    /// `Css` for `.css`, `Sass` for `.scss`/`.sass`.
    pub language: Language,
}

#[derive(Debug, thiserror::Error)]
pub enum CssDiscoverError {
    #[error("invalid css root glob `{pattern}`: {source}")]
    BadRoot {
        pattern: String,
        source: globset::Error,
    },
    #[error("invalid css exclude glob `{pattern}`: {source}")]
    BadExclude {
        pattern: String,
        source: globset::Error,
    },
}

/// Discover every stylesheet whose relpath matches a configured root glob, with
/// the effective excludes (always-on build/vendor denies merged with user
/// excludes) applied. The caller gates on `config.enabled`.
pub fn discover_stylesheets(
    config: &CssConfig,
    workspace_root: &Path,
) -> Result<Vec<DiscoveredStylesheet>, CssDiscoverError> {
    let includes = build_include_set(&config.roots, workspace_root)?;
    let excludes = build_exclude_set(&config.effective_excludes())?;
    let mut out = Vec::new();
    walk_dir(
        workspace_root,
        workspace_root,
        &includes,
        &excludes,
        &mut out,
    );
    out.sort_by(|a, b| a.abs_path.cmp(&b.abs_path));
    out.dedup();
    Ok(out)
}

/// Discover the files to scan for class usage (Phase 2), matching the
/// `usage_sources` globs (any file type — usage is language-agnostic) minus the
/// effective excludes. Returns `(abs_path, workspace-relpath)` pairs. Empty when
/// `usage_sources` is unset (the explicit opt-in — usage mining stays off).
pub(crate) fn discover_usage_sources(
    config: &CssConfig,
    workspace_root: &Path,
) -> Result<Vec<(PathBuf, String)>, CssDiscoverError> {
    if config.usage_sources.is_empty() {
        return Ok(Vec::new());
    }
    let includes = build_include_set(&config.usage_sources, workspace_root)?;
    let excludes = build_exclude_set(&config.effective_excludes())?;
    let mut out = Vec::new();
    walk_usage(
        workspace_root,
        workspace_root,
        &includes,
        &excludes,
        &mut out,
    );
    out.sort();
    out.dedup();
    Ok(out)
}

/// Walk for `usage_sources`: any file (no extension filter) whose relpath
/// matches the include set and isn't excluded — except indexed-HTML extensions
/// (`.html`/`.htm`), which the HTML parser owns the class usage of (design D5,
/// task 6.1a). Including them here would double-emit `uses_css_class` edges.
fn walk_usage(
    dir: &Path,
    workspace_root: &Path,
    includes: &GlobSet,
    excludes: &GlobSet,
    out: &mut Vec<(PathBuf, String)>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_symlink() {
            continue;
        }
        let Some(rel) = relpath_of(&path, workspace_root) else {
            continue;
        };
        if excludes.is_match(&rel) {
            continue;
        }
        if ft.is_dir() {
            walk_usage(&path, workspace_root, includes, excludes, out);
        } else if ft.is_file() && includes.is_match(&rel) && !is_indexed_html(&path) {
            out.push((path, rel));
        }
    }
}

/// Whether `path` is an indexed-HTML file (`.html`/`.htm`) — excluded from the
/// raw usage scan so the HTML parser is the sole source of HTML class usage
/// (design D5).
fn is_indexed_html(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        Language::Html
            .extensions()
            .contains(&e.to_ascii_lowercase().as_str())
    })
}

/// Build the include `GlobSet` from roots. A root naming an existing directory
/// expands to `<dir>/**` (or `**` for `.`); any other root is treated verbatim
/// as a glob pattern.
fn build_include_set(roots: &[String], workspace_root: &Path) -> Result<GlobSet, CssDiscoverError> {
    let mut builder = GlobSetBuilder::new();
    for root in roots {
        let trimmed = root.trim_end_matches('/');
        let pattern = if workspace_root.join(root).is_dir() {
            if trimmed.is_empty() || trimmed == "." {
                "**".to_string()
            } else {
                format!("{trimmed}/**")
            }
        } else {
            root.clone()
        };
        let glob = Glob::new(&pattern).map_err(|source| CssDiscoverError::BadRoot {
            pattern: pattern.clone(),
            source,
        })?;
        builder.add(glob);
    }
    builder.build().map_err(|source| CssDiscoverError::BadRoot {
        pattern: "<root-set>".to_string(),
        source,
    })
}

fn build_exclude_set(patterns: &[String]) -> Result<GlobSet, CssDiscoverError> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat).map_err(|source| CssDiscoverError::BadExclude {
            pattern: pat.clone(),
            source,
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|source| CssDiscoverError::BadExclude {
            pattern: "<exclude-set>".to_string(),
            source,
        })
}

/// The stylesheet language implied by a path's extension, or `None`.
fn stylesheet_language(path: &Path) -> Option<Language> {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    if Language::Css.extensions().contains(&ext.as_str()) {
        Some(Language::Css)
    } else if Language::Sass.extensions().contains(&ext.as_str()) {
        Some(Language::Sass)
    } else {
        None
    }
}

fn relpath_of(path: &Path, workspace_root: &Path) -> Option<String> {
    let rel = path.strip_prefix(workspace_root).ok()?;
    Some(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )
}

/// Recursively walk `dir`, pruning excluded directories and emitting stylesheet
/// files that match the include set into `out`. Symlinks are not followed.
fn walk_dir(
    dir: &Path,
    workspace_root: &Path,
    includes: &GlobSet,
    excludes: &GlobSet,
    out: &mut Vec<DiscoveredStylesheet>,
) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if file_type.is_symlink() {
            continue;
        }
        let Some(rel) = relpath_of(&path, workspace_root) else {
            continue;
        };
        if excludes.is_match(&rel) {
            continue;
        }
        if file_type.is_dir() {
            walk_dir(&path, workspace_root, includes, excludes, out);
        } else if file_type.is_file() {
            if let Some(language) = stylesheet_language(&path) {
                if includes.is_match(&rel) {
                    out.push(DiscoveredStylesheet {
                        abs_path: path,
                        relpath: rel,
                        language,
                    });
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn cfg(roots: Vec<&str>) -> CssConfig {
        CssConfig {
            enabled: true,
            roots: roots.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn directory_root_discovers_css_scss_sass_and_skips_build_output() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/a.css", ".a{}");
        write(ws.path(), "src/b.scss", ".b{}");
        write(ws.path(), "src/c.sass", ".c");
        write(ws.path(), "src/notes.txt", "x");
        write(ws.path(), "dist/a.css", ".a{}"); // always-on deny

        let found = discover_stylesheets(&cfg(vec!["."]), ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["src/a.css", "src/b.scss", "src/c.sass"]);
        assert_eq!(found[0].language, Language::Css);
        assert_eq!(found[1].language, Language::Sass);
    }

    #[test]
    fn directory_named_root_scopes_to_that_directory() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/a.css", ".a{}");
        write(ws.path(), "vendor/b.css", ".b{}");
        let found = discover_stylesheets(&cfg(vec!["src"]), ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["src/a.css"]); // vendor/ not under the root
    }

    #[test]
    fn glob_root_pattern_is_honored() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/a.css", ".a{}");
        write(ws.path(), "src/deep/b.scss", ".b{}");
        write(ws.path(), "src/c.sass", ".c"); // excluded by the css/scss glob
                                              // A glob root (not a directory) — the documented config shape.
        let found = discover_stylesheets(&cfg(vec!["src/**/*.{css,scss}"]), ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["src/a.css", "src/deep/b.scss"]);
    }

    /// Task 6.1a: indexed-HTML extensions are excluded from the raw usage scan,
    /// so the HTML parser is the sole source of HTML class usage (no double edge).
    #[test]
    fn usage_scan_excludes_indexed_html() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/app.ts", "const c = 'btn';");
        write(ws.path(), "src/page.html", "<div class=\"btn\">");
        write(ws.path(), "src/legacy.htm", "<div class=\"btn\">");
        let mut config = cfg(vec!["."]);
        config.usage_sources = vec!["**/*".into()];
        let found = discover_usage_sources(&config, ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|(_, r)| r.as_str()).collect();
        assert!(rels.contains(&"src/app.ts"), "code source scanned");
        assert!(
            !rels.contains(&"src/page.html"),
            ".html excluded from raw scan"
        );
        assert!(
            !rels.contains(&"src/legacy.htm"),
            ".htm excluded from raw scan"
        );
    }

    #[test]
    fn user_exclude_does_not_drop_build_output_denies() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/keep.css", ".k{}");
        write(ws.path(), "node_modules/dep/x.css", ".x{}");
        let mut config = cfg(vec!["."]);
        config.excludes = vec!["legacy/**".into()];
        let found = discover_stylesheets(&config, ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["src/keep.css"]);
    }
}
