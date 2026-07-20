//! Per-language `pub_id` rendering (shell-safe-symbol-ids, design D6).
//!
//! A `pub_id` is handed to the shell as a `kenn get <pub_id>` argument, so it must be
//! shell-safe — and **shell-safety lives here, in Rust ingestion, only** (design D6).
//! The indexers (external tools and our own sidecars alike) emit real, language-native
//! symbols and know nothing about shells; this module is the single place that renders
//! them safe. The transform is **per-language**, not uniform: the same byte means
//! different things in different languages, and only a language-specific rule renders
//! it right. Proof: `<` is an *operator name* in Swift but *opens a generic* in
//! Rust/C#, so a uniform `<`→`~` would corrupt the Swift `<` operator into a fake
//! generic. So each arm renders its language's structure here, then passes through the
//! readable [`floor`] so any leaf byte its rules did not consume becomes a safe `_`.
//! The shared [`kenn_model::shell_safe::is_safe`] defines what "safe" means; the
//! flooring itself is this crate's concern.

use kenn_model::language::Language;
use kenn_model::shell_safe::is_safe;

/// Render a raw descriptor-based `pub_id` into a shell-safe token, per language.
#[must_use]
pub(crate) fn render(language: Language, raw: &str) -> String {
    match language {
        Language::Rust => render_rust(raw),
        Language::TypeScript => render_ts(raw),
        Language::Csharp => render_csharp(raw),
        Language::Swift => render_swift(raw),
        // Go/Python: SCIP, no fn/generic grammar to substitute, but they still
        // carry a trailing kind marker to drop (D1) before the readable floor.
        Language::Go | Language::Python => floor(strip_kind_suffix(raw)),
        // Markdown/css/sass/html/text: no SCIP markers — the floor only.
        _ => floor(raw),
    }
}

/// Drop a SCIP descriptor's trailing kind marker — the term `.` (`Foo#config.`)
/// or a bare type's `#` (`IdRegistry#`). The symbol's kind is carried by the
/// store's `kind` column, not the id (design D1), so the trailing marker is noise.
/// Only the LAST segment's marker is a suffix; internal `#`/`.` (a type→member
/// separator, a dotted name) are left untouched — `trim_end_matches` only strips
/// from the end, and an identifier never ends in `.`/`#`.
fn strip_kind_suffix(raw: &str) -> &str {
    raw.trim_end_matches(['.', '#'])
}

/// The one shell-safety floor for every `pub_id`: the structureless languages
/// (go, python, markdown, css, sass, html, text) render straight through it, the
/// structural renders (rust/ts/c#/swift) pass their transformed output through it,
/// and the markdown/html ingesters call it directly.
///
/// Every maximal run of shell-hostile characters collapses to a single `_`
/// (`Some Note` → `Some_Note`, `a<>b` → `a_b`, `x%20y` → `x_20y`). It is lossy
/// (distinct hostile chars collapse, a small `pub_id` collision risk) but the ids
/// read like names rather than `_x<NN>_` byte escapes. Safe characters — including
/// an existing `_`, so `__init__` survives intact — pass through verbatim; only
/// hostile runs coalesce. Idempotent, and its output always satisfies
/// [`is_safe`], which is what the store's writer asserts.
#[must_use]
pub(crate) fn floor(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut in_hostile_run = false;
    for ch in raw.chars() {
        if is_safe(ch) {
            out.push(ch);
            in_hostile_run = false;
        } else if !in_hostile_run {
            out.push('_');
            in_hostile_run = true;
        }
    }
    out
}

