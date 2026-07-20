//! Markdown corpus discovery.
//!
//! Expands the configured [`MarkdownConfig`] search roots into a concrete set
//! of `.md` files, applying the exclude globs. A root glob naming a directory
//! is walked recursively (`<dir>/**/*.md`); excluded directories are pruned
//! during descent. Each discovered file carries the root `label` and a
//! normalized relative path, which together form its `md:<label>/<relpath>`
//! identity (built later by the walker).
//!
//! Markdown owns its own discovery (design D11): it does not route through
//! `Workspace::is_excluded`; the exclude globs are compiled and applied here.

use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_config::MarkdownConfig;
use kenn_model::Language;

/// One discovered markdown file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredMarkdown {
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Corpus root label (`workspace` for in-repo, or the configured label
    /// for an external vault). First segment of the node id.
    pub label: String,
    /// Path relative to the root's relpath base, `/`-normalized. The
    /// `<relpath>` portion of `md:<label>/<relpath>`.
    pub relpath: String,
    /// Whether this file came from an in-repo root (a relative root glob).
    /// External vault roots (absolute globs) are `false`. Only in-repo files
    /// get md→code resolution (design D6).
    pub in_repo: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum MarkdownDiscoverError {
    #[error("invalid markdown exclude glob `{pattern}`: {source}")]
    BadExclude {
        pattern: String,
        source: globset::Error,
    },
}

/// Discover every `.md`/`.markdown` file across the configured roots, with
/// excludes applied. The caller gates on `config.enabled`; this walks the
/// roots unconditionally.
pub fn discover_markdown(
    config: &MarkdownConfig,
    workspace_root: &Path,
) -> Result<Vec<DiscoveredMarkdown>, MarkdownDiscoverError> {
    let excludes = build_exclude_set(&config.excludes)?;
    let includes = build_exclude_set(&config.includes)?;
    let has_includes = !config.includes.is_empty();
    let mut out = Vec::new();
    for root in &config.roots {
        let Some(plan) =
            RootPlan::resolve(root.glob.as_str(), root.label.as_deref(), workspace_root)
        else {
            // Glob doesn't resolve to an existing directory; nothing to walk.
            // (General file-glob patterns are a later extension.)
            continue;
        };
        walk_dir(
            &plan.base,
            &plan,
            &excludes,
            &includes,
            has_includes,
            &mut out,
        );
    }
    out.sort_by(|a, b| a.abs_path.cmp(&b.abs_path));
    out.dedup();
    Ok(out)
}

/// A resolved root ready to walk.
struct RootPlan {
    /// Directory to walk.
    base: PathBuf,
    /// Directory that relpaths are computed against (the workspace root for
    /// an in-repo root so paths read `docs/x.md`; the base itself for an
    /// external vault).
    relpath_root: PathBuf,
    /// Node-id label for files under this root.
    label: String,
    /// Whether this root is in-repo (a relative glob) vs an external vault.
    in_repo: bool,
}

impl RootPlan {
    /// Resolve a root glob to a walkable directory plan, or `None` if it does
    /// not name an existing directory.
    fn resolve(glob: &str, label: Option<&str>, workspace_root: &Path) -> Option<Self> {
        let candidate = Path::new(glob);
        if candidate.is_absolute() {
            let base = candidate.to_path_buf();
            base.is_dir().then(|| {
                let label = label.map_or_else(
                    || {
                        base.file_name().map_or_else(
                            || "vault".to_string(),
                            |n| n.to_string_lossy().into_owned(),
                        )
                    },
                    ToString::to_string,
                );
                Self {
                    relpath_root: base.clone(),
                    base,
                    label,
                    in_repo: false,
                }
            })
        } else {
            let base = workspace_root.join(glob);
            base.is_dir().then(|| Self {
                base,
                relpath_root: workspace_root.to_path_buf(),
                label: label.unwrap_or("workspace").to_string(),
                in_repo: true,
            })
        }
    }
}

fn build_exclude_set(patterns: &[String]) -> Result<GlobSet, MarkdownDiscoverError> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat).map_err(|source| MarkdownDiscoverError::BadExclude {
            pattern: pat.clone(),
            source,
        })?;
        builder.add(glob);
    }
    // An empty builder yields a set that matches nothing — the desired
    // "no excludes" behavior.
    builder
        .build()
        .map_err(|source| MarkdownDiscoverError::BadExclude {
            pattern: "<exclude-set>".to_string(),
            source,
        })
}

/// `/`-normalized path of `path` relative to `plan.relpath_root`. `None` when
/// `path` is not under the relpath root (shouldn't happen during descent).
fn relpath_of(path: &Path, plan: &RootPlan) -> Option<String> {
    let rel = path.strip_prefix(&plan.relpath_root).ok()?;
    Some(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )
}

