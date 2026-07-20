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
    fn rejects_wrong_scheme() {
        let scip = "scip-go gomod A 0.1.0 Foo#Bar().";
        assert!(matches!(
            RustTransformer.scip_to_public(scip),
            Err(IdError::WrongScheme(_, _))
        ));
    }
}
