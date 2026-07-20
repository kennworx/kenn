//! Tailwind-style usage candidate extraction (Phase 2, step 1).
//!
//! Scans a source file as raw text and pulls class-shaped tokens out of string
//! literals — language-agnostic, no per-language parser. Each candidate carries
//! its byte offset (for later enclosing-symbol resolution) and whether it sat in
//! a recognized class-attribute / class-helper context (which grades the edge
//! `Exact` vs `Fuzzy` once it intersects the registry). Bare identifiers outside
//! string literals are ignored — class names live in strings, and scanning every
//! identifier would be pure noise.

/// Where a candidate token was found, which drives its edge grade later.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum UsageContext {
    /// Inside a `class=`/`className=`/`clsx(...)`-style class context → `Exact`.
    ClassAttr,
    /// Inside some other string literal → `Fuzzy`.
    OtherString,
}

/// One class-shaped token mined from source text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Candidate {
    pub name: String,
    /// Byte offset of the token's first char in the file.
    pub offset: usize,
    pub context: UsageContext,
}

use std::collections::HashSet;

use kenn_model::{Language, LinkGrade, ShortId};

/// Lookup over the class registry — the `css_class` node ids for a class name
/// (empty = not a defined class). Implemented against the building store
/// post-barrier; mocked in tests.
pub(crate) trait ClassRegistry {
    fn class_ids(&self, name: &str) -> Vec<ShortId>;
}

/// A resolved usage: a registry-hit class node, the byte offset of the token
/// (for enclosing-symbol resolution), and the confidence grade.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct UsageHit {
    pub class_id: ShortId,
    pub offset: usize,
    pub grade: LinkGrade,
}

/// Result of scanning one file: confirmed hits (→ `uses_css_class` edges) and
/// undefined class-shaped tokens (→ the `check_css` report; never graph nodes).
#[derive(Debug, Default)]
pub(crate) struct UsageScan {
    pub hits: Vec<UsageHit>,
    pub undefined: Vec<(String, usize)>,
}

/// Resolve a file's candidates against the registry. A hit becomes a graded
/// `UsageHit` (Exact in class context, Fuzzy elsewhere, Ambiguous when the name
/// has several definitions); a miss that is not a known utility is recorded as
/// undefined — never as an edge or node.
pub(crate) fn resolve_usages(
    text: &str,
    registry: &dyn ClassRegistry,
    is_utility: &dyn Fn(&str) -> bool,
) -> UsageScan {
    let mut scan = UsageScan::default();
    for c in extract_candidates(text) {
        let ids = registry.class_ids(&c.name);
        if ids.is_empty() {
            if !is_utility(&c.name) {
                scan.undefined.push((c.name, c.offset));
            }
            continue;
        }
        let grade = if ids.len() > 1 {
            LinkGrade::Ambiguous
        } else if c.context == UsageContext::ClassAttr {
            LinkGrade::Exact
        } else {
            LinkGrade::Fuzzy
        };
        for class_id in ids {
            scan.hits.push(UsageHit {
                class_id,
                offset: c.offset,
                grade,
            });
        }
    }
    scan
}

/// Markers whose presence shortly before a string literal marks it as a class
/// context (graded `Exact`). Covers HTML/JSX attributes and common helpers.
const CLASS_MARKERS: &[&str] = &[
    "class=",
    "className=",
    "class:",
    "classList",
    "clsx",
    "classnames",
    "classNames",
    "cn(",
    "cx(",
    "tw`",
];

/// Extract class-shaped candidate tokens from `text`. Tokens come only from
/// string-literal spans (`"`, `'`, or `` ` ``); each span is `ClassAttr` when a
/// class marker appears in the ~64 bytes before it, else `OtherString`.
pub(crate) fn extract_candidates(text: &str) -> Vec<Candidate> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        if b == b'"' || b == b'\'' || b == b'`' {
            let open = i;
            let mut j = i + 1;
            while bytes.get(j).is_some_and(|&x| x != b) {
                j += 1;
            }
            // [open+1, j) is the string content (j is the closing quote or EOF).
            let context = if has_class_marker(text, open) {
                UsageContext::ClassAttr
            } else {
                UsageContext::OtherString
            };
            tokenize_span(text, open + 1, j.min(bytes.len()), context, &mut out);
            i = j + 1; // skip past the closing quote
        } else {
            i += 1;
        }
    }
    out
}

