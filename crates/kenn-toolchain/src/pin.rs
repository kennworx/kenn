//! Reading the toolchain version a workspace pins for itself.
//!
//! Every language declares its toolchain in a file the repository owns. This
//! module finds the nearest such file and reads the declaration out of it. It
//! does NOT resolve a declaration to a concrete release — a pin can name a
//! channel (`stable`), a partial version (`3.12`), or carry roll-forward
//! semantics; turning that into one artifact is the resolver's job.
//!
//! # Nearest wins, even when it declares nothing
//!
//! The search walks up from the workspace, and the FIRST pin file found decides
//! the answer — including when that file carries no version. A nested
//! `global.json` holding only `msbuild-sdks` shadows a version pin above it,
//! because that is what the .NET SDK resolver itself does. Reporting the farther
//! pin would name a version that is not actually in effect.

use std::path::{Path, PathBuf};

/// How far up the tree to look before giving up. A pin belongs to a repository,
/// so a pin tens of levels above the workspace is far likelier to be someone
/// else's than ours.
const MAX_ASCENT: usize = 32;

/// A language whose toolchain kenn provisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Language {
    Dotnet,
    Rust,
    Go,
    Python,
    Node,
    Swift,
    /// Has no external toolchain at all: the kenn-ts binary embeds its runtime
    /// and TypeScript compiler. Present so every image can run the same
    /// entrypoint, and so adding a TS toolchain later needs no new plumbing.
    TypeScript,
}

impl Language {
    /// The cache directory name for this language.
    #[must_use]
    pub fn key(self) -> &'static str {
        match self {
            Language::Dotnet => "dotnet",
            Language::Rust => "rust",
            Language::Go => "go",
            Language::Python => "python",
            Language::Node => "node",
            Language::Swift => "swift",
            Language::TypeScript => "typescript",
        }
    }

    /// The pin files this language may declare a toolchain in, nearest-first
    /// within a directory. Rust has two spellings; the `.toml` form wins when
    /// both are present, matching rustup.
    #[expect(
        clippy::match_same_arms,
        reason = "Node and TypeScript both have no pin file but for different \
                  reasons — one is supplied by us, the other has no toolchain at \
                  all. Merging the arms would erase that distinction from the \
                  place a reader looks for it"
    )]
    fn pin_files(self) -> &'static [&'static str] {
        match self {
            Language::Dotnet => &["global.json"],
            Language::Rust => &["rust-toolchain.toml", "rust-toolchain"],
            Language::Go => &["go.mod"],
            Language::Python => &[".python-version"],
            Language::Swift => &["Package.swift"],
            // scip-python needs a node to run on, but no repository declares one
            // for it. The resolver supplies a default.
            Language::Node => &[],
            // Nothing to pin: the toolchain is inside the indexer binary.
            Language::TypeScript => &[],
        }
    }
}

/// A toolchain declaration read from a workspace file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pin {
    pub language: Language,
    /// Exactly as written — `"9.0.308"`, `"stable"`, `"1.24.5"`, `"3.12"`.
    pub version: String,
    /// The file it was read from, for diagnostics. A failure names this.
    pub source: PathBuf,
    /// .NET's `rollForward`, which changes which concrete SDK satisfies the pin.
    pub roll_forward: Option<String>,
}

/// Find the toolchain `language` pins, starting at `start` and walking up.
///
/// `Ok(None)` means no pin file was found, or the nearest one declared no
/// version — both mean "no pin in effect", and the resolver picks a default.
/// `Err` means a pin file was found but could not be read.
pub fn find_pin(language: Language, start: &Path) -> Result<Option<Pin>, PinError> {
    let mut dir = Some(start);
    for _ in 0..MAX_ASCENT {
        let Some(current) = dir else { break };
        for name in language.pin_files() {
            let path = current.join(name);
            if !path.is_file() {
                continue;
            }
            let text = std::fs::read_to_string(&path).map_err(|source| PinError::Read {
                path: path.clone(),
                source,
            })?;
            // The nearest pin FILE decides, even if it names no version —
            // returning None here rather than continuing the walk is the point.
            return parse(language, &text, &path).map(|version| {
                version.map(|(version, roll_forward)| Pin {
                    language,
                    version,
                    source: path,
                    roll_forward,
                })
            });
        }
        dir = current.parent();
    }
    Ok(None)
}

#[derive(Debug, thiserror::Error)]
pub enum PinError {
    #[error("reading {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {message}")]
    Malformed { path: PathBuf, message: String },
}

