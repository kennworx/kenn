use serde::{Deserialize, Serialize};

/// A project / build file matcher for [`Language::project_files`]. The
/// indexer's file watcher matches either by extension (without leading
/// dot, e.g. `csproj` → `MyApp.csproj`) or by full filename (e.g.
/// `Cargo.toml`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProjectFile {
    /// Match any file whose extension equals this string (no leading dot).
    Extension(&'static str),
    /// Match any file whose full filename equals this string.
    Filename(&'static str),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Language {
    Csharp,
    TypeScript,
    Rust,
    Go,
    Python,
    Markdown,
    Css,
    Sass,
    Html,
    Swift,
    /// The generic text-fallback producer: user-configured non-semantic text
    /// files (yaml/json/txt/…) chunked into searchable nodes. Not a real
    /// language — it owns its own short-id partition so its ids never collide
    /// with a real producer's, and it claims no fixed extensions (it is driven
    /// entirely by configured include globs).
    Text,
}

impl Language {
    /// Two-letter prefix used in public IDs (`cs:`, `ts:`, `rs:`, `go:`, `py:`).
    #[must_use]
    pub const fn prefix(self) -> &'static str {
        match self {
            Self::Csharp => "cs",
            Self::TypeScript => "ts",
            Self::Rust => "rs",
            Self::Go => "go",
            Self::Python => "py",
            Self::Markdown => "md",
            Self::Css => "css",
            Self::Sass => "sass",
            Self::Html => "html",
            Self::Swift => "sw",
            Self::Text => "text",
        }
    }

    /// Source-file extensions associated with this language. Used by
    /// the MCP file-watcher to decide whether a filesystem event might
    /// have changed code the indexer cares about. Returned without the
    /// leading dot. Source code only — project / build files live on
    /// [`Self::project_files`].
    #[must_use]
    pub const fn extensions(self) -> &'static [&'static str] {
        match self {
            Self::Csharp => &["cs"],
            // `.ts`/`.tsx` are the common cases; `.mts`/`.cts` cover ES
            // module / CommonJS variants.
            Self::TypeScript => &["ts", "tsx", "mts", "cts"],
            Self::Rust => &["rs"],
            Self::Go => &["go"],
            Self::Python => &["py", "pyi"],
            Self::Markdown => &["md", "markdown"],
            // Plain CSS only; Sass owns `.scss`/`.sass`.
            Self::Css => &["css"],
            // One language, two syntaxes: SCSS (`.scss`) and indented (`.sass`).
            Self::Sass => &["scss", "sass"],
            // Classic `.html` plus the legacy short `.htm`.
            Self::Html => &["html", "htm"],
            Self::Swift => &["swift"],
            // Glob-driven, not extension-scoped: the text-fallback producer
            // owns no fixed extensions (the user's include globs decide).
            Self::Text => &[],
        }
    }

    /// Project / dependency files associated with this language. A
    /// change to one of these restructures the symbol graph (added or
    /// removed project, changed dependency, retargeted build) and
    /// warrants a reindex trigger. Returned as
    /// [`ProjectFile`] so each entry carries whether the indexer
    /// matches it by extension (e.g. `*.csproj`) or by full filename
    /// (e.g. `Cargo.toml`).
    #[must_use]
    pub const fn project_files(self) -> &'static [ProjectFile] {
        match self {
            Self::Csharp => &[
                ProjectFile::Extension("csproj"),
                ProjectFile::Extension("sln"),
            ],
            Self::TypeScript => &[
                ProjectFile::Filename("tsconfig.json"),
                ProjectFile::Filename("package.json"),
            ],
            Self::Rust => &[ProjectFile::Filename("Cargo.toml")],
            Self::Go => &[
                ProjectFile::Filename("go.mod"),
                ProjectFile::Filename("go.sum"),
            ],
            Self::Python => &[
                ProjectFile::Filename("pyproject.toml"),
                ProjectFile::Filename("requirements.txt"),
            ],
            Self::Swift => &[ProjectFile::Filename("Package.swift")],
            // Markdown, stylesheets (css/sass), HTML, and the text fallback have
            // no project/build files; a change to one restructures nothing the
            // way a manifest does.
            Self::Markdown | Self::Css | Self::Sass | Self::Html | Self::Text => &[],
        }
    }

    /// Long-form name stored in `symbols.language` and `files.language`.
    #[must_use]
    pub const fn db_name(self) -> &'static str {
        match self {
            Self::Csharp => "csharp",
            Self::TypeScript => "typescript",
            Self::Rust => "rust",
            Self::Go => "go",
            Self::Python => "python",
            Self::Markdown => "markdown",
            Self::Css => "css",
            Self::Sass => "sass",
            Self::Html => "html",
            Self::Swift => "swift",
            Self::Text => "text",
        }
    }

    #[must_use]
    pub fn from_prefix(prefix: &str) -> Option<Self> {
        Some(match prefix {
            "cs" => Self::Csharp,
            "ts" => Self::TypeScript,
            "rs" => Self::Rust,
            "go" => Self::Go,
            "py" => Self::Python,
            "md" => Self::Markdown,
            "css" => Self::Css,
            "sass" => Self::Sass,
            "html" => Self::Html,
            "sw" => Self::Swift,
            "text" => Self::Text,
            _ => return None,
        })
    }

    /// Inverse of [`db_name`](Self::db_name).
    #[must_use]
    pub fn from_db_name(name: &str) -> Option<Self> {
        Some(match name {
            "csharp" => Self::Csharp,
            "typescript" => Self::TypeScript,
            "rust" => Self::Rust,
            "go" => Self::Go,
            "python" => Self::Python,
            "markdown" => Self::Markdown,
            "css" => Self::Css,
            "sass" => Self::Sass,
            "html" => Self::Html,
            "swift" => Self::Swift,
            "text" => Self::Text,
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::Language;

    #[test]
    fn prefix_round_trip() {
        for lang in [
            Language::Csharp,
            Language::TypeScript,
            Language::Rust,
            Language::Go,
            Language::Python,
            Language::Markdown,
            Language::Css,
            Language::Sass,
            Language::Html,
            Language::Swift,
            Language::Text,
        ] {
            assert_eq!(Language::from_prefix(lang.prefix()), Some(lang));
        }
    }

    #[test]
    fn unknown_prefix_is_none() {
        assert_eq!(Language::from_prefix("java"), None);
        assert_eq!(Language::from_prefix(""), None);
    }

    #[test]
    fn html_prefix_and_db_name_round_trip() {
        assert_eq!(Language::Html.prefix(), "html");
        assert_eq!(Language::from_prefix("html"), Some(Language::Html));
        assert_eq!(Language::Html.db_name(), "html");
        assert_eq!(Language::from_db_name("html"), Some(Language::Html));
    }

    #[test]
    fn swift_prefix_and_db_name_round_trip() {
        assert_eq!(Language::Swift.prefix(), "sw");
        assert_eq!(Language::from_prefix("sw"), Some(Language::Swift));
        assert_eq!(Language::Swift.db_name(), "swift");
        assert_eq!(Language::from_db_name("swift"), Some(Language::Swift));
        assert!(Language::Swift.extensions().contains(&"swift"));
        assert!(matches!(
            Language::Swift.project_files(),
            [super::ProjectFile::Filename("Package.swift")]
        ));
    }

    #[test]
    fn text_prefix_and_db_name_round_trip_with_no_extensions() {
        assert_eq!(Language::Text.prefix(), "text");
        assert_eq!(Language::from_prefix("text"), Some(Language::Text));
        assert_eq!(Language::Text.db_name(), "text");
        assert_eq!(Language::from_db_name("text"), Some(Language::Text));
        // The text fallback is glob-driven: it deliberately owns no fixed
        // extensions or project files (so it never appears in the
        // `extensions_cover_every_variant` list below).
        assert!(Language::Text.extensions().is_empty());
        assert!(Language::Text.project_files().is_empty());
    }

    #[test]
    fn both_html_extensions_map_to_html() {
        for ext in ["html", "htm"] {
            assert!(
                Language::Html.extensions().contains(&ext),
                "{ext} should be an Html extension"
            );
        }
    }

    #[test]
    fn extensions_cover_every_variant() {
        for lang in [
            Language::Csharp,
            Language::TypeScript,
            Language::Rust,
            Language::Go,
            Language::Python,
            Language::Markdown,
            Language::Css,
            Language::Sass,
            Language::Html,
            Language::Swift,
        ] {
            assert!(!lang.extensions().is_empty(), "{lang:?} has no extensions");
            for ext in lang.extensions() {
                assert!(!ext.starts_with('.'), "leading dot in {ext:?}");
            }
        }
    }

    #[test]
    fn project_files_have_no_leading_dot() {
        for lang in [
            Language::Csharp,
            Language::TypeScript,
            Language::Rust,
            Language::Go,
            Language::Python,
            Language::Markdown,
            Language::Css,
            Language::Sass,
            Language::Html,
            Language::Swift,
        ] {
            for pf in lang.project_files() {
                let s = match pf {
                    super::ProjectFile::Extension(s) | super::ProjectFile::Filename(s) => *s,
                };
                assert!(!s.starts_with('.'), "leading dot in {s:?}");
            }
        }
    }
}
