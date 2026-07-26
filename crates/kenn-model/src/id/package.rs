//! Which package a SCIP symbol belongs to, per language.

use super::{go_package_of, head_package_of};
use crate::language::Language;

/// The `(name, version)` of the package a SCIP symbol belongs to, or `None`
/// when the moniker names none.
///
/// The SCIP ingest path wrote `pkg_id: 0` for every symbol it produced, so the
/// `packages` table was empty for rust, go and python alike — leaving
/// `--package` filters silently matching nothing and the atlas falling back to
/// manifest directories to work out what a package even was. The identity was
/// in the moniker the whole time.
///
/// Go is the one language whose head is not the unit of import; see
/// [`go_package_of`].
#[must_use]
pub fn package_of(language: Language, scip_symbol: &str) -> Option<(String, &str)> {
    match language {
        Language::Go => go_package_of(scip_symbol),
        _ => head_package_of(scip_symbol).map(|(n, v)| (n.to_string(), v)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Rust and Python name the unit of import in the moniker HEAD — the crate
    /// and the distribution respectively.
    #[test]
    fn head_naming_languages_take_the_head() {
        assert_eq!(
            package_of(
                Language::Rust,
                "rust-analyzer cargo kenn-embed 0.1.0 llama/LlamaEmbedder#"
            ),
            Some(("kenn-embed".to_string(), "0.1.0"))
        );
        assert_eq!(
            package_of(
                Language::Python,
                "scip-python python httpx b5addb64 `httpx._client`/AsyncClient#"
            ),
            Some(("httpx".to_string(), "b5addb64"))
        );
    }

    /// Go's head is the MODULE, which covers every package in it — taking it
    /// would collapse spf13/afero's eight importable packages into one, the
    /// same mistake manifest-based anchoring makes. The package is the
    /// descriptor's leading namespace, backtick-quoted because it contains `/`.
    /// Mutation-checked: returning `head.package` fails the first assertion.
    #[test]
    fn go_takes_the_package_not_the_module() {
        assert_eq!(
            package_of(
                Language::Go,
                "scip-go gomod github.com/spf13/afero 768f1fb `github.com/spf13/afero/mem`/FileData#"
            ),
            Some(("afero/mem".to_string(), "768f1fb")),
            "the sub-package, module-relative — not the module, not the full path"
        );
        // A symbol in the module's root package.
        assert_eq!(
            package_of(
                Language::Go,
                "scip-go gomod github.com/spf13/afero 768f1fb `github.com/spf13/afero`/Fs#"
            ),
            Some(("afero".to_string(), "768f1fb")),
            "the module's own package is just its leaf"
        );
        // No leading namespace — fall back to the module so a symbol always
        // belongs to something.
        assert_eq!(
            package_of(Language::Go, "scip-go gomod example.com/m v1 Foo#"),
            Some(("m".to_string(), "v1"))
        );
    }

    #[test]
    fn a_malformed_moniker_names_no_package() {
        assert_eq!(package_of(Language::Rust, "not a scip symbol"), None);
    }
}