/// Parse one pin file's contents. `Ok(None)` = the file declares no version.
fn parse(
    language: Language,
    text: &str,
    path: &Path,
) -> Result<Option<(String, Option<String>)>, PinError> {
    let malformed = |message: String| PinError::Malformed {
        path: path.to_path_buf(),
        message,
    };
    match language {
        Language::Dotnet => parse_global_json(text).map_err(malformed),
        Language::Rust => parse_rust_toolchain(text, path).map_err(malformed),
        Language::Go => Ok(parse_go_mod(text).map(|v| (v, None))),
        Language::Python => Ok(parse_python_version(text).map(|v| (v, None))),
        Language::Swift => Ok(parse_swift_tools_version(text).map(|v| (v, None))),
        Language::Node | Language::TypeScript => Ok(None),
    }
}

fn parse_global_json(text: &str) -> Result<Option<(String, Option<String>)>, String> {
    let doc: serde_json::Value =
        serde_json::from_str(text).map_err(|e| format!("invalid global.json: {e}"))?;
    let Some(sdk) = doc.get("sdk") else {
        // A global.json with only `msbuild-sdks` is legitimate and pins nothing.
        return Ok(None);
    };
    let Some(version) = sdk.get("version").and_then(serde_json::Value::as_str) else {
        return Ok(None);
    };
    let roll_forward = sdk
        .get("rollForward")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    Ok(Some((version.to_string(), roll_forward)))
}

fn parse_rust_toolchain(
    text: &str,
    path: &Path,
) -> Result<Option<(String, Option<String>)>, String> {
    // The legacy `rust-toolchain` file is a bare channel name, not TOML.
    if path.file_name().is_some_and(|n| n == "rust-toolchain") {
        let channel = text.trim();
        return Ok((!channel.is_empty()).then(|| (channel.to_string(), None)));
    }
    let doc: toml::Value =
        toml::from_str(text).map_err(|e| format!("invalid rust-toolchain.toml: {e}"))?;
    Ok(doc
        .get("toolchain")
        .and_then(|t| t.get("channel"))
        .and_then(toml::Value::as_str)
        .map(|c| (c.to_string(), None)))
}

/// `go.mod`'s `toolchain` line wins over its `go` line: `go` is the minimum
/// language version, `toolchain` is the toolchain actually asked for.
fn parse_go_mod(text: &str) -> Option<String> {
    let mut go_line = None;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("toolchain ") {
            // `toolchain go1.24.5` — normalize away the `go` prefix.
            let v = rest.trim();
            return Some(v.strip_prefix("go").unwrap_or(v).to_string());
        }
        if let Some(rest) = line.strip_prefix("go ") {
            go_line = Some(rest.trim().to_string());
        }
    }
    go_line
}

/// `.python-version` may hold several lines (pyenv allows a list); the first
/// non-comment entry is the one that takes effect.
fn parse_python_version(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .find(|l| !l.is_empty() && !l.starts_with('#'))
        .map(str::to_string)
}

