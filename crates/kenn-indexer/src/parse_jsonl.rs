//! Streaming parser for the kenn JSONL wire format.
//!
//! Mirrors `indexers/frames.ts`. Reads frame-per-line from any `BufRead`
//! and invokes the caller's frame handler. Memory bounded by max line
//! length.

use std::io::BufRead;

use serde::Deserialize;

#[derive(Debug, thiserror::Error)]
pub enum ParseJsonlError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("json on line {line}: {source}")]
    Json {
        line: u64,
        #[source]
        source: serde_json::Error,
    },
    #[error("handler: {0}")]
    Handler(String),
}

/// 0-based `[start_line, start_col, end_line, end_col]`.
pub type FrameRange = [i64; 4];

/// Run-local id assigned by the producer (mirrors TS `Ref`).
pub type Ref = u32;

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Frame {
    Meta(MetaFrame),
    Toolchain(ToolchainFrame),
    File(FileFrame),
    Package(PackageFrame),
    Stub(StubFrame),
    Symbol(SymbolFrame),
    Edge(EdgeFrame),
    Error(ErrorFrame),
    End(EndFrame),
}

#[derive(Debug, Deserialize)]
pub struct MetaFrame {
    pub v: u32,
    pub project_root: String,
    pub tool: String,
    pub tool_version: String,
    pub language: String,
    /// ISO 8601 UTC timestamp when the producer wrote this frame
    /// (millisecond precision, `YYYY-MM-DDTHH:mm:ss.sssZ`).
    pub ts: String,
}

/// A toolchain the provisioning entrypoint resolved from the workspace's pin
/// file and made available before exec'ing the indexer.
///
/// Emitted by `kenn-toolchain` (our entrypoint), NOT by the indexer: the
/// indexer runs as a separate exec'd process and reports only its OWN version
/// in [`MetaFrame::tool_version`]. This carries the *provisioned* toolchain —
/// the .NET SDK / Go / Rust the workspace pinned — so a result change is
/// attributable to the toolchain that produced it. One frame per provisioned
/// toolchain: an image may provision more than one (python + node for
/// scip-python).
#[derive(Debug, Deserialize)]
pub struct ToolchainFrame {
    pub language: String,
    pub version: String,
}

#[derive(Debug, Deserialize)]
pub struct FileFrame {
    pub id: Ref,
    pub path: String,
    pub content_hash: String,
    #[serde(default)]
    pub test: bool,
    #[serde(default)]
    pub external: bool,
    /// File-level comment trivia (one entry per contiguous comment block),
    /// raw and unfiltered. Empty when the file has none; license-boilerplate
    /// filtering happens in `transform_jsonl`.
    #[serde(default)]
    pub doc: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct PackageFrame {
    pub id: Ref,
    pub name: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub manager: Option<String>,
    #[serde(default)]
    pub external: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StubFrame {
    pub id: Ref,
    pub kind: String,
    pub name: String,
    pub key: String,
    #[serde(default)]
    pub pkg: Ref,
}

#[derive(Debug, Clone, Deserialize)]
pub struct SymbolFrame {
    pub id: Ref,
    #[serde(default)]
    pub pkg: Ref,
    pub key: String,
    pub kind: String,
    pub name: String,
    #[serde(default)]
    pub parent: Ref,
    #[serde(default)]
    pub file: Ref,
    pub range: FrameRange,
    /// Optional enclosing-item body span (whole declaration incl. doc comment /
    /// attributes), 0-based `[start_line, start_col, end_line, end_col]` — same
    /// convention as `range` (the name span). Omitted when the producer has no
    /// declaration extent; ingest then stores a `0` def body extent and
    /// `get_source` falls back to the name span.
    #[serde(default)]
    pub body: Option<FrameRange>,
    #[serde(default)]
    pub partial: bool,
    // u16, not u8 — see SymbolRecord::nargs. A 257-arg method in Newtonsoft.Json
    // overflowed the u8 and failed the whole C# index at parse time.
    #[serde(default)]
    pub nargs: u16,
    #[serde(default)]
    pub targs: u16,
    #[serde(default)]
    pub test: bool,
    #[serde(default)]
    pub sig: Option<String>,
    #[serde(default)]
    pub doc: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EdgeFrame {
    pub edge_kind: String,
    pub source: Ref,
    pub target: Ref,
    #[serde(default)]
    pub range: Option<FrameRange>,
    #[serde(default)]
    pub field_op: Option<String>,
}

/// `ErrorFrame` severity, validated once at parse time instead of
/// string-matched at each use site. The wire contract is lowercase
/// `"error"` / `"warning"` (frames.ts); parsing is case-insensitive as a
/// producer-drift guard, and anything unrecognized maps to [`Self::Other`],
/// which consumers treat like an error — an unknown severity on an error
/// frame must fail loud, not silently lose attribution.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Error,
    Warning,
    Other,
}

impl<'de> serde::Deserialize<'de> for Severity {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.to_ascii_lowercase().as_str() {
            "error" => Self::Error,
            "warning" | "warn" => Self::Warning,
            _ => Self::Other,
        })
    }
}

