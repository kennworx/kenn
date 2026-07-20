//! Minimal SCIP descriptor segmenter.
//!
//! This is a hand-rolled byte-level parser. Every `bytes[i]` index is gated
//! by `i < bytes.len()` (or follows a `read_until` that returned a known
//! in-bounds end). Every `from_utf8(&bytes[a..b])` slices a sub-range of a
//! `&str`'s already-validated UTF-8, so the result is always `Ok`. Clippy
//! can't see those invariants; the file-level `expect`s below silence the
//! resulting noise without weakening the lints anywhere else.
#![expect(
    clippy::indexing_slicing,
    clippy::unwrap_used,
    clippy::unwrap_in_result,
    reason = "byte-level parser; every index is bounds-checked, every from_utf8 slices a sub-range of valid UTF-8"
)]
//!
//! Per the SCIP spec, the descriptor is a sequence of segments terminated by
//! a single suffix character that classifies the segment:
//!
//! * `name/`        — namespace
//! * `name#`        — type
//! * `name.`        — term (field, constant, ...)
//! * `name(sig).`   — method (with optional disambiguator)
//! * `[name]`       — type parameter
//! * `(name)`       — parameter
//! * `name:`        — meta
//! * `name!`        — macro
//!
//! Names may be backtick-quoted to allow special characters; an embedded
//! backtick is escaped by doubling it inside the quoted name.
//!
//! This parser is intentionally tolerant — it is used at ingest time to slice
//! a descriptor into well-typed segments; per-language transformers decide how
//! to emit the public form.

