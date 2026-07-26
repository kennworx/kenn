//! The TOON grammar: the error type, the token/quoting rules, and the
//! direct-to-writer primitives every serializer shares.
//!
//! Quoting mirrors the `toon` crate's rules for the comma delimiter: a string is
//! written bare only when it can't be mistaken for structure, a number, or a
//! keyword. The `tok_*` helpers return a `String` (row cells are collected then
//! joined); the `write_*` helpers go straight to the sink and allocate nothing.

use std::fmt;
use std::io::{self, Write};

use serde::ser;

pub(super) const DELIM: char = ',';

#[derive(Debug)]
pub struct Error(String);

impl Error {
    pub(super) fn msg(text: &str) -> Self {
        Self(text.to_string())
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Error {}
impl ser::Error for Error {
    fn custom<T: fmt::Display>(msg: T) -> Self {
        Error(msg.to_string())
    }
}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error(e.to_string())
    }
}

pub(super) fn nested() -> Error {
    Error("not a flat-table shape (nested); render as JSON instead".into())
}

/// Write one indented line (with its trailing newline) to the sink.
pub(super) fn line(out: &mut dyn Write, depth: usize, content: &str) -> Result<(), Error> {
    for _ in 0..depth {
        out.write_all(b"  ")?;
    }
    out.write_all(content.as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

// --- primitive token encoding (shared) --------------------------------------

pub(super) fn tok_bool(b: bool) -> String {
    b.to_string()
}

pub(super) fn tok_f64(f: f64) -> String {
    if !f.is_finite() {
        "null".to_string()
    } else if f == 0.0 {
        "0".to_string() // canonicalize -0
    } else if f.fract() == 0.0 && f.abs() < 1e15 {
        format!("{f:.0}")
    } else {
        format!("{f}")
    }
}

pub(super) fn tok_str(s: &str) -> String {
    if is_safe_unquoted(s) {
        s.to_string()
    } else {
        format!("\"{}\"", escape_string(s))
    }
}

fn escape_string(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
        .replace('\t', "\\t")
}

/// A string is bare only when it can't be mistaken for structure, a number, or
/// a keyword. Mirrors `toon`'s `is_safe_unquoted` for the comma delimiter.
fn is_safe_unquoted(s: &str) -> bool {
    !s.is_empty()
        && s == s.trim()
        && s != "true"
        && s != "false"
        && s != "null"
        && !is_numeric_like(s)
        && !s.contains([':', '"', '\\', '[', ']', '{', '}', '\n', '\r', '\t', DELIM])
        && !s.starts_with("- ")
}

/// `^-?\d+(?:\.\d+)?(?:e[+-]?\d+)?$` OR `^0\d+$`, ASCII — the upstream regex.
pub(super) fn is_numeric_like(s: &str) -> bool {
    numeric_main(s) || leading_zero_int(s)
}

fn numeric_main(s: &str) -> bool {
    let b = s.as_bytes();
    let mut i = 0;
    if b.first() == Some(&b'-') {
        i += 1;
    }
    let int_start = i;
    while b.get(i).is_some_and(u8::is_ascii_digit) {
        i += 1;
    }
    if i == int_start {
        return false;
    }
    if b.get(i) == Some(&b'.') {
        i += 1;
        let frac_start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == frac_start {
            return false;
        }
    }
    if b.get(i) == Some(&b'e') {
        i += 1;
        if matches!(b.get(i), Some(&b'+' | &b'-')) {
            i += 1;
        }
        let exp_start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if i == exp_start {
            return false;
        }
    }
    i == b.len()
}

fn leading_zero_int(s: &str) -> bool {
    matches!(s.as_bytes(), [b'0', rest @ ..] if !rest.is_empty() && rest.iter().all(u8::is_ascii_digit))
}

/// A key is bare when it matches `^[A-Za-z_][\w.]*$` (ASCII), else quoted.
fn is_bare_key(k: &str) -> bool {
    matches!(k.as_bytes(), [first, rest @ ..]
        if (first.is_ascii_alphabetic() || *first == b'_')
            && rest.iter().all(|&c| c.is_ascii_alphanumeric() || c == b'_' || c == b'.'))
}

pub(super) fn encode_key(k: &str) -> String {
    if is_bare_key(k) {
        k.to_string()
    } else {
        format!("\"{}\"", escape_string(k))
    }
}

/// `label[N]` — the array header stem (before `{cols}:` / `:` / `: values`).
pub(super) fn array_stem(label: Option<&str>, len: usize) -> String {
    format!("{}[{len}]", label.map(encode_key).unwrap_or_default())
}

// --- direct-to-writer helpers (the scalar path allocates nothing) -----------

pub(super) fn write_indent(out: &mut dyn Write, depth: usize) -> io::Result<()> {
    for _ in 0..depth {
        out.write_all(b"  ")?;
    }
    Ok(())
}

pub(super) fn write_key(out: &mut dyn Write, k: &str) -> io::Result<()> {
    if is_bare_key(k) {
        out.write_all(k.as_bytes())
    } else {
        write_quoted(out, k)
    }
}

/// A `"..."`-quoted, escaped string, written character-by-character — no
/// intermediate `String`.
fn write_quoted(out: &mut dyn Write, s: &str) -> io::Result<()> {
    out.write_all(b"\"")?;
    for ch in s.chars() {
        match ch {
            '\\' => out.write_all(b"\\\\")?,
            '"' => out.write_all(b"\\\"")?,
            '\n' => out.write_all(b"\\n")?,
            '\r' => out.write_all(b"\\r")?,
            '\t' => out.write_all(b"\\t")?,
            _ => write!(out, "{ch}")?,
        }
    }
    out.write_all(b"\"")
}

/// A string value: bare when safe, else quoted — written directly.
pub(super) fn write_str_token(out: &mut dyn Write, s: &str) -> io::Result<()> {
    if is_safe_unquoted(s) {
        out.write_all(s.as_bytes())
    } else {
        write_quoted(out, s)
    }
}

/// A float the way TOON renders it (whole → no decimal, -0 → 0, non-finite →
/// null), written directly.
pub(super) fn write_f64(out: &mut dyn Write, v: f64) -> io::Result<()> {
    if !v.is_finite() {
        out.write_all(b"null")
    } else if v == 0.0 {
        out.write_all(b"0")
    } else if v.fract() == 0.0 && v.abs() < 1e15 {
        write!(out, "{v:.0}")
    } else {
        write!(out, "{v}")
    }
}