/// Whether a class marker appears in the window of source just before the quote
/// at `open` (so `className="…"` / `clsx('…')` grade `Exact`).
fn has_class_marker(text: &str, open: usize) -> bool {
    let start = open.saturating_sub(64);
    // Snap to a char boundary so the slice is valid UTF-8.
    let start = (start..=open)
        .find(|&k| text.is_char_boundary(k))
        .unwrap_or(open);
    let window = text.get(start..open).unwrap_or("");
    CLASS_MARKERS.iter().any(|m| window.contains(m))
}

/// Split `[start, end)` of `text` into class-shaped tokens (`[A-Za-z0-9_-]+`
/// containing at least one ASCII letter), pushing each with its byte offset.
fn tokenize_span(
    text: &str,
    start: usize,
    end: usize,
    context: UsageContext,
    out: &mut Vec<Candidate>,
) {
    let bytes = text.as_bytes();
    let class_char = |k: usize| k < end && bytes.get(k).copied().is_some_and(is_class_char);
    let mut k = start;
    while k < end {
        if class_char(k) {
            let tok_start = k;
            while class_char(k) {
                k += 1;
            }
            if let Some(tok) = text.get(tok_start..k) {
                if tok.bytes().any(|c| c.is_ascii_alphabetic()) {
                    out.push(Candidate {
                        name: tok.to_string(),
                        offset: tok_start,
                        context,
                    });
                }
            }
        } else {
            k += 1;
        }
    }
}

const fn is_class_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'-'
}

/// Extract stylesheet import specifiers from a code source: the quoted specifiers
/// on `import`/`require`/`from` lines whose extension's MIME type is a stylesheet
/// (`.css`/`.scss`/`.sass`). Returns each `(specifier, target Language)`. A light
/// keyword + quote scan, not a JS parser — covers `import './x.css'`,
/// `import s from './x.module.css'`, `export … from './x.css'`,
/// `require('./x.css')`, and dynamic `import('./x.css')`.
pub(crate) fn extract_style_imports(source: &str) -> Vec<(String, Language)> {
    let mut out = Vec::new();
    for line in source.lines() {
        if !(line.contains("import") || line.contains("require") || line.contains("from")) {
            continue;
        }
        for spec in quoted_on_line(line) {
            if let Some(lang) = is_stylesheet_import(&spec) {
                out.push((spec, lang));
            }
        }
    }
    out
}

/// Classify an import specifier as a stylesheet by its extension's MIME type:
/// `text/css` → `Css`, `text/x-scss`/`text/x-sass` → `Sass`, else `None`. (The
/// MIME detector also keeps `.ts` — `video/mp2t` — and `.json` out.)
pub(crate) fn is_stylesheet_import(spec: &str) -> Option<Language> {
    match mime_guess::from_path(spec).first()?.essence_str() {
        "text/css" => Some(Language::Css),
        "text/x-scss" | "text/x-sass" => Some(Language::Sass),
        _ => None,
    }
}

/// CSS-Modules default/namespace import bindings in a JS/TS source: the local
/// name a stylesheet is bound to, paired with its specifier. Covers
/// `import s from './x.module.css'`, `import * as s from './x.css'`, and
/// `const s = require('./x.css')`. Named (`import { a } from`) and side-effect
/// (`import './x'`) imports bind no local and are excluded (the latter is the
/// code→stylesheet import path instead).
pub(crate) fn extract_module_bindings(source: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    for line in source.lines() {
        let Some(spec) = quoted_on_line(line).into_iter().next() else {
            continue;
        };
        let trimmed = line.trim_start();
        let local = trimmed
            .strip_prefix("import ")
            .and_then(import_local)
            .or_else(|| require_local(trimmed));
        if let Some(local) = local {
            out.push((local, spec));
        }
    }
    out
}

/// The bound local of an `import …` clause (the text after `import `), or `None`
/// for a named-only / side-effect import.
fn import_local(rest: &str) -> Option<String> {
    let rest = rest.trim_start();
    if let Some(after) = rest.strip_prefix("* as ") {
        return Some(leading_ident(after)).filter(|s| !s.is_empty());
    }
    if rest.starts_with(['{', '"', '\'']) {
        return None; // named-only or side-effect import binds no local
    }
    Some(leading_ident(rest)).filter(|s| !s.is_empty())
}