use super::IdError;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment<'a> {
    Namespace(&'a str),
    Type(&'a str),
    Term(&'a str),
    Method { name: &'a str, signature: &'a str },
    TypeParam(&'a str),
    Parameter(&'a str),
    Meta(&'a str),
    Macro(&'a str),
}

pub fn parse_descriptor(input: &str) -> Result<Vec<Segment<'_>>, IdError> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut out = Vec::new();
    while i < bytes.len() {
        match bytes[i] {
            b'[' => {
                let (name_end, after) = read_until(bytes, i + 1, b']')?;
                out.push(Segment::TypeParam(
                    std::str::from_utf8(&bytes[i + 1..name_end]).unwrap(),
                ));
                i = after;
            }
            b'(' => {
                let (name_end, after) = read_until(bytes, i + 1, b')')?;
                out.push(Segment::Parameter(
                    std::str::from_utf8(&bytes[i + 1..name_end]).unwrap(),
                ));
                i = after;
            }
            _ => {
                let (name_end, after_name) = read_name(bytes, i)?;
                let name = std::str::from_utf8(&bytes[i..name_end]).unwrap();
                if after_name >= bytes.len() {
                    return Err(IdError::BadDescriptor(format!(
                        "missing suffix after `{name}` in `{input}`"
                    )));
                }
                match bytes[after_name] {
                    b'/' => {
                        out.push(Segment::Namespace(trim_backticks(name)));
                        i = after_name + 1;
                    }
                    b'#' => {
                        out.push(Segment::Type(trim_backticks(name)));
                        i = after_name + 1;
                    }
                    b'(' => {
                        // method: read disambiguator until `).`
                        let (sig_end, after_sig) = read_until(bytes, after_name + 1, b')')?;
                        if after_sig >= bytes.len() || bytes[after_sig] != b'.' {
                            return Err(IdError::BadDescriptor(format!(
                                "method `{name}` missing trailing `.` in `{input}`"
                            )));
                        }
                        out.push(Segment::Method {
                            name: trim_backticks(name),
                            signature: std::str::from_utf8(&bytes[after_name + 1..sig_end])
                                .unwrap(),
                        });
                        i = after_sig + 1;
                    }
                    b'.' => {
                        out.push(Segment::Term(trim_backticks(name)));
                        i = after_name + 1;
                    }
                    b':' => {
                        out.push(Segment::Meta(trim_backticks(name)));
                        i = after_name + 1;
                    }
                    b'!' => {
                        out.push(Segment::Macro(trim_backticks(name)));
                        i = after_name + 1;
                    }
                    other => {
                        return Err(IdError::BadDescriptor(format!(
                            "unexpected suffix `{}` after `{name}` in `{input}`",
                            other as char
                        )));
                    }
                }
            }
        }
    }
    Ok(out)
}

/// Return the descriptor with its last segment removed, or `None` if the
/// descriptor has zero or one segments (no enclosing parent).
///
/// Used at SCIP ingest time to set `SymbolRecord.enclosing_symbol` —
/// `Foo#bar.` parents to `Foo#`, `pkg/util/Foo#bar().` to
/// `pkg/util/Foo#`, etc. The returned slice is always a prefix of the
/// input, ending immediately after a segment terminator (`/`, `#`, `.`,
/// `:`, `!`, `]`, `)`), so combining it with the SCIP head produces a
/// well-formed parent SCIP symbol.
#[must_use]
pub fn descriptor_parent(input: &str) -> Option<&str> {
    let bytes = input.as_bytes();
    let mut i = 0;
    let mut prev_end: Option<usize> = None;
    let mut last_end: Option<usize> = None;
    while i < bytes.len() {
        prev_end = last_end;
        let next = match bytes[i] {
            b'[' => match read_until(bytes, i + 1, b']') {
                Ok((_, after)) => after,
                Err(_) => return None,
            },
            b'(' => match read_until(bytes, i + 1, b')') {
                Ok((_, after)) => after,
                Err(_) => return None,
            },
            _ => {
                let Ok((_name_end, after_name)) = read_name(bytes, i) else {
                    return None;
                };
                if after_name >= bytes.len() {
                    return None;
                }
                match bytes[after_name] {
                    b'/' | b'#' | b'.' | b':' | b'!' => after_name + 1,
                    b'(' => {
                        let Ok((_sig_end, after_sig)) = read_until(bytes, after_name + 1, b')')
                        else {
                            return None;
                        };
                        if after_sig >= bytes.len() || bytes[after_sig] != b'.' {
                            return None;
                        }
                        after_sig + 1
                    }
                    _ => return None,
                }
            }
        };
        last_end = Some(next);
        i = next;
    }
    let cut = prev_end?;
    // The parser only lands `cut` after a complete segment, always on a
    // UTF-8 boundary; `get` returns `Some` there and avoids the panic.
    input.get(..cut)
}

/// Read a name, handling backtick-quoted forms (a doubled backtick inside a
/// quoted name escapes a literal backtick). Stops before a suffix character.
/// Returns (`end_of_name`, `index_at_suffix`).
fn read_name(bytes: &[u8], start: usize) -> Result<(usize, usize), IdError> {
    if start < bytes.len() && bytes[start] == b'`' {
        let mut i = start + 1;
        while i < bytes.len() {
            if bytes[i] == b'`' {
                if i + 1 < bytes.len() && bytes[i + 1] == b'`' {
                    i += 2;
                    continue;
                }
                return Ok((i + 1, i + 1));
            }
            i += 1;
        }
        Err(IdError::BadDescriptor(format!(
            "unterminated backtick-quoted name at byte {start}"
        )))
    } else {
        let mut i = start;
        while i < bytes.len() {
            match bytes[i] {
                b'/' | b'#' | b'.' | b':' | b'!' | b'(' | b'[' | b']' | b')' => break,
                _ => i += 1,
            }
        }
        Ok((i, i))
    }
}

fn read_until(bytes: &[u8], start: usize, terminator: u8) -> Result<(usize, usize), IdError> {
    let mut i = start;
    while i < bytes.len() {
        if bytes[i] == terminator {
            return Ok((i, i + 1));
        }
        i += 1;
    }
    Err(IdError::BadDescriptor(format!(
        "unterminated `{}` at byte {start}",
        terminator as char
    )))
}

fn trim_backticks(name: &str) -> &str {
    name.strip_prefix('`')
        .and_then(|n| n.strip_suffix('`'))
        .unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::{parse_descriptor, Segment};

    #[test]
    fn namespaces_and_type() {
        let segs = parse_descriptor("Microsoft/AspNetCore/Mvc/ControllerBase#").unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::Namespace("Microsoft"),
                Segment::Namespace("AspNetCore"),
                Segment::Namespace("Mvc"),
                Segment::Type("ControllerBase"),
            ]
        );
    }

    #[test]
    fn method_with_signature() {
        let segs = parse_descriptor("Foo#Bar().").unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::Type("Foo"),
                Segment::Method {
                    name: "Bar",
                    signature: ""
                },
            ]
        );
    }

    #[test]
    fn method_with_overload() {
        let segs = parse_descriptor("Foo#Bar(string,int).").unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::Type("Foo"),
                Segment::Method {
                    name: "Bar",
                    signature: "string,int",
                },
            ]
        );
    }

    #[test]
    fn type_param_and_parameter() {
        let segs = parse_descriptor("foo().[T](x)").unwrap();
        assert_eq!(
            segs,
            vec![
                Segment::Method {
                    name: "foo",
                    signature: ""
                },
                Segment::TypeParam("T"),
                Segment::Parameter("x"),
            ]
        );
    }

    #[test]
    fn backtick_quoted_name() {
        let segs = parse_descriptor("`Foo Bar`#").unwrap();
        assert_eq!(segs, vec![Segment::Type("Foo Bar")]);
    }

    #[test]
    fn parent_strips_last_segment() {
        use super::descriptor_parent;
        assert_eq!(descriptor_parent("Foo#bar."), Some("Foo#"));
        assert_eq!(descriptor_parent("Foo#bar()."), Some("Foo#"));
        assert_eq!(descriptor_parent("Foo#Bar(string,int)."), Some("Foo#"));
        assert_eq!(
            descriptor_parent("pkg/util/Foo#bar."),
            Some("pkg/util/Foo#")
        );
        assert_eq!(
            descriptor_parent("Microsoft/AspNetCore/Mvc/ControllerBase#"),
            Some("Microsoft/AspNetCore/Mvc/")
        );
        // Single segment → no parent.
        assert_eq!(descriptor_parent("Foo#"), None);
        // Empty input → no parent.
        assert_eq!(descriptor_parent(""), None);
        // Type-param dropping.
        assert_eq!(descriptor_parent("foo().[T]"), Some("foo()."));
    }

    #[test]
    fn macro_and_meta() {
        let segs = parse_descriptor("println!").unwrap();
        assert_eq!(segs, vec![Segment::Macro("println")]);

        let segs = parse_descriptor("foo:").unwrap();
        assert_eq!(segs, vec![Segment::Meta("foo")]);
    }
}
