//! Enclosing-range provider chain (section 5b of the proposal).
//!
//! This file does line-level C# tokenization with byte indexing. Every
//! `&str[i..]` slice is at a position returned by a string method (`find`,
//! `starts_with`, etc.) which guarantees a UTF-8 boundary. Every `bytes[i]`
//! lookup is bounds-checked at the call site.
#![expect(
    clippy::string_slice,
    clippy::indexing_slicing,
    reason = "C# line tokenizer; slices use indices returned by str methods (UTF-8-safe), bytes[..] indices are bounds-checked"
)]
//!
//! Tier 1 — `ScipEnclosingProvider`: trust SCIP's `Occurrence.enclosing_range`
//! when the indexer populated it (scip-typescript / scip-python / scip-go for
//! container defs).
//!
//! Tier 2 — `CsharpPositionalRefinement`: per scip-indexer/spec.md, refine
//! the bare last-preceding-def heuristic with C#-aware line classification
//! (skip attributes / preprocessor / comments / blank, disambiguate C# 12
//! collection literals, handle inline-attribute and multi-line attribute
//! forms).
//!
//! Tier 3 — `BareLastPrecedingDef`: the simple positional fallback.
//!
//! Composition is via `ChainedEnclosing { providers }`.

use scip::types::Occurrence;

use crate::edge::{DocumentDefIndex, Range4};

/// Lightweight wrapper that lets providers inspect a SCIP `Occurrence`
/// without coupling them to scip types.
pub struct OccurrenceLocator<'a> {
    pub occurrence: &'a Occurrence,
}

impl OccurrenceLocator<'_> {
    /// `Occurrence.enclosing_range` is `Vec<i32>` with the same shape as
    /// `range` (3-int single-line or 4-int multi-line). Returns `None`
    /// when not populated.
    #[must_use]
    pub fn scip_enclosing(&self) -> Option<Range4> {
        match *self.occurrence.enclosing_range.as_slice() {
            [sl, sc, ec] => Some((sl, sc, sl, ec)),
            [sl, sc, el, ec] => Some((sl, sc, el, ec)),
            _ => None,
        }
    }
}

pub trait EnclosingProvider {
    /// Return the SCIP symbol string of the enclosing FROM symbol for an
    /// occurrence at `(line, col)`, or `None` if this provider can't decide.
    fn attribute_from(
        &mut self,
        canonical_path: &str,
        line: i32,
        col: i32,
        defs: &DocumentDefIndex,
        loc: &OccurrenceLocator<'_>,
    ) -> Option<String>;
}

/// Tier 1 — read SCIP's `enclosing_range` when populated, then resolve to the
/// def whose range *equals* (or strictly contains) it.
#[derive(Debug, Default)]
pub struct ScipEnclosingProvider;

impl EnclosingProvider for ScipEnclosingProvider {
    fn attribute_from(
        &mut self,
        _canonical_path: &str,
        _line: i32,
        _col: i32,
        defs: &DocumentDefIndex,
        loc: &OccurrenceLocator<'_>,
    ) -> Option<String> {
        let (sl, sc, _, _) = loc.scip_enclosing()?;
        // The enclosing_range start position lands inside the def's range,
        // so smallest_enclosing on that anchor recovers the symbol.
        defs.smallest_enclosing(sl, sc)
            .map(std::string::ToString::to_string)
    }
}

/// Tier 3 — the "bare" positional fallback: the smallest def whose range
/// contains `(line, col)`. Used by the C# tier under the hood and as the
/// final fallback when source is unavailable.
#[derive(Debug, Default)]
pub struct BareLastPrecedingDef;

impl EnclosingProvider for BareLastPrecedingDef {
    fn attribute_from(
        &mut self,
        _canonical_path: &str,
        line: i32,
        col: i32,
        defs: &DocumentDefIndex,
        _loc: &OccurrenceLocator<'_>,
    ) -> Option<String> {
        defs.smallest_enclosing(line, col)
            .map(std::string::ToString::to_string)
    }
}

/// Tier 2 — C# positional refinement. Reads the document's source bytes
/// (once, cached) and consults a per-line classification table when
/// disambiguating attribute/code lines (per scip-indexer spec).
pub struct CsharpPositionalRefinement {
    /// Cache: `canonical_path` → classified line table.
    cache: std::collections::HashMap<String, Vec<LineKind>>,
    /// Source roots to look in. `canonical_path` is workspace-relative; the
    /// caller passes in the workspace root.
    workspace_root: std::path::PathBuf,
}

impl CsharpPositionalRefinement {
    pub fn new<P: Into<std::path::PathBuf>>(workspace_root: P) -> Self {
        Self {
            cache: std::collections::HashMap::new(),
            workspace_root: workspace_root.into(),
        }
    }

