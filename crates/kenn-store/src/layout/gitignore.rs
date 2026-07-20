//! Write the committed `.gitignore` that excludes the derived subtree
//! while keeping committed data (`vectors/`, `findings/`) tracked.
//! Called once at [`crate::layout::Store::open`].

use std::fs;

use super::types::{Layout, StoreError};

/// Write the committed `.gitignore` so the derived subtree is excluded
/// while the committed data — `vectors/` and `findings/` — stays
/// tracked. With the default layout this ignores `local/`; with a
/// relocated `derived_root` nothing derived lands under `.kenn/`, so the
/// file only documents that. Idempotent.
pub(crate) fn write_gitignore(layout: &Layout) -> Result<(), StoreError> {
    // The gitignore text lives in the sibling `gitignore.template` file — a
    // valid, static gitignore (data, not code). The only thing that varies is
    // the derived-store exclude line, appended here: the relative derived-root
    // segment (`local/` by default) when the derived store is under `.kenn/`, or
    // nothing when `[layout] derived_root` relocates it outside.
    let template = include_str!("gitignore.template");
    // The template is passed as a `{}` argument so its literal `{...}` braces
    // are not treated as format specifiers.
    let body = match layout.derived_root().strip_prefix(layout.committed_root()) {
        Ok(rel) => format!("{template}{}/\n", rel.display()),
        Err(_) => template.to_owned(),
    };
    let path = layout.gitignore_path();
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    if fs::read_to_string(&path).ok().as_deref() != Some(&body) {
        fs::write(&path, body)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::layout::Store;
    use std::fs;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        TempDir::new().unwrap()
    }

    #[test]
    fn open_writes_gitignore_ignoring_derived_dirs() {
        let ws = workspace();
        let store = Store::open_default(ws.path()).unwrap();
        let gitignore = fs::read_to_string(store.layout().gitignore_path())
            .expect(".gitignore must be written");
        let patterns: Vec<&str> = gitignore
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .collect();
        assert!(patterns.iter().any(|p| p.trim_end_matches('/') == "local"));
        assert!(
            !patterns
                .iter()
                .any(|p| p.trim_end_matches('/') == "vectors"),
            "vectors/ must be tracked"
        );
    }
}
