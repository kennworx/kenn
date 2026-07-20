//! CSS-internal dependency scan (Phase 3, step 1): `@use`/`@import`/`@forward`
//! between stylesheets.
//!
//! These at-rules are resolved away by dart-sass compilation, so they are
//! recovered by a **light source scan** (keyword + quoted-specifier spotting,
//! not Sass parsing) and turned into `imports` edges between the stylesheet
//! `module` nodes. Pure over its inputs (no I/O / no store) so it is unit
//! testable; the ingest pass supplies the module-id map.

use std::collections::HashMap;

use kenn_model::ShortId;

/// Extract the import specifiers of `@use`/`@import`/`@forward` at-rules from a
/// stylesheet source: the first quoted string after each keyword (`@use
/// 'tokens';` → `tokens`; `@import "a.css";` → `a.css`; `url("x")` unwrapped).
#[must_use]
pub(crate) fn extract_imports(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let t = line.trim_start();
        let is_import =
            t.starts_with("@use") || t.starts_with("@import") || t.starts_with("@forward");
        if !is_import {
            continue;
        }
        if let Some(spec) = first_quoted(t) {
            out.push(spec);
        }
    }
    out
}

/// The first single- or double-quoted substring in `s`, or `None`.
pub(crate) fn first_quoted(s: &str) -> Option<String> {
    let open = s.bytes().position(|b| b == b'"' || b == b'\'')?;
    let q = char::from(*s.as_bytes().get(open)?);
    let rest = s.get(open + 1..)?;
    let close = rest.find(q)?;
    Some(rest.get(..close)?.to_string())
}

/// Resolve an import `spec` (relative to the importing file `importer_relpath`)
/// to the relpath of an existing stylesheet `module`, applying Sass resolution
/// (bare name → partial `_name.scss`, index files, …). Returns the `(relpath,
/// module id)` of the first candidate present in `modules`, or `None` (→ no
/// edge — missing targets never dangle).
#[must_use]
pub(crate) fn resolve_import(
    importer_relpath: &str,
    spec: &str,
    modules: &HashMap<String, ShortId>,
) -> Option<(String, ShortId)> {
    let dir = importer_relpath.rsplit_once('/').map_or("", |(d, _)| d);
    let base = normalize_join(dir, spec.trim_start_matches("./"));
    let (bdir, stem) = base
        .rsplit_once('/')
        .map_or(("", base.as_str()), |(d, s)| (d, s));
    let with_dir = |name: String| {
        if bdir.is_empty() {
            name
        } else {
            format!("{bdir}/{name}")
        }
    };
    let candidates = [
        base.clone(), // spec carried its own extension
        format!("{base}.scss"),
        format!("{base}.sass"),
        format!("{base}.css"),
        with_dir(format!("_{stem}.scss")), // Sass partial
        with_dir(format!("_{stem}.sass")),
        format!("{base}/_index.scss"),
        format!("{base}/index.scss"),
    ];
    candidates
        .into_iter()
        .find_map(|c| modules.get(&c).map(|&id| (c, id)))
}

/// Join a `/`-relative `dir` with a `spec` that may contain `../` segments,
/// returning a normalized `/`-path (no leading `./`, `..` collapsed).
pub(crate) fn normalize_join(dir: &str, spec: &str) -> String {
    let mut parts: Vec<&str> = if dir.is_empty() {
        Vec::new()
    } else {
        dir.split('/').collect()
    };
    for seg in spec.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            s => parts.push(s),
        }
    }
    parts.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_use_import_forward_specifiers() {
        let src =
            "@use 'tokens';\n@forward \"base/colors\";\n.x { color: red }\n@import 'theme.css';\n";
        assert_eq!(extract_imports(src), ["tokens", "base/colors", "theme.css"]);
    }

    #[test]
    fn non_import_lines_ignored() {
        assert!(extract_imports(".btn { /* @use not really */ }\n").is_empty());
    }

    fn modules(paths: &[(&str, ShortId)]) -> HashMap<String, ShortId> {
        paths
            .iter()
            .map(|(p, id)| ((*p).to_string(), *id))
            .collect()
    }

    #[test]
    fn resolves_sass_partial_in_same_dir() {
        let m = modules(&[("sass/_tokens.scss", 5)]);
        // `@use 'tokens'` from sass/main.scss → sass/_tokens.scss.
        assert_eq!(
            resolve_import("sass/main.scss", "tokens", &m),
            Some(("sass/_tokens.scss".to_string(), 5))
        );
    }

    #[test]
    fn resolves_relative_and_explicit_extension() {
        let m = modules(&[("base/colors.scss", 7), ("theme.css", 9)]);
        assert_eq!(
            resolve_import("ui/main.scss", "../base/colors", &m),
            Some(("base/colors.scss".to_string(), 7))
        );
        assert_eq!(
            resolve_import("main.scss", "theme.css", &m),
            Some(("theme.css".to_string(), 9))
        );
    }

    #[test]
    fn missing_target_resolves_to_none() {
        let m = modules(&[("a.scss", 1)]);
        assert_eq!(resolve_import("main.scss", "ghost", &m), None);
    }
}
