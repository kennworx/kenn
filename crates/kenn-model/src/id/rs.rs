//! Rust (rust-analyzer) ID transformer.
//!
//! Input: `rust-analyzer cargo <crate> <version> <descriptor>`. Descriptor
//! uses `/` for module path, `#` for types, `.` for terms, `!` for macros.
//! rust-analyzer additionally encodes impl blocks structurally as
//! `impl#[Type][Trait]` — those are flattened into the impl-block's parent
//! item ID.
//!
//! Output: `rs:crate::path::to::item`. Generic args are not in the canonical
//! ID (no turbofish); arity lives on `symbols.generic_arity`.

use super::descriptor::{parse_descriptor, Segment};
use super::{split_scip_head, IdError, IdTransformer, ParsedId, PublicId};
use crate::language::Language;

const SCHEME: &str = "rust-analyzer";
const MANAGER: &str = "cargo";

#[derive(Debug, Default, Clone, Copy)]
pub struct RustTransformer;

impl IdTransformer for RustTransformer {
    fn language(&self) -> Language {
        Language::Rust
    }

    fn scip_to_public(&self, scip_symbol: &str) -> Result<PublicId, IdError> {
        let head = split_scip_head(scip_symbol)?;
        if head.scheme != SCHEME {
            return Err(IdError::WrongScheme(head.scheme.into(), Language::Rust));
        }
        if head.manager != MANAGER {
            return Err(IdError::MalformedScip(scip_symbol.into()));
        }
        let segs = parse_descriptor(head.descriptor)?;
        let native = emit_rust(head.package, &segs);
        Ok(PublicId::new(Language::Rust, &native))
    }

    fn parse_public(&self, id: &str) -> Result<ParsedId, IdError> {
        let parsed = super::parse_public_generic(id)?;
        if parsed.language != Language::Rust {
            return Err(IdError::WrongScheme(
                parsed.language.prefix().into(),
                Language::Rust,
            ));
        }
        Ok(parsed)
    }
}

/// The package a SCIP symbol belongs to: `(name, version)` from the moniker
/// head.
///
/// Correct for every SCIP indexer whose head names the unit of import —
/// rust-analyzer writes the crate, scip-python the distribution. Go is the
/// exception and overrides this ([`go_package_of`]): its head is the MODULE,
/// while the unit of import is the package, which lives in the descriptor.
#[must_use]
pub fn head_package_of(scip_symbol: &str) -> Option<(&str, &str)> {
    let head = split_scip_head(scip_symbol).ok()?;
    (!head.package.is_empty()).then_some((head.package, head.version))
}

/// The `(type, trait)` pair a rust-analyzer **trait**-impl moniker encodes.
/// Both are bare names — rust-analyzer writes `impl#[Type][Trait]`, never the
/// trait's defining crate — so a consumer must resolve them against symbols it
/// has seen elsewhere.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TraitImpl<'a> {
    pub type_name: &'a str,
    pub trait_name: &'a str,
}

/// Strip a SCIP descriptor name down to the identifier used for matching: the
/// backtick quoting the segmenter preserves, and any generic argument list.
/// `` `Index<I>` `` → `Index`, `` `Vec<T, A>` `` → `Vec`, `Try` → `Try`.
///
/// A name that STARTS with `<` is a qualified path (`<Foo as Bar>::Baz`), where
/// splitting yields an empty base. Those keep their full unquoted form: an empty
/// string is not an identifier, and returning it would collapse every qualified
/// path in a document under one map key — making unrelated types collide as if
/// they were the same name.
#[must_use]
pub fn base_type_name(raw: &str) -> &str {
    let unquoted = raw.trim_matches('`');
    match unquoted.split_once('<') {
        Some((base, _)) if !base.is_empty() => base,
        _ => unquoted,
    }
}