/// Rust: drop the trailing kind marker (D1), SCIP backtick quoting; `<`→`~`,
/// `>`-drop (generics). Lifetimes carry no identity, so the whole lifetime is
/// dropped and the comma/space it left is cleaned up — `Cow<'_, B>` → `Cow~B`,
/// `Walker<'_>` → `Walker` (an all-lifetime generic collapses away entirely).
/// The structural pass may leave stray leaf bytes; [`floor`] makes it safe.
fn render_rust(raw: &str) -> String {
    let raw = strip_kind_suffix(raw);
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            // drop backtick quoting, the generic close, and signature spaces
            '`' | '>' | ' ' => {}
            '<' => out.push('~'),
            // a lifetime (`'_`, `'a`, `'static`) is dropped whole — the tick AND
            // its name — so `<'_, B>` heads toward `~B`, not `~_,_B`.
            '\'' => {
                while chars
                    .peek()
                    .is_some_and(|c| c.is_alphanumeric() || *c == '_')
                {
                    chars.next();
                }
            }
            // drop a comma a removed lifetime orphaned (no left operand: right
            // after the `~` open or another comma)
            ',' if out.is_empty() || out.ends_with('~') || out.ends_with(',') => {}
            c => out.push(c),
        }
    }
    while out.ends_with(',') {
        out.pop();
    }
    // Collapse an empty generic left when every arg was a lifetime (`Walker<'_>`
    // → `Walker~` → `Walker`): a `~` with no args — before a `::` or at the end.
    let mut cleaned = String::with_capacity(out.len());
    let mut it = out.chars().peekable();
    while let Some(c) = it.next() {
        if c == '~' && matches!(it.peek(), None | Some(':')) {
            continue;
        }
        cleaned.push(c);
    }
    floor(&cleaned)
}

/// TypeScript: drop the trailing kind marker (D1), SCIP backtick quoting;
/// `(`→`+`, `)`-drop (method marker).
fn render_ts(raw: &str) -> String {
    let raw = strip_kind_suffix(raw);
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            '`' | ')' => {}
            '(' => out.push('+'),
            c => out.push(c),
        }
    }
    floor(&out)
}

/// C# (string-level; the richer `_word_`/external-leaf version is task 3.1, still
/// rendered here in Rust — D6): `(`→`+`, `)`-drop, `<`→`~`, `>`-drop, drop signature
/// spaces; [`floor`] the rest (arrays `[]`, nullable `?` → a readable `_`). The pretty
/// forms would need kenn-dotnet to emit a richer *real* symbol for this arm to read.
fn render_csharp(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    for ch in raw.chars() {
        match ch {
            ')' | '>' | ' ' => {}
            '(' => out.push('+'),
            '<' => out.push('~'),
            c => out.push(c),
        }
    }
    floor(&out)
}

/// The name of a Swift operator glyph, for `op_*` wording. `None` for non-operator
/// characters.
fn swift_op_glyph(ch: char) -> Option<&'static str> {
    Some(match ch {
        '<' => "lt",
        '>' => "gt",
        '=' => "eq",
        '+' => "add",
        '-' => "sub",
        '*' => "mul",
        '/' => "div",
        '%' => "mod",
        '!' => "bang",
        '&' => "amp",
        '|' => "pipe",
        '^' => "caret",
        '~' => "tilde",
        '?' => "q",
        _ => return None,
    })
}

/// Swift: an operator *name* is a maximal run of operator glyphs (`<`, `==`, `<*>`) —
/// worded `op_<glyph>_<glyph>` so a `<` operator never becomes a generic `~`; then
/// `(`→`+`, `)`-drop for the argument-label list. Its existing `#<hash>` overload
/// disambiguator is preserved (`#` is safe).
fn render_swift(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(ch) = chars.next() {
        // Operator run: a non-dot glyph, OR a dot-operator (`...`, `..<`), which
        // starts with ≥2 dots. A lone `.` is member access (`Name.<`) and stays a
        // separator — so a single `.` before a glyph does NOT start an operator.
        let starts_operator = if ch == '.' {
            chars.peek() == Some(&'.')
        } else {
            swift_op_glyph(ch).is_some()
        };
        if starts_operator {
            out.push_str("op_");
            out.push_str(swift_op_token(ch));
            while let Some(&next) = chars.peek() {
                if next == '.' || swift_op_glyph(next).is_some() {
                    out.push('_');
                    out.push_str(swift_op_token(next));
                    chars.next();
                } else {
                    break;
                }
            }
        } else if ch == '(' {
            out.push('+');
        } else if ch == ')' {
            // drop the argument-label list close
        } else {
            out.push(ch);
        }
    }
    floor(&out)
}