/// The bound local of a `const/let/var <id> = require('…')`, or `None`.
fn require_local(line: &str) -> Option<String> {
    if !(line.contains("require") && line.contains('=')) {
        return None;
    }
    let line = line.trim_start();
    for kw in ["const ", "let ", "var "] {
        if let Some(rest) = line.strip_prefix(kw) {
            return Some(leading_ident(rest.trim_start())).filter(|s| !s.is_empty());
        }
    }
    None
}

/// The leading JS identifier (`[A-Za-z_$][A-Za-z0-9_$]*`) of `s`.
fn leading_ident(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut j = 0;
    while bytes.get(j).copied().is_some_and(is_ident_char) {
        j += 1;
    }
    s.get(..j).unwrap_or("").to_string()
}

/// Member accesses on the given `locals`: `s.foo` and `s['foo']`/`s["foo"]`.
/// Returns `(local, member, byte offset of the access)`. The `.member` form
/// yields JS-identifier members; the `['…']` form yields any class name
/// (including kebab-case).
pub(crate) fn extract_member_accesses(
    source: &str,
    locals: &HashSet<String>,
) -> Vec<(String, String, usize)> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(&b) = bytes.get(i) {
        let prev_is_ident = i > 0 && bytes.get(i - 1).copied().is_some_and(is_ident_char);
        if is_ident_start(b) && !prev_is_ident {
            let start = i;
            let mut j = i;
            while bytes.get(j).copied().is_some_and(is_ident_char) {
                j += 1;
            }
            if let Some(ident) = source.get(start..j) {
                if locals.contains(ident) {
                    if let Some((member, off)) = member_after(source, j) {
                        out.push((ident.to_string(), member, off));
                    }
                }
            }
            i = j;
        } else {
            i += 1;
        }
    }
    out
}

/// Parse the member just past a local binding at byte `pos`: `.member` or
/// `['member']` / `["member"]`. Returns `(member, byte offset of the member)`.
fn member_after(source: &str, pos: usize) -> Option<(String, usize)> {
    let bytes = source.as_bytes();
    match bytes.get(pos)? {
        b'.' => {
            let start = pos + 1;
            let mut j = start;
            while bytes.get(j).copied().is_some_and(is_ident_char) {
                j += 1;
            }
            if j > start {
                source.get(start..j).map(|m| (m.to_string(), start))
            } else {
                None
            }
        }
        b'[' => {
            let q = *bytes.get(pos + 1)?;
            if q != b'"' && q != b'\'' {
                return None;
            }
            let start = pos + 2;
            let mut j = start;
            while bytes.get(j).is_some_and(|&b| b != q) {
                j += 1;
            }
            source.get(start..j).map(|m| (m.to_string(), start))
        }
        _ => None,
    }
}

/// Class-name candidates for a CSS-module member, folding camelCase↔kebab-case
/// (CSS-Modules loaders camelize, but `['btn-primary']` accesses the raw name):
/// `btnPrimary` → `[btnPrimary, btn-primary]`; `btn-primary` → `[btn-primary, btnPrimary]`.
pub(crate) fn class_name_candidates(member: &str) -> Vec<String> {
    let mut out = vec![member.to_string()];
    for folded in [to_kebab(member), to_camel(member)] {
        if !out.contains(&folded) {
            out.push(folded);
        }
    }
    out
}

/// `btnPrimary` → `btn-primary` (lowercase, `-` before each interior capital).
fn to_kebab(s: &str) -> String {
    let mut out = String::new();
    for c in s.chars() {
        if c.is_ascii_uppercase() {
            if !out.is_empty() {
                out.push('-');
            }
            out.push(c.to_ascii_lowercase());
        } else {
            out.push(c);
        }
    }
    out
}

/// `btn-primary` → `btnPrimary` (drop `-`, uppercase the next char).
fn to_camel(s: &str) -> String {
    let mut out = String::new();
    let mut upper = false;
    for c in s.chars() {
        if c == '-' {
            upper = true;
        } else if upper {
            out.extend(c.to_uppercase());
            upper = false;
        } else {
            out.push(c);
        }
    }
    out
}

