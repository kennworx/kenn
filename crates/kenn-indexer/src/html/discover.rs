//! HTML corpus discovery (mirrors `css::discover`).
//!
//! Expands the configured [`HtmlConfig`] roots into a concrete set of
//! `.html`/`.htm` files, applying the (always-on build/vendor + user) exclude
//! globs. Roots are matched as **globs**: a root naming an existing directory is
//! expanded to `<dir>/**`, and a glob-pattern root is used verbatim. HTML owns
//! its discovery (like markdown / stylesheets) — it does not route through
//! `Workspace::is_excluded`.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_config::HtmlConfig;
use kenn_model::Language;

/// One discovered HTML file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredHtml {
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Workspace-relative, `/`-normalized path — the `<relpath>` of the node id.
    pub relpath: String,
}

#[derive(Debug, thiserror::Error)]
pub enum HtmlDiscoverError {
    #[error("invalid html root glob `{pattern}`: {source}")]
    BadRoot {
        pattern: String,
        source: globset::Error,
    },
    #[error("invalid html exclude glob `{pattern}`: {source}")]
    BadExclude {
        pattern: String,
        source: globset::Error,
    },
}

/// Discover every HTML file whose relpath matches a configured root glob, with
/// the effective excludes (always-on build/vendor denies merged with user
/// excludes) applied. The caller gates on `config.enabled`.
pub fn discover_html(
    config: &HtmlConfig,
    workspace_root: &Path,
) -> Result<Vec<DiscoveredHtml>, HtmlDiscoverError> {
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

/// Build the include `GlobSet` from roots. A root naming an existing directory
/// expands to `<dir>/**` (or `**` for `.`); any other root is a verbatim glob.
fn build_include_set(
    roots: &[String],
    workspace_root: &Path,
) -> Result<GlobSet, HtmlDiscoverError> {
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
        let glob = Glob::new(&pattern).map_err(|source| HtmlDiscoverError::BadRoot {
            pattern: pattern.clone(),
            source,
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|source| HtmlDiscoverError::BadRoot {
            pattern: "<root-set>".to_string(),
            source,
        })
}

fn build_exclude_set(patterns: &[String]) -> Result<GlobSet, HtmlDiscoverError> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat).map_err(|source| HtmlDiscoverError::BadExclude {
            pattern: pat.clone(),
            source,
        })?;
        builder.add(glob);
    }
    builder
        .build()
        .map_err(|source| HtmlDiscoverError::BadExclude {
            pattern: "<exclude-set>".to_string(),
            source,
        })
}

/// True when `path`'s extension is one of HTML's (`.html`/`.htm`).
fn is_html(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| Language::Html.extensions().contains(&e.as_str()))
}

fn relpath_of(path: &Path, workspace_root: &Path) -> Option<String> {
    let rel = path.strip_prefix(workspace_root).ok()?;
    Some(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )
}

/// Recursively walk `dir`, pruning excluded directories and emitting HTML files
/// that match the include set. Symlinks are not followed.
fn walk_dir(
    dir: &Path,
    workspace_root: &Path,
    includes: &GlobSet,
    excludes: &GlobSet,
    out: &mut Vec<DiscoveredHtml>,
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
        } else if file_type.is_file() && is_html(&path) && includes.is_match(&rel) {
            out.push(DiscoveredHtml {
                abs_path: path,
                relpath: rel,
            });
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

    fn cfg(roots: Vec<&str>) -> HtmlConfig {
        HtmlConfig {
            enabled: true,
            roots: roots.into_iter().map(String::from).collect(),
            ..Default::default()
        }
    }

    #[test]
    fn discovers_html_and_htm_and_skips_build_output() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "pages/index.html", "<html></html>");
        write(ws.path(), "pages/legacy.htm", "<html></html>");
        write(ws.path(), "pages/notes.txt", "x");
        write(ws.path(), "dist/index.html", "<html></html>"); // always-on deny

        let found = discover_html(&cfg(vec!["."]), ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["pages/index.html", "pages/legacy.htm"]);
    }

    #[test]
    fn directory_named_root_scopes_to_that_directory() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/a.html", "<html></html>");
        write(ws.path(), "vendor/b.html", "<html></html>");
        let found = discover_html(&cfg(vec!["src"]), ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["src/a.html"]);
    }

    #[test]
    fn user_exclude_does_not_drop_build_output_denies() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "src/keep.html", "<html></html>");
        write(ws.path(), "node_modules/dep/x.html", "<html></html>");
        let mut config = cfg(vec!["."]);
        config.excludes = vec!["legacy/**".into()];
        let found = discover_html(&config, ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["src/keep.html"]);
    }
}
