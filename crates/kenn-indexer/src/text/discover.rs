//! Text-fallback discovery (task 2.2).
//!
//! A single recursive walk from the workspace root, driven by the configured
//! `include` file globs (there is no default — an empty include list discovers
//! nothing). Three filters apply, in order: excluded directories are pruned
//! during descent; a file whose extension is claimed by an enabled semantic /
//! native producer is skipped (no double-indexing, design D2); and the file
//! must match an include glob. The root label is always `workspace` — the
//! fallback is in-repo only.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_config::TextConfig;

/// The corpus root label for every text-fallback node id (`text:workspace/…`).
pub const ROOT_LABEL: &str = "workspace";

/// One discovered text-fallback file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiscoveredText {
    /// Absolute path on disk.
    pub abs_path: PathBuf,
    /// Workspace-relative, `/`-normalized path. The `<relpath>` of the node id.
    pub relpath: String,
}

#[derive(Debug, thiserror::Error)]
pub enum TextDiscoverError {
    #[error("invalid text {kind} glob `{pattern}`: {source}")]
    BadGlob {
        kind: &'static str,
        pattern: String,
        source: globset::Error,
    },
}

/// Discover every file under `workspace_root` that matches an `include` glob,
/// is not excluded, and whose extension no enabled producer claims. The caller
/// gates on `config.enabled`; this walks unconditionally.
pub fn discover_text(
    config: &TextConfig,
    workspace_root: &Path,
    claimed_exts: &BTreeSet<String>,
) -> Result<Vec<DiscoveredText>, TextDiscoverError> {
    if config.include.is_empty() {
        return Ok(Vec::new());
    }
    let includes = build_set(&config.include, "include")?;
    let excludes = build_set(&config.excludes, "exclude")?;
    let mut out = Vec::new();
    walk_dir(
        workspace_root,
        workspace_root,
        &includes,
        &excludes,
        claimed_exts,
        &mut out,
    );
    out.sort_by(|a, b| a.abs_path.cmp(&b.abs_path));
    out.dedup();
    Ok(out)
}

fn build_set(patterns: &[String], kind: &'static str) -> Result<GlobSet, TextDiscoverError> {
    let mut builder = GlobSetBuilder::new();
    for pat in patterns {
        let glob = Glob::new(pat).map_err(|source| TextDiscoverError::BadGlob {
            kind,
            pattern: pat.clone(),
            source,
        })?;
        builder.add(glob);
    }
    // An empty builder yields a set that matches nothing — the desired
    // "no excludes" behavior.
    builder
        .build()
        .map_err(|source| TextDiscoverError::BadGlob {
            kind,
            pattern: "<glob-set>".to_string(),
            source,
        })
}

/// `/`-normalized path of `path` relative to `root`. `None` when `path` is not
/// under `root` (shouldn't happen during descent).
fn relpath_of(path: &Path, root: &Path) -> Option<String> {
    let rel = path.strip_prefix(root).ok()?;
    Some(
        rel.to_string_lossy()
            .replace(std::path::MAIN_SEPARATOR, "/"),
    )
}

/// Lowercase extension (no dot) of `path`, or `None` when it has none.
fn extension_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
}

/// Recursively walk `dir`, pruning excluded directories and emitting matching
/// files into `out`. Symlinks are not followed (avoids cycles).
fn walk_dir(
    dir: &Path,
    root: &Path,
    includes: &GlobSet,
    excludes: &GlobSet,
    claimed_exts: &BTreeSet<String>,
    out: &mut Vec<DiscoveredText>,
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
        let Some(rel) = relpath_of(&path, root) else {
            continue;
        };
        if excludes.is_match(&rel) {
            continue;
        }
        if file_type.is_dir() {
            walk_dir(&path, root, includes, excludes, claimed_exts, out);
        } else if file_type.is_file() {
            // Skip files an enabled producer already claims by extension, then
            // require an include-glob match.
            if extension_of(&path).is_some_and(|ext| claimed_exts.contains(&ext)) {
                continue;
            }
            if includes.is_match(&rel) {
                out.push(DiscoveredText {
                    abs_path: path,
                    relpath: rel,
                });
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

    fn cfg(include: &[&str], excludes: &[&str]) -> TextConfig {
        TextConfig {
            enabled: true,
            include: include.iter().map(|s| (*s).to_string()).collect(),
            excludes: excludes.iter().map(|s| (*s).to_string()).collect(),
            ..Default::default()
        }
    }

    fn rels(found: &[DiscoveredText]) -> Vec<&str> {
        found.iter().map(|f| f.relpath.as_str()).collect()
    }

    #[test]
    fn empty_include_discovers_nothing() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "config/app.yaml", "a: 1");
        let config = cfg(&[], &[]);
        let found = discover_text(&config, ws.path(), &BTreeSet::new()).unwrap();
        assert!(found.is_empty());
    }

    #[test]
    fn include_globs_match_recursively() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "config/app.yaml", "a: 1");
        write(ws.path(), "config/sub/db.yaml", "b: 2");
        write(ws.path(), "data.json", "{}");
        write(ws.path(), "readme.txt", "hi");
        let config = cfg(&["**/*.yaml", "**/*.json"], &[]);
        let found = discover_text(&config, ws.path(), &BTreeSet::new()).unwrap();
        assert_eq!(
            rels(&found),
            ["config/app.yaml", "config/sub/db.yaml", "data.json"]
        );
    }

    #[test]
    fn excluded_paths_are_pruned() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "keep.yaml", "a: 1");
        write(ws.path(), "node_modules/dep/x.yaml", "b: 2");
        let config = cfg(&["**/*.yaml"], &["**/node_modules/**"]);
        let found = discover_text(&config, ws.path(), &BTreeSet::new()).unwrap();
        assert_eq!(rels(&found), ["keep.yaml"]);
    }

    #[test]
    fn claimed_extension_is_skipped_even_when_include_matches() {
        let ws = TempDir::new().unwrap();
        write(ws.path(), "lib.rs", "fn main() {}");
        write(ws.path(), "notes.txt", "hi");
        // A broad include glob would match `.rs`, but rust claims `rs`.
        let config = cfg(&["**/*"], &[]);
        let claimed: BTreeSet<String> = ["rs".to_string()].into_iter().collect();
        let found = discover_text(&config, ws.path(), &claimed).unwrap();
        assert_eq!(rels(&found), ["notes.txt"]);
    }
}