#[derive(Debug, Deserialize)]
pub struct ErrorFrame {
    pub severity: Severity,
    pub source: String,
    pub message: String,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub range: Option<FrameRange>,
    #[serde(default)]
    pub code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct EndFrame {
    pub stats: EndStats,
    /// ISO 8601 UTC timestamp when the producer wrote this frame
    /// (millisecond precision, `YYYY-MM-DDTHH:mm:ss.sssZ`). Pair with
    /// `MetaFrame.ts` to compute producer wall time.
    pub ts: String,
}

#[derive(Debug, Deserialize)]
pub struct EndStats {
    pub files: i64,
    pub symbols: i64,
    pub edges: i64,
    pub errors: i64,
}

/// Read frames line-by-line and call `on_frame` for each. Blank lines are
/// skipped silently. The handler may return any `Display` error type;
/// it's wrapped as `ParseJsonlError::Handler`.
pub fn parse_jsonl_stream<R, F, E>(reader: &mut R, mut on_frame: F) -> Result<(), ParseJsonlError>
where
    R: BufRead,
    F: FnMut(Frame) -> Result<(), E>,
    E: std::fmt::Display,
{
    let mut buf = String::new();
    let mut line_no: u64 = 0;
    loop {
        buf.clear();
        line_no += 1;
        let n = reader.read_line(&mut buf)?;
        if n == 0 {
            break;
        }
        let trimmed = buf.trim_end_matches(&['\n', '\r'][..]);
        if trimmed.is_empty() {
            continue;
        }
        let frame: Frame =
            serde_json::from_str(trimmed).map_err(|source| ParseJsonlError::Json {
                line: line_no,
                source,
            })?;
        on_frame(frame).map_err(|e| ParseJsonlError::Handler(e.to_string()))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_meta_then_end() {
        let jsonl = r#"{"type":"meta","v":1,"project_root":"file:///x","tool":"kenn-dotnet","tool_version":"0.1.0","language":"csharp","ts":"2026-05-06T10:00:00.000Z"}
{"type":"end","stats":{"files":0,"symbols":0,"edges":0,"errors":0},"ts":"2026-05-06T10:00:01.000Z"}
"#;
        let mut count = 0;
        parse_jsonl_stream(&mut jsonl.as_bytes(), |f| -> Result<(), String> {
            match f {
                Frame::Meta(_) | Frame::End(_) => count += 1,
                _ => panic!(),
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn parses_a_toolchain_frame() {
        // The entrypoint emits this before the indexer's own frames; the wire
        // must recognize `type:"toolchain"` and carry language + version.
        let jsonl = "{\"type\":\"toolchain\",\"language\":\"dotnet\",\"version\":\"9.0.308\"}\n";
        let mut got: Option<(String, String)> = None;
        parse_jsonl_stream(&mut jsonl.as_bytes(), |f| -> Result<(), String> {
            if let Frame::Toolchain(t) = f {
                got = Some((t.language, t.version));
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(got, Some(("dotnet".to_string(), "9.0.308".to_string())));
    }

    #[test]
    fn parses_package_then_symbol() {
        let jsonl = concat!(
            r#"{"type":"package","id":1,"name":"Web","version":"1.0"}"#,
            "\n",
            r#"{"type":"symbol","id":42,"pkg":1,"key":"Models.Foo","kind":"class","name":"Foo","range":[0,0,5,0]}"#,
            "\n",
        );
        let mut saw_pkg = false;
        let mut saw_sym = false;
        parse_jsonl_stream(&mut jsonl.as_bytes(), |f| -> Result<(), String> {
            match f {
                Frame::Package(p) => {
                    assert_eq!(p.id, 1);
                    assert_eq!(p.name, "Web");
                    saw_pkg = true;
                }
                Frame::Symbol(s) => {
                    assert_eq!(s.id, 42);
                    assert_eq!(s.pkg, 1);
                    assert_eq!(s.key, "Models.Foo");
                    saw_sym = true;
                }
                _ => panic!(),
            }
            Ok(())
        })
        .unwrap();
        assert!(saw_pkg && saw_sym);
    }

    /// A large arity must not fail the whole stream. `nargs`/`targs` were `u8`,
    /// and a 257-arg method in Newtonsoft.Json failed the entire C# index at
    /// parse time with "invalid value: integer 257, expected u8".
    #[test]
    fn a_symbol_with_more_than_255_args_parses() {
        let jsonl = concat!(
            r#"{"type":"symbol","id":1,"pkg":0,"key":"Big.M","kind":"method","name":"M","range":[0,0,1,0],"nargs":257,"targs":300}"#,
            "\n",
        );
        let mut nargs = 0u16;
        parse_jsonl_stream(&mut jsonl.as_bytes(), |f| -> Result<(), String> {
            if let Frame::Symbol(s) = f {
                nargs = s.nargs;
                assert_eq!(s.targs, 300);
            }
            Ok(())
        })
        .unwrap();
        assert_eq!(nargs, 257);
    }

    #[test]
    fn parses_stub_frame() {
        let jsonl = r#"{"type":"stub","id":7,"kind":"class","name":"DateTime","key":"System.DateTime","pkg":3}"#;
        parse_jsonl_stream(&mut jsonl.as_bytes(), |f| -> Result<(), String> {
            if let Frame::Stub(s) = f {
                assert_eq!(s.id, 7);
                assert_eq!(s.key, "System.DateTime");
                assert_eq!(s.pkg, 3);
            } else {
                panic!()
            }
            Ok(())
        })
        .unwrap();
    }

    #[test]
    fn reports_line_number_on_bad_json() {
        let jsonl = "{\"type\":\"meta\",\"v\":1,\"project_root\":\"\",\"tool\":\"x\",\"tool_version\":\"0\",\"language\":\"csharp\",\"ts\":\"2026-05-06T10:00:00.000Z\"}\n{not json\n";
        let err = parse_jsonl_stream(&mut jsonl.as_bytes(), |_| -> Result<(), String> { Ok(()) })
            .unwrap_err();
        match err {
            ParseJsonlError::Json { line, .. } => assert_eq!(line, 2),
            _ => panic!("expected json error"),
        }
    }
}