/// Token for one swift operator glyph in the `op_<tok>_<tok>` wording: `.`→`dot`
/// (dot-operators), otherwise the [`swift_op_glyph`] name.
fn swift_op_token(ch: char) -> &'static str {
    if ch == '.' {
        "dot"
    } else {
        swift_op_glyph(ch).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn has_hostile(s: &str) -> bool {
        s.chars()
            .any(|c| !kenn_model::shell_safe::is_safe(c) && c != '@')
    }

    #[test]
    fn rust_generic_and_lifetime() {
        assert_eq!(
            render(Language::Rust, "rs:X::exit::ExitCode::`From<ExitCodes>`"),
            "rs:X::exit::ExitCode::From~ExitCodes"
        );
        // An all-lifetime generic collapses away — no `~_` residue.
        assert_eq!(
            render(Language::Rust, "rs:X::`Walker<'_>`::record"),
            "rs:X::Walker::record"
        );
    }

    #[test]
    fn rust_lifetime_dropped_from_generic_args() {
        // `<'_, B>` → `~B`: the lifetime + the comma/space it left are gone.
        assert_eq!(
            render(Language::Rust, "rs:alloc::borrow::Cow<'_, B>::into_owned"),
            "rs:alloc::borrow::Cow~B::into_owned"
        );
        // A leading real arg + trailing lifetime cleans up too: `<T, 'a>` → `~T`.
        assert_eq!(render(Language::Rust, "rs:m::Ref<T, 'a>"), "rs:m::Ref~T");
    }

    #[test]
    fn rust_multiple_lifetimes_all_cleaned() {
        // Leading run of lifetimes.
        assert_eq!(render(Language::Rust, "rs:m::F<'a, 'b, T>"), "rs:m::F~T");
        // A lifetime between two real args keeps both and one comma.
        assert_eq!(render(Language::Rust, "rs:m::G<T, 'a, U>"), "rs:m::G~T,U");
        // Lifetimes surrounding a real arg.
        assert_eq!(render(Language::Rust, "rs:m::H<'a, T, 'b>"), "rs:m::H~T");
        // All-lifetime generics collapse the whole `<…>` away.
        assert_eq!(render(Language::Rust, "rs:m::I<'a, 'b>"), "rs:m::I");
        assert_eq!(
            render(Language::Rust, "rs:m::J<'_, '_>::iter"),
            "rs:m::J::iter"
        );
    }

    #[test]
    fn ts_backtick_and_method() {
        // Method: `()`→`+`, and the trailing term `.` is dropped (D1).
        assert_eq!(
            render(Language::TypeScript, "ts:`indexers/frames.ts`/walk()."),
            "ts:indexers/frames.ts/walk+"
        );
    }

    #[test]
    fn ts_trailing_kind_markers_dropped_internal_kept() {
        // Bare type: the trailing `#` type marker is dropped.
        assert_eq!(
            render(Language::TypeScript, "ts:`indexers/frames.ts`/IdRegistry#"),
            "ts:indexers/frames.ts/IdRegistry"
        );
        // Member: the trailing term `.` is dropped, but the INTERNAL type→member
        // `#` separator stays.
        assert_eq!(
            render(
                Language::TypeScript,
                "ts:`indexers/frames.ts`/EdgeFrame#edge_kind."
            ),
            "ts:indexers/frames.ts/EdgeFrame#edge_kind"
        );
    }

    #[test]
    fn csharp_signature_arrays_nullable_safe() {
        assert_eq!(
            render(Language::Csharp, "cs:C#Run(A.B, C.D)"),
            "cs:C#Run+A.B,C.D"
        );
        for raw in [
            "cs:X#set_Doc(string[])",
            "cs:X#F((int Sl, int Sc)?)",
            "cs:Program#<Main>$(string[])",
        ] {
            assert!(
                !has_hostile(&render(Language::Csharp, raw)),
                "hostile: {raw}"
            );
        }
    }

    #[test]
    fn swift_operator_is_worded_not_a_generic() {
        // The load-bearing case: swift `<` is an OPERATOR, must not become `~`.
        assert_eq!(
            render(Language::Swift, "sw:ArgumentParser.Name.<(_:_:)"),
            "sw:ArgumentParser.Name.op_lt+_:_:"
        );
        assert_eq!(
            render(Language::Swift, "sw:ArgumentParser.Tree.==(_:_:)"),
            "sw:ArgumentParser.Tree.op_eq_eq+_:_:"
        );
        assert!(!render(Language::Swift, "sw:X.<(_:_:)").contains('~'));
    }

    #[test]
    fn swift_dot_operators_are_worded() {
        // Range operators are worded by the general glyph rule, not left as
        // literal dots (`...` / `..<` were `...` / `..op_lt` before).
        assert_eq!(
            render(Language::Swift, "sw:...(_:_:)"),
            "sw:op_dot_dot_dot+_:_:"
        );
        assert_eq!(
            render(Language::Swift, "sw:..<(_:_:)"),
            "sw:op_dot_dot_lt+_:_:"
        );
        // A lone member-access `.` before an operator stays a separator.
        assert_eq!(
            render(Language::Swift, "sw:A.Name.<(_:)"),
            "sw:A.Name.op_lt+_:"
        );
    }

    #[test]
    fn swift_op_glyph_maps_every_operator() {
        // Guard the whole mapping table — every glyph → its `op_*` token — so a
        // dropped or mistyped arm fails loudly (and the table stays honestly
        // covered rather than a lightly-tested high-arity match).
        for (ch, want) in [
            ('<', "lt"),
            ('>', "gt"),
            ('=', "eq"),
            ('+', "add"),
            ('-', "sub"),
            ('*', "mul"),
            ('/', "div"),
            ('%', "mod"),
            ('!', "bang"),
            ('&', "amp"),
            ('|', "pipe"),
            ('^', "caret"),
            ('~', "tilde"),
            ('?', "q"),
        ] {
            assert_eq!(swift_op_glyph(ch), Some(want), "glyph {ch:?}");
        }
        assert_eq!(swift_op_glyph('x'), None);
    }

    #[test]
    fn swift_labels_and_hash_overload() {
        assert_eq!(
            render(
                Language::Swift,
                "sw:A.Argument.init(help:completion:)#796bbd"
            ),
            "sw:A.Argument.init+help:completion:#796bbd"
        );
    }

    #[test]
    fn go_python_markdown_floor_only() {
        assert_eq!(
            render(Language::Go, "go:github.com/samber/lo.Map"),
            "go:github.com/samber/lo.Map"
        );
        assert!(!has_hostile(&render(
            Language::Markdown,
            "md:d.md#My Section (draft)"
        )));
    }

    #[test]
    fn floor_coalesces_hostile_runs_and_keeps_identifiers() {
        // Each maximal run of hostile chars → one `_` (readable, no `_x<NN>_`).
        assert_eq!(floor("Some Note"), "Some_Note");
        assert_eq!(floor("a<>b"), "a_b"); // `<>` run coalesces to one `_`
        assert_eq!(floor("x%20y"), "x_20y"); // only `%` is hostile; `20` is safe
        assert_eq!(floor("$relpath^logo"), "_relpath_logo"); // `$` and `^` → `_`
                                                             // Existing underscores pass through — `__init__` is not corrupted.
        assert_eq!(floor("py:m.C.__init__"), "py:m.C.__init__");
        // Output is always shell-safe + idempotent.
        assert!(!has_hostile(&floor("a b<c>(d)")));
        assert_eq!(floor(&floor("a b<c>(d)")), floor("a b<c>(d)"));
    }
}