/// The trait impl a rust-analyzer symbol belongs to, or `None` when it is not
/// inside one.
///
/// rust-analyzer encodes impl blocks structurally: `impl#[Type]member` for an
/// inherent impl and `impl#[Type][Trait]member` for a trait impl. Only the
/// two-parameter form is a trait impl — this is the ONE place the
/// implements relationship survives for Rust, because rust-analyzer does not
/// populate SCIP `SymbolInformation.relationships` at all (unlike scip-go and
/// scip-python, which do). Detection is structural, never a name heuristic.
///
/// Both names come back bare and un-normalized; callers wanting to match them
/// against other symbols should run them through [`base_type_name`].
#[must_use]
pub fn trait_impl_of(scip_symbol: &str) -> Option<TraitImpl<'_>> {
    let head = split_scip_head(scip_symbol).ok()?;
    if head.scheme != SCHEME || head.manager != MANAGER {
        return None;
    }
    let segs = parse_descriptor(head.descriptor).ok()?;
    let impl_at = segs
        .iter()
        .position(|s| matches!(s, Segment::Type(n) if *n == "impl"))?;
    // Exactly the two type params directly following `impl#`. A third would not
    // be an impl header, and one alone is an inherent impl (no trait).
    match (segs.get(impl_at + 1), segs.get(impl_at + 2)) {
        (Some(Segment::TypeParam(ty)), Some(Segment::TypeParam(tr))) => Some(TraitImpl {
            type_name: ty,
            trait_name: tr,
        }),
        _ => None,
    }
}

/// The name of the type a SCIP symbol *is*, when its descriptor ends in a type
/// segment (`…/Default#`). `None` for methods, terms, and anything else — those
/// are not candidates for resolving an `impl#[…][…]` name against.
///
/// Scheme-guarded like [`trait_impl_of`], so the pair stays symmetric: a
/// `scip-go` symbol is not a candidate for resolving a rust-analyzer impl name,
/// and a future caller that forgets to gate on the language cannot silently
/// match across indexers.
#[must_use]
pub fn terminal_type_name(scip_symbol: &str) -> Option<&str> {
    let head = split_scip_head(scip_symbol).ok()?;
    if head.scheme != SCHEME || head.manager != MANAGER {
        return None;
    }
    let segs = parse_descriptor(head.descriptor).ok()?;
    match segs.last() {
        Some(Segment::Type(n)) => Some(base_type_name(n)),
        _ => None,
    }
}

