//! Go (scip-go) ID transformer.
//!
//! Input: `gomod <package_path> <version> <descriptor>` where descriptor uses
//! `/` for package segments, `#` for types, `.` for terms.
//!
//! Output: `go:package_path.Symbol` or `go:package_path.Type.Method`.

use super::descriptor::{parse_descriptor, Segment};
use super::{split_scip_head, IdError, IdTransformer, ParsedId, PublicId};
use crate::language::Language;

const SCHEME: &str = "scip-go";
const MANAGER: &str = "gomod";

#[derive(Debug, Default, Clone, Copy)]
pub struct GoTransformer;

impl IdTransformer for GoTransformer {
    fn language(&self) -> Language {
        Language::Go
    }

    fn scip_to_public(&self, scip_symbol: &str) -> Result<PublicId, IdError> {
        let head = split_scip_head(scip_symbol)?;
        if head.scheme != SCHEME {
            return Err(IdError::WrongScheme(head.scheme.into(), Language::Go));
        }
        if head.manager != MANAGER {
            return Err(IdError::MalformedScip(scip_symbol.into()));
        }
        // The public id is built from the DESCRIPTOR segments alone. scip-go
        // composes every symbol's first `Namespace` descriptor from
        // `obj.Pkg().Path()` — the full package import path — while the
        // `Package.Name` head field carries the *module* path. The two differ
        // (a sub-package's path extends the module; a stdlib package like
        // `context` has no module prefix at all), so `head.package` MUST NOT be
        // prepended — doing so duplicates the package path for first-party
        // symbols and emits the wrong package when module != package. The
        // module/version is metadata, not part of the id.
        let segs = parse_descriptor(head.descriptor)?;
        let mut native = String::with_capacity(head.descriptor.len());
        for seg in &segs {
            match seg {
                Segment::Namespace(n) => {
                    if !native.is_empty() {
                        native.push('/');
                    }
                    native.push_str(n);
                }
                Segment::Type(n) | Segment::Term(n) => {
                    native.push('.');
                    native.push_str(n);
                }
                Segment::Method { name, .. } => {
                    native.push('.');
                    native.push_str(name);
                }
                Segment::TypeParam(_)
                | Segment::Parameter(_)
                | Segment::Meta(_)
                | Segment::Macro(_) => {}
            }
        }
        Ok(PublicId::new(Language::Go, &native))
    }

    fn parse_public(&self, id: &str) -> Result<ParsedId, IdError> {
        let parsed = super::parse_public_generic(id)?;
        if parsed.language != Language::Go {
            return Err(IdError::WrongScheme(
                parsed.language.prefix().into(),
                Language::Go,
            ));
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Map a real scip-go symbol string to its expected public id.
    fn go(scip: &str) -> String {
        GoTransformer.scip_to_public(scip).unwrap().as_str().into()
    }

    /// Ground-truth cases lifted verbatim from scip-go's own
    /// `internal/symbols/composer_test.go`. scip-go emits the FULL package
    /// path as the first (backtick-escaped) `Namespace` descriptor; the
    /// `Package.Name` head field is the *module* path. The public id derives
    /// from the descriptor, never the module field.
    #[test]
    fn composer_ground_truth() {
        assert_eq!(
            go("scip-go gomod example.com/lib v1.0.0 `example.com/lib`/MyStruct#"),
            "go:example.com/lib.MyStruct"
        );
        assert_eq!(
            go("scip-go gomod example.com/lib v1.0.0 `example.com/lib`/MaxRetries."),
            "go:example.com/lib.MaxRetries"
        );
        assert_eq!(
            go("scip-go gomod example.com/lib v1.0.0 `example.com/lib`/DoWork()."),
            "go:example.com/lib.DoWork"
        );
        assert_eq!(
            go("scip-go gomod example.com/lib v1.0.0 `example.com/lib`/Server#Start()."),
            "go:example.com/lib.Server.Start"
        );
        assert_eq!(
            go("scip-go gomod example.com/lib v1.0.0 `example.com/lib`/Config#Name."),
            "go:example.com/lib.Config.Name"
        );
    }

    /// `composer_test.go:154` — the module field (`example.com/project`) and the
    /// symbol's package (`example.com/lib`) differ. The id MUST follow the
    /// descriptor's package, ignoring the module entirely. This is the case
    /// the old `head.package`-prepending implementation got wrong.
    #[test]
    fn module_differs_from_package() {
        assert_eq!(
            go("scip-go gomod example.com/project 1.0.0 `example.com/lib`/Version."),
            "go:example.com/lib.Version"
        );
    }

    /// Real first-party symbol from `github.com/sourcegraph/conc` (scip-go
    /// 0.2.4). The sub-package path is the full namespace; prepending the
    /// module would have duplicated it.
    #[test]
    fn real_first_party_subpackage_method() {
        assert_eq!(
            go("scip-go gomod github.com/sourcegraph/conc 5f936abd7ae8 \
                 `github.com/sourcegraph/conc/pool`/Pool#New()."),
            "go:github.com/sourcegraph/conc/pool.Pool.New"
        );
    }

    /// Real stdlib symbol: the descriptor namespace is the short import path
    /// (`context`, no backticks needed), and the module field is the Go source
    /// tree. Dropping the module yields the natural `go:context.Context.Done`.
    #[test]
    fn real_stdlib_method() {
        assert_eq!(
            go("scip-go gomod github.com/golang/go/src go1.20 context/Context#Done()."),
            "go:context.Context.Done"
        );
    }

    /// Real corpus from indexing `github.com/sourcegraph/conc` @5f936ab with
    /// scip-go 0.2.4 — package funcs, types, methods, and struct fields across
    /// sub-packages. Every id must carry the package path exactly once (the
    /// duplicated-package regression this fix removes).
    #[test]
    fn real_conc_corpus() {
        let m = "github.com/sourcegraph/conc";
        let cases = [
            // package-level generic funcs (iter sub-package)
            ("`github.com/sourcegraph/conc/iter`/Map().", "iter.Map"),
            (
                "`github.com/sourcegraph/conc/iter`/ForEach().",
                "iter.ForEach",
            ),
            // type
            (
                "`github.com/sourcegraph/conc/panics`/Catcher#",
                "panics.Catcher",
            ),
            // method on type
            (
                "`github.com/sourcegraph/conc/panics`/Catcher#Recovered().",
                "panics.Catcher.Recovered",
            ),
            // unexported struct field (Term)
            (
                "`github.com/sourcegraph/conc/panics`/Catcher#recovered.",
                "panics.Catcher.recovered",
            ),
            // exported struct field on another type
            (
                "`github.com/sourcegraph/conc/panics`/Recovered#Callers.",
                "panics.Recovered.Callers",
            ),
        ];
        for (descriptor, want_tail) in cases {
            let scip = format!("scip-go gomod {m} 5f936abd7ae8 {descriptor}");
            let want = format!("go:github.com/sourcegraph/conc/{want_tail}");
            assert_eq!(go(&scip), want, "for {descriptor}");
        }
    }
}
