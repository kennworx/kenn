//! Transform error type + language detection: SCIP language strings and
//! file extensions to `kenn_model::Language`, and the per-language
//! `IdTransformer` dispatcher.

use kenn_model::id::{
    GoTransformer, IdTransformer, PythonTransformer, RustTransformer, TypeScriptTransformer,
};
use kenn_model::Language;

#[derive(Debug, thiserror::Error)]
pub enum TransformError {
    #[error("canonicalize: {0}")]
    Canonicalize(#[from] crate::canonicalize::CanonicalizeError),
    #[error("id transform: {0}")]
    Id(#[from] kenn_model::id::IdError),
    #[error("unsupported scip document.language `{0}`")]
    UnknownLanguage(String),
}

/// Owned dispatcher for the per-language `IdTransformer` impls.
///
/// C# is intentionally absent: it goes through the JSONL ingest path
/// (`kenn-dotnet` produces `key`-bearing `SymbolFrame`s consumed by
/// `transform_jsonl`) and never enters the SCIP transformer chain.
#[must_use]
pub fn transformer_for(language: Language) -> Option<Box<dyn IdTransformer>> {
    Some(match language {
        Language::TypeScript => Box::new(TypeScriptTransformer),
        Language::Rust => Box::new(RustTransformer),
        Language::Go => Box::new(GoTransformer),
        Language::Python => Box::new(PythonTransformer),
        // C# and Swift go through the JSONL ingest path; markdown, stylesheets
        // (css/sass), HTML, and the text fallback are walked by their own
        // producers — none enters the SCIP transformer chain.
        Language::Csharp
        | Language::Swift
        | Language::Markdown
        | Language::Css
        | Language::Sass
        | Language::Html
        | Language::Text => return None,
    })
}

#[must_use]
pub fn language_from_scip(name: &str) -> Option<Language> {
    Some(match name {
        "csharp" | "c_sharp" | "C#" | "c#" => Language::Csharp,
        "typescript" | "TypeScript" | "tsx" => Language::TypeScript,
        "rust" | "Rust" => Language::Rust,
        "go" | "Go" => Language::Go,
        "python" | "Python" => Language::Python,
        "swift" | "Swift" => Language::Swift,
        _ => return None,
    })
}

/// Fallback language inference from a `Document.relative_path` extension.
/// Used when an indexer (notably scip-typescript 0.4.0) leaves
/// `Document.language` empty.
#[must_use]
pub fn language_from_path(relative_path: &str) -> Option<Language> {
    // `rfind('.')` returns a byte index that lands on the `.` (an ASCII
    // byte), so `dot + 1..` is always a valid UTF-8 boundary.
    #[expect(
        clippy::string_slice,
        reason = "dot from rfind('.') is ASCII; dot+1 is on a char boundary"
    )]
    let ext = {
        let dot = relative_path.rfind('.')?;
        &relative_path[dot + 1..]
    };
    Some(match ext {
        "ts" | "tsx" | "js" | "jsx" | "mjs" | "cjs" => Language::TypeScript,
        "rs" => Language::Rust,
        "go" => Language::Go,
        "py" | "pyi" => Language::Python,
        "swift" => Language::Swift,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn language_from_path_handles_common_extensions() {
        assert_eq!(language_from_path("src/foo.ts"), Some(Language::TypeScript));
        assert_eq!(
            language_from_path("src/foo.tsx"),
            Some(Language::TypeScript)
        );
        assert_eq!(language_from_path("src/foo.rs"), Some(Language::Rust));
        assert_eq!(language_from_path("src/foo.go"), Some(Language::Go));
        assert_eq!(language_from_path("src/foo.py"), Some(Language::Python));
        assert_eq!(
            language_from_path("Sources/App/Foo.swift"),
            Some(Language::Swift)
        );
        assert_eq!(language_from_path("src/no_extension"), None);
        // .cs intentionally NOT inferred — kenn-dotnet handles C# via JSONL.
        assert_eq!(language_from_path("Foo.cs"), None);
    }

    /// `transformer_for` returns a `Some` per supported indexer
    /// language and explicitly `None` for C# (which goes through the
    /// JSONL path, not SCIP). Every variant must be exercised.
    #[test]
    fn transformer_for_covers_every_language() {
        assert!(transformer_for(Language::TypeScript).is_some());
        assert!(transformer_for(Language::Rust).is_some());
        assert!(transformer_for(Language::Go).is_some());
        assert!(transformer_for(Language::Python).is_some());
        // C# and Swift have no SCIP transformer (JSONL path).
        assert!(transformer_for(Language::Csharp).is_none());
        assert!(transformer_for(Language::Swift).is_none());
    }
}