fn emit_rust(crate_name: &str, segs: &[Segment<'_>]) -> String {
    let mut path: Vec<String> = vec![crate_name.into()];
    let mut consuming_impl = false;
    for seg in segs {
        match seg {
            // rust-analyzer encodes impl blocks structurally as `impl#` followed
            // by `[Type]` and (for trait impls) `[Trait]` type-param segments.
            // Flatten to `crate::mod::Type::Trait::member` (D1 / scip-indexing
            // resolved-risk).
            Segment::Type(n) if *n == "impl" => {
                consuming_impl = true;
            }
            Segment::Namespace(n) | Segment::Type(n) | Segment::Term(n) => {
                consuming_impl = false;
                path.push((*n).into());
            }
            Segment::Method { name, .. } => {
                consuming_impl = false;
                path.push((*name).into());
            }
            Segment::Macro(n) => {
                consuming_impl = false;
                path.push(format!("{n}!"));
            }
            Segment::TypeParam(n) if consuming_impl => {
                path.push((*n).into());
            }
            // Generics on normal items + parameters + meta segments don't
            // contribute to the public ID — generic arity lives on the symbol.
            Segment::TypeParam(_) | Segment::Parameter(_) | Segment::Meta(_) => {}
        }
    }
    path.join("::")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_path_to_function() {
        let scip = "rust-analyzer cargo quinn_proto 0.10.0 connection/Connection#new().";
        let id = RustTransformer.scip_to_public(scip).unwrap();
        assert_eq!(id.as_str(), "rs:quinn_proto::connection::Connection::new");
    }

    #[test]
    fn impl_block_flattens_to_type_trait_path() {
        let scip = "rust-analyzer cargo foo 0.1.0 impl#[MyType][MyTrait]some_method().";
        let id = RustTransformer.scip_to_public(scip).unwrap();
        assert_eq!(id.as_str(), "rs:foo::MyType::MyTrait::some_method");
    }

    #[test]
    fn macro_segment_keeps_bang() {
        let scip = "rust-analyzer cargo foo 0.1.0 println!";
        let id = RustTransformer.scip_to_public(scip).unwrap();
        assert_eq!(id.as_str(), "rs:foo::println!");
    }

    #[test]
    fn trait_impl_is_detected_structurally_not_by_name() {
        let scip =
            "rust-analyzer cargo foo 0.1.0 llama/impl#[LlamaEmbedder][EmbeddingProducer]embed().";
        assert_eq!(
            trait_impl_of(scip),
            Some(TraitImpl {
                type_name: "LlamaEmbedder",
                trait_name: "EmbeddingProducer"
            })
        );
    }

    /// An INHERENT impl carries one type param and no trait. Emitting an edge for
    /// it would invent an implements relationship that does not exist — and
    /// inherent impls outnumber trait impls in real Rust.
    #[test]
    fn inherent_impl_yields_no_trait() {
        let scip = "rust-analyzer cargo foo 0.1.0 store/impl#[Store]open().";
        assert_eq!(trait_impl_of(scip), None);
        // A symbol not inside any impl likewise.
        assert_eq!(
            trait_impl_of("rust-analyzer cargo foo 0.1.0 connection/Connection#new()."),
            None
        );
        // Another indexer's symbol is not a rust-analyzer impl.
        assert_eq!(trait_impl_of("scip-go gomod A 0.1.0 Foo#Bar()."), None);
    }

    #[test]
    fn generic_and_quoted_names_reduce_to_a_matchable_base() {
        // Real monikers from a workspace index: both names backtick-quoted, both
        // generic. Matching against a plain `Index#` symbol needs the base name.
        let scip = "rust-analyzer cargo foo 0.1.0 impl#[`Vec<T, A>`][`Index<I>`]index().";
        let got = trait_impl_of(scip).expect("a trait impl");
        assert_eq!(base_type_name(got.type_name), "Vec");
        assert_eq!(base_type_name(got.trait_name), "Index");
        assert_eq!(base_type_name("Try"), "Try");
    }

    #[test]
    fn terminal_type_name_only_matches_type_descriptors() {
        assert_eq!(
            terminal_type_name("rust-analyzer cargo std 1.0.0 default/Default#"),
            Some("Default")
        );
        // A method is not a resolution candidate for an `impl#[…][…]` name.
        assert_eq!(
            terminal_type_name("rust-analyzer cargo foo 0.1.0 Bar#baz()."),
            None
        );
        // Symmetric with `trait_impl_of`: another indexer's symbol is not a
        // candidate for resolving a rust-analyzer impl name.
        assert_eq!(terminal_type_name("scip-go gomod A 0.1.0 Foo#"), None);
    }

    /// A qualified path (`<Foo as Bar>::Baz`) splits to an EMPTY base. Returning
    /// that would file every qualified path in a document under one map key, so
    /// unrelated types would collide as if they shared a name — either marking
    /// each other ambiguous or resolving an impl to an arbitrary unrelated symbol.
    #[test]
    fn qualified_path_names_do_not_collapse_to_one_key() {
        assert_eq!(base_type_name("`<Foo as Bar>::Baz`"), "<Foo as Bar>::Baz");
        assert_ne!(
            base_type_name("`<A as T>::X`"),
            base_type_name("`<B as U>::Y`"),
            "two unrelated qualified paths must not share a key"
        );
    }

    #[test]
    fn rejects_wrong_scheme() {
        let scip = "scip-go gomod A 0.1.0 Foo#Bar().";
        assert!(matches!(
            RustTransformer.scip_to_public(scip),
            Err(IdError::WrongScheme(_, _))
        ));
    }
}
