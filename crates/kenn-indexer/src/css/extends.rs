//! `@extend .class` / CSS-Modules `composes` rule-to-rule reference scan
//! (Phase 3, step 2).
//!
//! Both are resolved away before the browser sees them — dart-sass inlines
//! `@extend` into the extending selector; the CSS-Modules loader rewrites
//! `composes` — so they are recovered by the same kind of light brace-tracking
//! source scan as `@use`/`@import` (keyword spotting plus the enclosing rule's
//! selector), NOT by parsing Sass. An `extends_rule` edge is emitted only when
//! BOTH the enclosing selector and the target resolve to a known class node (a
//! single bare class name on each side); compound / `&`-nested / interpolated
//! selectors and `.sass` indented syntax simply yield no edge — consistent with
//! usage resolution's no-dangling-stubs rule.

use super::internal::first_quoted;

/// One `@extend`/`composes` reference recovered from a stylesheet source.
#[derive(Debug, PartialEq, Eq)]
pub(crate) struct ExtendRef {
    /// Bare class name of the enclosing rule (the extending class).
    pub(crate) enclosing: String,
    /// Bare class name being extended / composed.
    pub(crate) target: String,
    /// CSS-Modules `composes <name> from '<spec>'` import specifier, if present
    /// (the target lives in another stylesheet); `None` for same-file `composes`
    /// and for Sass `@extend` (resolved by name across the compilation).
    pub(crate) from: Option<String>,
}

/// Scan a stylesheet source for `@extend`/`composes` references, tracking the
/// enclosing rule via brace depth. Only declarations directly inside a rule
/// whose header is a sole bare class selector (`.btn-primary { … }`) are
/// recovered; anything else (compound, `&`, comma list, interpolation) yields
/// no ref. `.sass` indented syntax has no braces and so produces nothing.
#[must_use]
pub(crate) fn extract_extends(source: &str) -> Vec<ExtendRef> {
    let mut out = Vec::new();
    let mut stack: Vec<Option<String>> = Vec::new();
    let mut buf = String::new();
    for ch in source.chars() {
        match ch {
            '{' => {
                stack.push(sole_class(&buf));
                buf.clear();
            }
            '}' => {
                stack.pop();
                buf.clear();
            }
            ';' => {
                if let Some(Some(enclosing)) = stack.last() {
                    collect_refs(buf.trim(), enclosing, &mut out);
                }
                buf.clear();
            }
            _ => buf.push(ch),
        }
    }
    out
}

/// The sole class name of a selector header, or `None` when it is not exactly
/// one bare class selector (`.name`). Rejects compound (`.a.b`), descendant
/// (`.a .b`), comma lists, `&`-nesting, and interpolation.
fn sole_class(header: &str) -> Option<String> {
    let name = header.trim().strip_prefix('.')?;
    if is_class_name(name) {
        Some(name.to_string())
    } else {
        None
    }
}

/// A bare CSS class identifier: non-empty, only `[A-Za-z0-9_-]`.
fn is_class_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

/// Pull `@extend .x`/`composes: …` targets out of one `;`-terminated statement.
fn collect_refs(stmt: &str, enclosing: &str, out: &mut Vec<ExtendRef>) {
    if let Some(rest) = stmt.strip_prefix("@extend") {
        // `@extend .a, .b !optional` — comma list, trailing `!optional` flag.
        let rest = rest.split('!').next().unwrap_or(rest);
        for tok in rest.split(',') {
            if let Some(name) = tok.trim().strip_prefix('.') {
                if is_class_name(name) && name != enclosing {
                    out.push(ExtendRef {
                        enclosing: enclosing.to_string(),
                        target: name.to_string(),
                        from: None,
                    });
                }
            }
        }
    } else if let Some(rest) = stmt.strip_prefix("composes:") {
        // `composes: a b from './x'` or `composes: a b` (bare names, no dot).
        let (names, from) = match rest.split_once(" from ") {
            Some((names, src)) => (names, first_quoted(src)),
            None => (rest, None),
        };
        for name in names.split_whitespace() {
            if is_class_name(name) && !(from.is_none() && name == enclosing) {
                out.push(ExtendRef {
                    enclosing: enclosing.to_string(),
                    target: name.to_string(),
                    from: from.clone(),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_sass_extend_from_enclosing_class() {
        let src = ".btn-primary {\n  @extend .btn;\n  color: blue;\n}\n";
        assert_eq!(
            extract_extends(src),
            [ExtendRef {
                enclosing: "btn-primary".into(),
                target: "btn".into(),
                from: None,
            }]
        );
    }

    #[test]
    fn extends_comma_list_and_optional_flag() {
        let src = ".x { @extend .a, .b !optional; }";
        let refs = extract_extends(src);
        let targets: Vec<&str> = refs.iter().map(|r| r.target.as_str()).collect();
        assert_eq!(targets, ["a", "b"]);
    }

    #[test]
    fn composes_same_file_and_cross_file() {
        let src = ".card { composes: base; }\n.hero { composes: big from './u.css'; }\n";
        let refs = extract_extends(src);
        assert_eq!(refs[0].target, "base");
        assert_eq!(refs[0].from, None);
        assert_eq!(refs[1].target, "big");
        assert_eq!(refs[1].from.as_deref(), Some("./u.css"));
    }

    #[test]
    fn nested_or_compound_enclosing_yields_nothing() {
        // `&`-nesting, compound, and descendant headers are not sole classes.
        assert!(extract_extends(".card { &-x { @extend .h; } }").is_empty());
        assert!(extract_extends(".a.b { @extend .h; }").is_empty());
        assert!(extract_extends(".a .b { @extend .h; }").is_empty());
    }

    #[test]
    fn self_extend_is_dropped() {
        assert!(extract_extends(".btn { @extend .btn; }").is_empty());
    }
}