/// `// swift-tools-version:5.9` — must be the first line of `Package.swift`.
fn parse_swift_tools_version(text: &str) -> Option<String> {
    let first = text.lines().next()?.trim();
    let rest = first.strip_prefix("//")?.trim();
    let version = rest.strip_prefix("swift-tools-version")?;
    // Both `:5.9` and `: 5.9` occur in the wild.
    Some(
        version
            .trim_start_matches([':', '=', ' '])
            .trim()
            .to_string(),
    )
    .filter(|v| !v.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(dir: &Path, name: &str, text: &str) {
        std::fs::write(dir.join(name), text).expect("write");
    }

    #[test]
    fn global_json_yields_version_and_roll_forward() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "global.json",
            r#"{ "sdk": { "version": "9.0.308", "rollForward": "latestMinor" } }"#,
        );
        let pin = find_pin(Language::Dotnet, tmp.path())
            .expect("read")
            .expect("pinned");
        assert_eq!(pin.version, "9.0.308");
        assert_eq!(pin.roll_forward.as_deref(), Some("latestMinor"));
        assert!(pin.source.ends_with("global.json"));
    }

    /// The nearest pin FILE decides, even when it declares no version — that is
    /// what the .NET SDK resolver does, so reporting the farther pin would name
    /// a version that is not in effect.
    ///
    /// Mutation-checked: continuing the walk past a version-less file makes this
    /// return the outer 9.0.308.
    #[test]
    fn a_nearer_pinless_file_shadows_a_farther_pin() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            root.path(),
            "global.json",
            r#"{ "sdk": { "version": "9.0.308" } }"#,
        );
        let nested = root.path().join("nested");
        std::fs::create_dir(&nested).expect("mkdir");
        write(
            &nested,
            "global.json",
            r#"{ "msbuild-sdks": { "X": "1" } }"#,
        );

        assert_eq!(find_pin(Language::Dotnet, &nested).expect("read"), None);
        assert_eq!(
            find_pin(Language::Dotnet, root.path())
                .expect("read")
                .expect("pinned")
                .version,
            "9.0.308"
        );
    }

    #[test]
    fn the_walk_finds_a_pin_above_the_start_directory() {
        let root = tempfile::tempdir().expect("tempdir");
        write(
            root.path(),
            "global.json",
            r#"{ "sdk": { "version": "8.0.404" } }"#,
        );
        let deep = root.path().join("a/b/c");
        std::fs::create_dir_all(&deep).expect("mkdir");
        assert_eq!(
            find_pin(Language::Dotnet, &deep)
                .expect("read")
                .expect("pinned")
                .version,
            "8.0.404"
        );
    }

    #[test]
    fn no_pin_file_is_not_an_error() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(find_pin(Language::Dotnet, tmp.path()).expect("read"), None);
    }

    #[test]
    fn malformed_global_json_is_an_error_not_a_silent_none() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "global.json", "{ this is not json");
        let err = find_pin(Language::Dotnet, tmp.path()).expect_err("must not be silently ignored");
        assert!(matches!(err, PinError::Malformed { .. }), "{err}");
    }

    #[test]
    fn rust_toolchain_toml_and_the_legacy_bare_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.97.1\"\ncomponents = [\"rust-src\"]\n",
        );
        assert_eq!(
            find_pin(Language::Rust, tmp.path())
                .expect("read")
                .expect("pinned")
                .version,
            "1.97.1"
        );

        let legacy = tempfile::tempdir().expect("tempdir");
        write(legacy.path(), "rust-toolchain", "nightly-2026-07-16\n");
        assert_eq!(
            find_pin(Language::Rust, legacy.path())
                .expect("read")
                .expect("pinned")
                .version,
            "nightly-2026-07-16"
        );
    }

    /// `.toml` wins when both spellings exist, matching rustup.
    #[test]
    fn the_toml_spelling_wins_over_the_legacy_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "rust-toolchain", "1.70.0\n");
        write(
            tmp.path(),
            "rust-toolchain.toml",
            "[toolchain]\nchannel = \"1.97.1\"\n",
        );
        assert_eq!(
            find_pin(Language::Rust, tmp.path())
                .expect("read")
                .expect("pinned")
                .version,
            "1.97.1"
        );
    }

    /// `go` is the minimum language version; `toolchain` is the toolchain
    /// actually requested. Preferring `go` would under-provision.
    #[test]
    fn go_mod_prefers_the_toolchain_line_over_the_go_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            "go.mod",
            "module example.com/x\n\ngo 1.21\n\ntoolchain go1.24.5\n\nrequire (\n)\n",
        );
        assert_eq!(
            find_pin(Language::Go, tmp.path())
                .expect("read")
                .expect("pinned")
                .version,
            "1.24.5",
            "the `go` prefix is normalized away"
        );

        // With no toolchain line, the go line is the pin.
        let only_go = tempfile::tempdir().expect("tempdir");
        write(only_go.path(), "go.mod", "module x\n\ngo 1.22.3\n");
        assert_eq!(
            find_pin(Language::Go, only_go.path())
                .expect("read")
                .expect("pinned")
                .version,
            "1.22.3"
        );
    }

    #[test]
    fn python_version_takes_the_first_real_line() {
        let tmp = tempfile::tempdir().expect("tempdir");
        write(
            tmp.path(),
            ".python-version",
            "# a comment\n\n3.12.13\n3.11\n",
        );
        assert_eq!(
            find_pin(Language::Python, tmp.path())
                .expect("read")
                .expect("pinned")
                .version,
            "3.12.13"
        );
    }

    #[test]
    fn swift_tools_version_from_the_first_line() {
        for (text, want) in [
            (
                "// swift-tools-version:5.9\nimport PackageDescription\n",
                "5.9",
            ),
            ("// swift-tools-version: 6.0\n", "6.0"),
            ("//swift-tools-version:5.10\n", "5.10"),
        ] {
            let tmp = tempfile::tempdir().expect("tempdir");
            write(tmp.path(), "Package.swift", text);
            assert_eq!(
                find_pin(Language::Swift, tmp.path())
                    .expect("read")
                    .expect("pinned")
                    .version,
                want,
                "for {text:?}"
            );
        }

        // A Package.swift with no tools-version comment pins nothing.
        let tmp = tempfile::tempdir().expect("tempdir");
        write(tmp.path(), "Package.swift", "import PackageDescription\n");
        assert_eq!(find_pin(Language::Swift, tmp.path()).expect("read"), None);
    }

    #[test]
    fn node_has_no_pin_file() {
        let tmp = tempfile::tempdir().expect("tempdir");
        assert_eq!(find_pin(Language::Node, tmp.path()).expect("read"), None);
    }
}
