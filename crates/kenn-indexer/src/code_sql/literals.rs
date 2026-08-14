//! String-literal recovery from source text, per language.
//!
//! **Pure.** No store, no filesystem — text in, literals out, each tagged with
//! the 1-based line it starts on so a caller can place it against the stored
//! body extents.
//!
//! The index carries symbols, ranges, and roles, never literal *values*, so
//! SQL written inside code is invisible to it. Reading the source back is what
//! makes it visible, and the extents needed to place what is found are already
//! stored, captured for source retrieval.
//!
//! The raw and verbatim forms are not an afterthought here: SQL is written in
//! exactly those forms precisely because it contains quotes and newlines, so a
//! scanner handling only the plain form misses the queries most worth finding.

/// One recovered literal and where it begins.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Literal {
    /// The literal's contents, with the delimiters removed.
    pub text: String,
    /// 1-based line the literal starts on.
    pub line: u32,
}

/// Recover the string literals in `src` using `language`'s literal syntax.
///
/// A language with no scanner yields nothing. Absence of support is not a
/// defect in the workspace being indexed, so it is silent rather than reported.
#[must_use]
pub fn literals(language: &str, src: &str) -> Vec<Literal> {
    match language {
        "rust" => scan(src, Syntax::Rust),
        "csharp" => scan(src, Syntax::CSharp),
        "typescript" | "javascript" => scan(src, Syntax::EcmaScript),
        _ => Vec::new(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Syntax {
    Rust,
    CSharp,
    EcmaScript,
}

/// Walk `src` once, emitting each literal with its starting line.
///
/// Deliberately a scanner rather than a parser: it needs to find literals, not
/// understand the program. It does track line comments, because a commented-out
/// query is not a reference and `-- DROP TABLE users` inside `// …` would
/// otherwise read as one.
#[expect(
    clippy::indexing_slicing,
    reason = "every index is guarded by an `i < c.len()` / `i + 1 < c.len()` test on the line above it"
)]
fn scan(src: &str, syntax: Syntax) -> Vec<Literal> {
    let c: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let mut line = 1u32;
    let mut i = 0usize;
    while i < c.len() {
        let ch = c[i];
        if ch == '\n' {
            line += 1;
            i += 1;
            continue;
        }
        // Comments: skip so their contents never read as code.
        if ch == '/' && i + 1 < c.len() && c[i + 1] == '/' {
            while i < c.len() && c[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && i + 1 < c.len() && c[i + 1] == '*' {
            i += 2;
            while i + 1 < c.len() && !(c[i] == '*' && c[i + 1] == '/') {
                if c[i] == '\n' {
                    line += 1;
                }
                i += 1;
            }
            i = (i + 2).min(c.len());
            continue;
        }
        if let Some((text, next, newlines)) = literal_at(&c, i, syntax) {
            out.push(Literal { text, line });
            line += newlines;
            i = next;
            continue;
        }
        i += 1;
    }
    out
}

/// Try to read a literal starting at `i`. Returns its contents, the index just
/// past it, and how many newlines it spanned.
#[expect(
    clippy::indexing_slicing,
    reason = "the only indexing is `c[j]` inside `while j < c.len()`"
)]
fn literal_at(c: &[char], i: usize, syntax: Syntax) -> Option<(String, usize, u32)> {
    let at = |k: usize| c.get(k).copied();
    match syntax {
        // `r"…"` / `r#"…"#` — no escapes at all, which is why SQL uses them.
        Syntax::Rust if at(i) == Some('r') => {
            let mut hashes = 0usize;
            let mut j = i + 1;
            while at(j) == Some('#') {
                hashes += 1;
                j += 1;
            }
            if at(j) != Some('"') {
                return None;
            }
            let close: Vec<char> = std::iter::once('"')
                .chain(std::iter::repeat_n('#', hashes))
                .collect();
            let body_start = j + 1;
            let end = find(c, body_start, &close)?;
            Some(collect(c, body_start, end, end + close.len()))
        }
        // `@"…"` — a doubled `""` is one quote, and `\` is literal.
        Syntax::CSharp if at(i) == Some('@') && at(i + 1) == Some('"') => {
            let mut s = String::new();
            let mut j = i + 2;
            let mut nl = 0u32;
            while j < c.len() {
                if c[j] == '"' {
                    if at(j + 1) == Some('"') {
                        s.push('"');
                        j += 2;
                        continue;
                    }
                    return Some((s, j + 1, nl));
                }
                if c[j] == '\n' {
                    nl += 1;
                }
                s.push(c[j]);
                j += 1;
            }
            None
        }
        // `"""…"""` — raw, multi-line.
        Syntax::CSharp
            if at(i) == Some('"') && at(i + 1) == Some('"') && at(i + 2) == Some('"') =>
        {
            let close = ['"', '"', '"'];
            let end = find(c, i + 3, &close)?;
            Some(collect(c, i + 3, end, end + 3))
        }
        _ => {
            let quote = at(i)?;
            let opens = match syntax {
                // Kept as separate arms rather than merged: the two languages
                // agree on the plain form by coincidence, not by rule, and the
                // next language added will not.
                Syntax::Rust | Syntax::CSharp => quote == '"',
                Syntax::EcmaScript => quote == '"' || quote == '\'' || quote == '`',
            };
            if !opens {
                return None;
            }
            let mut s = String::new();
            let mut j = i + 1;
            let mut nl = 0u32;
            while j < c.len() {
                if c[j] == '\\' {
                    // Keep an escaped quote; replace any other escape with a
                    // space so `\n` between SQL keywords stays a separator
                    // rather than gluing two tokens together.
                    match at(j + 1) {
                        Some('"') => s.push('"'),
                        Some('\'') => s.push('\''),
                        _ => s.push(' '),
                    }
                    j += 2;
                    continue;
                }
                if c[j] == quote {
                    return Some((s, j + 1, nl));
                }
                if c[j] == '\n' {
                    nl += 1;
                }
                s.push(c[j]);
                j += 1;
            }
            None
        }
    }
}

/// Index of the next occurrence of `needle` at or after `from`.
#[expect(
    clippy::indexing_slicing,
    reason = "the range stops `needle.len() - 1` short of the end, so the window is always in bounds"
)]
fn find(c: &[char], from: usize, needle: &[char]) -> Option<usize> {
    (from..c.len().saturating_sub(needle.len() - 1)).find(|&k| c[k..k + needle.len()] == *needle)
}

