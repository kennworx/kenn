//! SCIP-symbol → public-ID transformers (one per language).
//!
//! The public ID is `<lang>:<native-id>` where `<lang>` is the two-letter
//! language prefix and the native ID portion is what a developer in that
//! language would type to refer to the symbol.

use thiserror::Error;

use crate::language::Language;

pub mod css;
pub mod descriptor;
mod go;
pub mod html;
pub mod md;
mod py;
mod rs;
pub mod text;
mod ts;

pub use go::GoTransformer;
pub use py::PythonTransformer;
pub use rs::RustTransformer;
pub use ts::TypeScriptTransformer;

/// A normalized public symbol ID (`cs:Foo.Bar`, `rs:foo::bar`, etc.).
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PublicId(String);

impl PublicId {
    #[must_use]
    pub fn new(language: Language, native: &str) -> Self {
        Self(format!("{}:{}", language.prefix(), native))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }

    #[must_use]
    pub fn into_string(self) -> String {
        self.0
    }
}

impl std::fmt::Display for PublicId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedId {
    pub language: Language,
    pub native: String,
}

#[derive(Debug, Error)]
pub enum IdError {
    #[error("scip symbol does not match the expected `<scheme> <pkg-mgr> <pkg> <ver> <descriptor>` shape: {0}")]
    MalformedScip(String),
    #[error("scip symbol scheme `{0}` does not match transformer for language `{1:?}`")]
    WrongScheme(String, Language),
    #[error("public id is missing a `<lang>:` prefix: {0}")]
    NoPrefix(String),
    #[error("public id has an unknown lang prefix `{0}`")]
    UnknownPrefix(String),
    #[error("public id descriptor is empty after the lang prefix")]
    EmptyNative,
    #[error("scip descriptor parse failed: {0}")]
    BadDescriptor(String),
}

pub trait IdTransformer {
    fn language(&self) -> Language;

    /// Convert a verbatim SCIP symbol string into a normalized public ID.
    fn scip_to_public(&self, scip_symbol: &str) -> Result<PublicId, IdError>;

    /// Parse a public ID back into language + native portion.
    fn parse_public(&self, id: &str) -> Result<ParsedId, IdError> {
        parse_public_generic(id)
    }
}

/// Lang-agnostic public-ID parse: split on the first `:`. The native portion
/// is opaque — language-specific structural parsing is the caller's choice.
pub fn parse_public_generic(id: &str) -> Result<ParsedId, IdError> {
    let (prefix, native) = id
        .split_once(':')
        .ok_or_else(|| IdError::NoPrefix(id.to_string()))?;
    let language =
        Language::from_prefix(prefix).ok_or_else(|| IdError::UnknownPrefix(prefix.to_string()))?;
    if native.is_empty() {
        return Err(IdError::EmptyNative);
    }
    Ok(ParsedId {
        language,
        native: native.to_string(),
    })
}

/// SCIP symbol head split: `<scheme> <manager> <package> <version> <descriptor>`.
/// Whitespace inside the descriptor is preserved (descriptors don't contain
/// spaces by SCIP grammar, so `split_whitespace` with a 4-cap is safe).
pub(crate) fn split_scip_head(scip: &str) -> Result<ScipHead<'_>, IdError> {
    // SCIP symbol grammar: scheme SP manager SP name SP version SP descriptor.
    // Use splitn(5, ' ') to keep descriptor intact; raise an error otherwise.
    let mut parts = scip.splitn(5, ' ');
    let scheme = parts
        .next()
        .ok_or_else(|| IdError::MalformedScip(scip.into()))?;
    let manager = parts
        .next()
        .ok_or_else(|| IdError::MalformedScip(scip.into()))?;
    let package = parts
        .next()
        .ok_or_else(|| IdError::MalformedScip(scip.into()))?;
    let version = parts
        .next()
        .ok_or_else(|| IdError::MalformedScip(scip.into()))?;
    let descriptor = parts
        .next()
        .ok_or_else(|| IdError::MalformedScip(scip.into()))?;
    Ok(ScipHead {
        scheme,
        manager,
        package,
        version,
        descriptor,
    })
}

#[expect(
    dead_code,
    reason = "scheme/manager/package/version are part of the parsed view; only `descriptor` is consumed today"
)]
pub(crate) struct ScipHead<'a> {
    pub scheme: &'a str,
    pub manager: &'a str,
    pub package: &'a str,
    pub version: &'a str,
    pub descriptor: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_public_round_trip_all_langs() {
        for (lang, native) in [
            (Language::Csharp, "Models.Order.Foo(string)"),
            (Language::TypeScript, "@acme/frontend-shared/api.AppError"),
            (Language::Rust, "quinn_proto::connection::Connection::new"),
            (Language::Go, "quinn-proto/connection.Connection.New"),
            (Language::Python, "click.core.Context.invoke"),
        ] {
            let id = PublicId::new(lang, native);
            let parsed = parse_public_generic(id.as_str()).unwrap();
            assert_eq!(parsed.language, lang);
            assert_eq!(parsed.native, native);
        }
    }

    #[test]
    fn parse_public_rejects_unknown_prefix() {
        assert!(matches!(
            parse_public_generic("java:foo.Bar"),
            Err(IdError::UnknownPrefix(_))
        ));
    }

    #[test]
    fn parse_public_rejects_no_prefix() {
        assert!(matches!(
            parse_public_generic("Foo.Bar"),
            Err(IdError::NoPrefix(_))
        ));
    }

    #[test]
    fn parse_public_rejects_empty_native() {
        assert!(matches!(
            parse_public_generic("rs:"),
            Err(IdError::EmptyNative)
        ));
    }
}