fn is_markdown(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|ext| {
            let ext = ext.to_ascii_lowercase();
            Language::Markdown.extensions().contains(&ext.as_str())
        })
}

/// Recursively walk `dir`, pruning excluded directories and emitting markdown
/// files into `out`. Symlinks are not followed (avoids cycles).
fn walk_dir(
    dir: &Path,
    plan: &RootPlan,
    excludes: &GlobSet,
    includes: &GlobSet,
    has_includes: bool,
    out: &mut Vec<DiscoveredMarkdown>,
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
        let Some(rel) = relpath_of(&path, plan) else {
            continue;
        };
        let is_excluded = excludes.is_match(&rel);
        if file_type.is_dir() {
            // Prune an excluded dir only when no `includes` glob could re-admit a
            // file beneath it; otherwise descend and filter per file below.
            if is_excluded && !has_includes {
                continue;
            }
            walk_dir(&path, plan, excludes, includes, has_includes, out);
        } else if file_type.is_file() && is_markdown(&path) {
            // `includes` wins over `excludes` — opt back in (e.g. generated docs).
            if is_excluded && !includes.is_match(&rel) {
                continue;
            }
            out.push(DiscoveredMarkdown {
                abs_path: path,
                label: plan.label.clone(),
                relpath: rel,
                in_repo: plan.in_repo,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_config::MarkdownRoot;
    use std::fs;
    use tempfile::TempDir;

    fn write(root: &Path, rel: &str, body: &str) {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    }

    fn cfg(roots: Vec<MarkdownRoot>, excludes: &[&str]) -> MarkdownConfig {
        MarkdownConfig {
            enabled: true,
            roots,
            excludes: excludes.iter().map(|s| (*s).to_string()).collect(),
            includes: Vec::new(),
        }
    }

    #[test]
    fn directory_root_discovers_md_recursively() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "docs/a.md", "# A");
        write(ws.path(), "docs/sub/b.md", "# B");
        write(ws.path(), "docs/sub/deep/c.markdown", "# C");
        write(ws.path(), "docs/notes.txt", "not markdown");

        let config = cfg(
            vec![MarkdownRoot {
                glob: "docs".into(),
                label: None,
            }],
            &[],
        );
        let found = discover_markdown(&config, ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(
            rels,
            ["docs/a.md", "docs/sub/b.md", "docs/sub/deep/c.markdown"]
        );
        assert!(found.iter().all(|f| f.label == "workspace"));
        assert!(found.iter().all(|f| f.in_repo)); // relative root → in-repo
    }

    #[test]
    fn excluded_paths_are_pruned() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "docs/keep.md", "# keep");
        write(ws.path(), "docs/node_modules/dep/readme.md", "# vendored");
        write(ws.path(), "docs/drafts/wip.md", "# wip");

        let config = cfg(
            vec![MarkdownRoot {
                glob: ".".into(),
                label: None,
            }],
            &["**/node_modules/**", "docs/drafts/**"],
        );
        let found = discover_markdown(&config, ws.path()).unwrap();
        let rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        assert_eq!(rels, ["docs/keep.md"]);
    }

    #[test]
    fn includes_re_admit_files_under_an_excluded_dir() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "keep.md", "# keep");
        write(
            ws.path(),
            "target/doc/api.md",
            "# generated docs we DO want",
        );
        write(
            ws.path(),
            "target/doc/internal.md",
            "# generated docs we do NOT want",
        );

        let mut config = cfg(
            vec![MarkdownRoot {
                glob: ".".into(),
                label: None,
            }],
            &["**/target/**"], // excludes the whole build dir…
        );
        config.includes = vec!["target/doc/api.md".to_string()]; // …but opt one back in

        let found = discover_markdown(&config, ws.path()).unwrap();
        let mut rels: Vec<&str> = found.iter().map(|f| f.relpath.as_str()).collect();
        rels.sort_unstable();
        // The included file is admitted (walker descended into the excluded dir);
        // its sibling under the same excluded dir stays out.
        assert_eq!(rels, ["keep.md", "target/doc/api.md"]);
    }

    #[test]
    fn external_root_uses_label_and_base_relative_paths() {
        let ws = TempDir::new().unwrap();
        let vault = TempDir::new().unwrap();
        write(vault.path(), "daily/today.md", "# today");

        let config = cfg(
            vec![MarkdownRoot {
                glob: vault.path().to_string_lossy().into_owned(),
                label: Some("notes".into()),
            }],
            &[],
        );
        let found = discover_markdown(&config, ws.path()).unwrap();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].label, "notes");
        assert_eq!(found[0].relpath, "daily/today.md");
        assert!(!found[0].in_repo); // absolute root → external vault
    }
}