/// `(contents, index past the literal, newlines spanned)` for `c[start..end]`.
#[expect(
    clippy::indexing_slicing,
    reason = "`end` comes from `find`, which only returns an in-bounds index"
)]
fn collect(c: &[char], start: usize, end: usize, next: usize) -> (String, usize, u32) {
    let text: String = c[start..end].iter().collect();
    let nl = u32::try_from(text.matches('\n').count()).unwrap_or(0);
    (text, next, nl)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(lang: &str, src: &str) -> Vec<String> {
        literals(lang, src).into_iter().map(|l| l.text).collect()
    }

    #[test]
    fn a_plain_literal_is_recovered_with_its_line() {
        let got = literals("rust", "fn a() {\n    let q = \"SELECT 1\";\n}");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "SELECT 1");
        assert_eq!(got[0].line, 2, "1-based line the literal starts on");
    }

    #[test]
    fn a_rust_raw_string_keeps_its_quotes_and_newlines() {
        // The form real SQL is written in — a scanner that only handles the
        // plain form misses exactly the queries worth finding.
        let src = "let q = r#\"SELECT \"id\" FROM users\nWHERE x = 1\"#;";
        let got = texts("rust", src);
        assert_eq!(got, vec!["SELECT \"id\" FROM users\nWHERE x = 1"]);
    }

    #[test]
    fn a_csharp_verbatim_string_unescapes_doubled_quotes() {
        let got = texts("csharp", "var q = @\"SELECT \"\"id\"\" FROM users\";");
        assert_eq!(got, vec!["SELECT \"id\" FROM users"]);
    }

    #[test]
    fn a_csharp_raw_string_is_recovered() {
        let got = texts("csharp", "var q = \"\"\"SELECT * FROM users\"\"\";");
        assert_eq!(got, vec!["SELECT * FROM users"]);
    }

    #[test]
    fn typescript_recovers_all_three_quote_forms() {
        let got = texts(
            "typescript",
            "const a = 'SELECT 1'; const b = \"SELECT 2\"; const c = `SELECT 3`;",
        );
        assert_eq!(got, vec!["SELECT 1", "SELECT 2", "SELECT 3"]);
    }

    #[test]
    fn an_escape_becomes_a_separator_not_a_join() {
        // `\n` between clauses must not glue `users` to `WHERE`.
        let got = texts("rust", r#"let q = "SELECT * FROM users\nWHERE id = 1";"#);
        assert_eq!(got, vec!["SELECT * FROM users WHERE id = 1"]);
    }

    #[test]
    fn a_commented_out_query_is_not_a_literal() {
        // A commented query is not a reference. Without this the scanner would
        // report dead SQL as a live table access.
        let src = "// let q = \"DROP TABLE users\";\nlet r = \"SELECT 1\";";
        assert_eq!(texts("rust", src), vec!["SELECT 1"]);
    }

    #[test]
    fn a_block_comment_is_skipped_and_still_counts_its_lines() {
        let got = literals(
            "rust",
            "/* \"DROP TABLE users\"\n   more\n*/\nlet q = \"SELECT 1\";",
        );
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].text, "SELECT 1");
        assert_eq!(got[0].line, 4, "lines inside the comment still counted");
    }

    #[test]
    fn a_language_with_no_scanner_is_silent() {
        assert!(literals("swift", "let q = \"SELECT 1\"").is_empty());
    }

    #[test]
    fn lines_advance_across_multi_line_literals() {
        let src = "let a = r#\"one\ntwo\"#;\nlet b = \"SELECT 1\";";
        let got = literals("rust", src);
        assert_eq!(got.len(), 2);
        assert_eq!(got[1].text, "SELECT 1");
        assert_eq!(got[1].line, 3, "the raw string's newline was counted");
    }
}