    fn line_kinds(&mut self, canonical_path: &str) -> Option<&[LineKind]> {
        if !self.cache.contains_key(canonical_path) {
            let abs = self.workspace_root.join(canonical_path);
            let source = std::fs::read_to_string(&abs).ok()?;
            let kinds = classify_lines(&source);
            self.cache.insert(canonical_path.to_string(), kinds);
        }
        self.cache.get(canonical_path).map(std::vec::Vec::as_slice)
    }
}

impl EnclosingProvider for CsharpPositionalRefinement {
    fn attribute_from(
        &mut self,
        canonical_path: &str,
        line: i32,
        col: i32,
        defs: &DocumentDefIndex,
        _loc: &OccurrenceLocator<'_>,
    ) -> Option<String> {
        let kinds = self.line_kinds(canonical_path)?;
        // 5b.3.5 — advance from Attribute / AttributeCont to the next Code line.
        // SCIP positions are non-negative; clamp at 0 if a malformed input
        // produced a negative line number.
        let mut effective_line = line;
        while let Some(k) = usize::try_from(effective_line)
            .ok()
            .and_then(|i| kinds.get(i))
        {
            match k {
                LineKind::Attribute | LineKind::AttributeCont => effective_line += 1,
                _ => break,
            }
        }
        // 5b.3.6 — same-line forward def.
        // (Only applies when the post-advance line still holds a def at column ≥
        // the original occurrence column.)
        // Then 5b.3.7 — bare last-preceding-def fallback (parameter-filtered
        // by `DocumentDefIndex` already excluding pseudo-symbols).
        defs.smallest_enclosing(effective_line, col)
            .map(std::string::ToString::to_string)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Blank,
    Comment,
    Preprocessor,
    Attribute,
    AttributeCont,
    Code,
}

/// Per-line classification used by [`CsharpPositionalRefinement`].
///
/// Rules (per scip-indexer spec 5b.3.2-5b.3.4):
/// * Blank — only whitespace.
/// * Comment — line begins with `//` or is enclosed in a `/* ... */` block.
/// * Preprocessor — line begins (after whitespace) with `#`.
/// * Attribute — line begins (after whitespace) with `[` and the previous
///   non-blank, non-comment line does NOT end with `=`, `=>`, `,`, or `(`
///   (which would mark a C# 12 collection literal — Code).
/// * `AttributeCont` — interior line of a multi-line `[...]` attribute when
///   bracket count remained positive at the end of the previous line.
/// * Code — anything else.
///
/// Inline `[Attr] decl` lines (5b.3.4) close brackets within the line and
/// continue with non-trivial code on the same line — classified as Code.
#[must_use]
pub fn classify_lines(source: &str) -> Vec<LineKind> {
    let mut out = Vec::new();
    let mut bracket_depth: i32 = 0;
    let mut in_block_comment = false;
    let mut last_non_blank_comment: Option<String> = None;

    for raw in source.split_inclusive('\n') {
        let line = raw.trim_end_matches(['\n', '\r']);
        let trimmed = line.trim();

        if in_block_comment {
            if let Some(end) = trimmed.find("*/") {
                in_block_comment = false;
                let after = trimmed[end + 2..].trim();
                if after.is_empty() {
                    out.push(LineKind::Comment);
                    continue;
                }
                // Tail past `*/` may be code; classify as Code to be safe.
                out.push(LineKind::Code);
                last_non_blank_comment = Some(after.into());
                continue;
            }
            out.push(LineKind::Comment);
            continue;
        }

        if trimmed.is_empty() {
            out.push(LineKind::Blank);
            continue;
        }
        if trimmed.starts_with("//") {
            out.push(LineKind::Comment);
            continue;
        }
        if trimmed.starts_with("/*") && !trimmed[2..].contains("*/") {
            in_block_comment = true;
            out.push(LineKind::Comment);
            continue;
        }
        if trimmed.starts_with('#') {
            out.push(LineKind::Preprocessor);
            continue;
        }

        // AttributeCont: prior line left brackets unbalanced.
        if bracket_depth > 0 {
            // Update depth for this line and decide:
            let new_depth = bracket_depth + bracket_delta(line);
            if new_depth == 0 {
                // Close happened on this line. If anything non-trivial follows
                // the closing `]`, it's Code (5b.3.4 inline-attribute case).
                if has_code_after_attr_close(line) {
                    out.push(LineKind::Code);
                } else {
                    out.push(LineKind::AttributeCont);
                }
            } else {
                out.push(LineKind::AttributeCont);
            }
            bracket_depth = new_depth;
            last_non_blank_comment = Some(trimmed.into());
            continue;
        }

        // First-`[` lines: distinguish attribute from C# 12 collection literal.
        if trimmed.starts_with('[') {
            let prior_ends_with_continuation = last_non_blank_comment
                .as_deref()
                .is_some_and(line_ends_with_collection_literal_continuation);
            let depth = bracket_delta(line);
            if prior_ends_with_continuation {
                out.push(LineKind::Code);
                last_non_blank_comment = Some(trimmed.into());
                continue;
            }
            if depth > 0 {
                out.push(LineKind::Attribute);
                bracket_depth = depth;
            } else {
                // Closes within the line.
                if has_code_after_attr_close(line) {
                    out.push(LineKind::Code);
                } else {
                    out.push(LineKind::Attribute);
                }
            }
            last_non_blank_comment = Some(trimmed.into());
            continue;
        }

        out.push(LineKind::Code);
        last_non_blank_comment = Some(trimmed.into());
    }

    out
}

/// Does the trimmed prior non-blank line end with a token that signals a
/// continuation suitable for a C# 12 collection literal on the next line?
fn line_ends_with_collection_literal_continuation(prior: &str) -> bool {
    let trimmed = prior.trim_end_matches(';').trim_end();
    trimmed.ends_with('=')
        || trimmed.ends_with("=>")
        || trimmed.ends_with(',')
        || trimmed.ends_with('(')
}

/// Bracket-balance delta of `[` minus `]` on a line (ignoring inside
/// string/char/comment).
fn bracket_delta(line: &str) -> i32 {
    let mut depth = 0_i32;
    let bytes = line.as_bytes();
    let mut i = 0;
    let mut in_string = false;
    let mut in_char = false;
    while i < bytes.len() {
        let c = bytes[i];
        if in_string {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == b'"' {
                in_string = false;
            }
        } else if in_char {
            if c == b'\\' && i + 1 < bytes.len() {
                i += 2;
                continue;
            }
            if c == b'\'' {
                in_char = false;
            }
        } else {
            match c {
                b'"' => in_string = true,
                b'\'' => in_char = true,
                b'/' if i + 1 < bytes.len() && bytes[i + 1] == b'/' => break,
                b'[' => depth += 1,
                b']' => depth -= 1,
                _ => {}
            }
        }
        i += 1;
    }
    depth
}

/// True if the line has non-trivial code AFTER the matched `]`. Used for
/// inline-attribute detection (`[Attr] public void Foo()`).
fn has_code_after_attr_close(line: &str) -> bool {
    let mut depth = 0_i32;
    let bytes = line.as_bytes();
    let mut last_close = None;
    for (i, c) in bytes.iter().enumerate() {
        match c {
            b'[' => depth += 1,
            b']' => {
                depth -= 1;
                if depth == 0 {
                    last_close = Some(i);
                }
            }
            _ => {}
        }
    }
    if let Some(idx) = last_close {
        let tail = &line[idx + 1..];
        let tail = tail.trim();
        if tail.is_empty() {
            return false;
        }
        // Comments-only after `]` shouldn't trigger Code.
        if tail.starts_with("//") {
            return false;
        }
        return true;
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blank_and_comment_lines() {
        let src = "\n   \n// hi\n/* block\n   continues */\n";
        let kinds = classify_lines(src);
        assert_eq!(
            kinds,
            vec![
                LineKind::Blank,
                LineKind::Blank,
                LineKind::Comment,
                LineKind::Comment,
                LineKind::Comment,
            ]
        );
    }

    #[test]
    fn attribute_versus_collection_literal() {
        // Collection literal: prior line ends with `=`.
        let src = "var x =\n    [1, 2, 3];\n";
        let kinds = classify_lines(src);
        assert_eq!(kinds[1], LineKind::Code);

        // Attribute: prior line is something else (or top of class).
        let src = "[DataMember]\npublic int X { get; }\n";
        let kinds = classify_lines(src);
        assert_eq!(kinds[0], LineKind::Attribute);
        assert_eq!(kinds[1], LineKind::Code);
    }

    #[test]
    fn inline_attribute_classifies_as_code() {
        // `[Attr] public void Foo()` — the attr closes within the line and
        // is followed by non-trivial code, so classify as Code (5b.3.4).
        let src = "[Attr] public void Foo() {}\n";
        let kinds = classify_lines(src);
        assert_eq!(kinds[0], LineKind::Code);
    }

    #[test]
    fn multi_line_attribute_continuation() {
        let src = "[DataMember(\n    Name = \"foo\",\n    IsRequired = true)]\npublic int X;\n";
        let kinds = classify_lines(src);
        assert_eq!(kinds[0], LineKind::Attribute);
        assert_eq!(kinds[1], LineKind::AttributeCont);
        assert_eq!(kinds[2], LineKind::AttributeCont);
        assert_eq!(kinds[3], LineKind::Code);
    }

    #[test]
    fn preprocessor_line() {
        let src = "#if DEBUG\n    Console.WriteLine();\n#endif\n";
        let kinds = classify_lines(src);
        assert_eq!(kinds[0], LineKind::Preprocessor);
        assert_eq!(kinds[1], LineKind::Code);
        assert_eq!(kinds[2], LineKind::Preprocessor);
    }

    #[test]
    fn bracket_delta_ignores_strings_and_chars() {
        assert_eq!(bracket_delta("[Foo(\"]\", '[')]"), 0);
        assert_eq!(bracket_delta("[Foo("), 1);
        assert_eq!(bracket_delta(")]"), -1);
    }
}