const fn is_ident_start(b: u8) -> bool {
    b.is_ascii_alphabetic() || b == b'_' || b == b'$'
}

const fn is_ident_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_' || b == b'$'
}

/// Every single-, double-, or backtick-quoted substring on one line.
fn quoted_on_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut quote: Option<(char, usize)> = None; // (quote char, content byte start)
    for (idx, ch) in line.char_indices() {
        if let Some((q, start)) = quote {
            if ch == q {
                if let Some(s) = line.get(start..idx) {
                    out.push(s.to_string());
                }
                quote = None;
            }
        } else if ch == '"' || ch == '\'' || ch == '`' {
            quote = Some((ch, idx + ch.len_utf8()));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names_ctx(text: &str) -> Vec<(String, UsageContext)> {
        extract_candidates(text)
            .into_iter()
            .map(|c| (c.name, c.context))
            .collect()
    }

    #[test]
    fn classname_attribute_tokens_are_class_attr() {
        let got = names_ctx(r#"<button className="btn btn-primary">x</button>"#);
        assert_eq!(
            got,
            [
                ("btn".to_string(), UsageContext::ClassAttr),
                ("btn-primary".to_string(), UsageContext::ClassAttr),
            ]
        );
    }

    #[test]
    fn plain_class_attribute_html() {
        let got = names_ctx(r#"<div class="card">"#);
        assert_eq!(got, [("card".to_string(), UsageContext::ClassAttr)]);
    }

    #[test]
    fn clsx_helper_args_are_class_attr() {
        let got = names_ctx(r"clsx('btn', isActive && 'is-active')");
        assert_eq!(
            got,
            [
                ("btn".to_string(), UsageContext::ClassAttr),
                ("is-active".to_string(), UsageContext::ClassAttr),
            ]
        );
    }

    #[test]
    fn unrelated_string_is_other_context() {
        let got = names_ctx(r#"const msg = "hello world";"#);
        assert_eq!(
            got,
            [
                ("hello".to_string(), UsageContext::OtherString),
                ("world".to_string(), UsageContext::OtherString),
            ]
        );
    }

    #[test]
    fn bare_identifiers_outside_strings_are_ignored() {
        // `card` here is a variable, not a string → not a candidate.
        assert!(extract_candidates("let card = makeCard();").is_empty());
    }

    #[test]
    fn offsets_point_at_the_token() {
        let text = r#"x="btn""#;
        let c = &extract_candidates(text)[0];
        assert_eq!(text.get(c.offset..c.offset + c.name.len()), Some("btn"));
    }

    #[test]
    fn template_literal_class_context() {
        let got = names_ctx("className={`btn ${v} card`}");
        // both `btn` and `card` are in the className backtick → ClassAttr.
        assert!(got.contains(&("btn".to_string(), UsageContext::ClassAttr)));
        assert!(got.contains(&("card".to_string(), UsageContext::ClassAttr)));
    }

    // ---- resolver (7.2 / 7.4) -------------------------------------------

    struct MockRegistry(std::collections::HashMap<&'static str, Vec<ShortId>>);
    impl ClassRegistry for MockRegistry {
        fn class_ids(&self, name: &str) -> Vec<ShortId> {
            self.0.get(name).cloned().unwrap_or_default()
        }
    }

    fn registry(pairs: &[(&'static str, &[ShortId])]) -> MockRegistry {
        MockRegistry(pairs.iter().map(|(n, ids)| (*n, ids.to_vec())).collect())
    }

    #[test]
    fn hit_in_class_attr_grades_exact_miss_is_undefined() {
        let reg = registry(&[("btn", &[10])]);
        let scan = resolve_usages(r#"<a className="btn flex">"#, &reg, &|_| false);
        assert_eq!(scan.hits.len(), 1);
        assert_eq!(scan.hits[0].class_id, 10);
        assert_eq!(scan.hits[0].grade, LinkGrade::Exact);
        // `flex` has no registry entry and isn't a known utility → undefined.
        assert_eq!(scan.undefined, [("flex".to_string(), 18)]);
    }

    #[test]
    fn bare_string_hit_grades_fuzzy() {
        let reg = registry(&[("card", &[7])]);
        let scan = resolve_usages(r#"const x = "card";"#, &reg, &|_| false);
        assert_eq!(scan.hits.len(), 1);
        assert_eq!(scan.hits[0].grade, LinkGrade::Fuzzy);
    }

    #[test]
    fn multiple_definitions_grade_ambiguous() {
        let reg = registry(&[("btn", &[1, 2])]);
        let scan = resolve_usages(r#"<a class="btn">"#, &reg, &|_| false);
        assert_eq!(scan.hits.len(), 2);
        assert!(scan.hits.iter().all(|h| h.grade == LinkGrade::Ambiguous));
    }

    #[test]
    fn known_utility_miss_is_not_undefined() {
        let reg = registry(&[]);
        let scan = resolve_usages(r#"<a class="flex">"#, &reg, &|n| n == "flex");
        assert!(scan.hits.is_empty());
        assert!(scan.undefined.is_empty()); // flex is a known utility → dropped
    }

    #[test]
    fn stylesheet_import_mime_classification() {
        assert_eq!(is_stylesheet_import("./a.css"), Some(Language::Css));
        assert_eq!(is_stylesheet_import("./a.module.css"), Some(Language::Css));
        assert_eq!(is_stylesheet_import("../x.scss"), Some(Language::Sass));
        assert_eq!(is_stylesheet_import("./y.sass"), Some(Language::Sass));
        assert_eq!(is_stylesheet_import("./data.json"), None);
        assert_eq!(is_stylesheet_import("./mod.ts"), None);
        assert_eq!(is_stylesheet_import("react"), None);
    }

    #[test]
    fn extracts_stylesheet_imports_across_forms() {
        let src = "import './a.css';\n\
                   import s from './b.module.css';\n\
                   export * from './c.scss';\n\
                   const d = require('./d.sass');\n\
                   const e = await import('./e.css');\n\
                   import { x } from './util.ts';\n\
                   const path = './theme.css';\n";
        let got = extract_style_imports(src);
        let specs: Vec<&str> = got.iter().map(|(s, _)| s.as_str()).collect();
        assert_eq!(
            specs,
            [
                "./a.css",
                "./b.module.css",
                "./c.scss",
                "./d.sass",
                "./e.css"
            ]
        );
        // `./util.ts` is not a stylesheet; `./theme.css` sits on a line with no
        // import/require/from keyword, so it isn't scanned (keyword gate).
        assert!(got.iter().all(|(s, _)| s != "./util.ts"));
        assert!(got.iter().all(|(s, _)| s != "./theme.css"));
    }

    #[test]
    fn module_bindings_default_namespace_and_require() {
        let src = "import s from './a.module.css';\n\
                   import * as t from './b.css';\n\
                   const u = require('./c.scss');\n\
                   import { x } from './d.css';\n\
                   import './e.css';\n";
        let got = extract_module_bindings(src);
        assert_eq!(
            got,
            vec![
                ("s".to_string(), "./a.module.css".to_string()),
                ("t".to_string(), "./b.css".to_string()),
                ("u".to_string(), "./c.scss".to_string()),
            ]
        );
        // named-only (`import { x }`) and side-effect (`import './e.css'`) bind no local.
    }

    #[test]
    fn member_accesses_dot_and_bracket() {
        let locals: HashSet<String> = ["s".to_string()].into_iter().collect();
        let src = "const a = s.btnPrimary;\nconst b = s['btn-primary'];\nconst c = other.foo;";
        let got: Vec<(String, String)> = extract_member_accesses(src, &locals)
            .into_iter()
            .map(|(l, m, _)| (l, m))
            .collect();
        assert_eq!(
            got,
            vec![
                ("s".to_string(), "btnPrimary".to_string()),
                ("s".to_string(), "btn-primary".to_string()),
            ]
        );
        // `other.foo` — `other` is not a bound local, so it's skipped.
    }

    #[test]
    fn class_candidates_fold_both_directions() {
        assert_eq!(
            class_name_candidates("btnPrimary"),
            ["btnPrimary", "btn-primary"]
        );
        assert_eq!(
            class_name_candidates("btn-primary"),
            ["btn-primary", "btnPrimary"]
        );
        assert_eq!(class_name_candidates("card"), ["card"]);
    }
}
