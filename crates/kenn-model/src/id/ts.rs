//! TypeScript (scip-typescript) ID transformer.
//!
//! Input: `scip-typescript npm <package> <version> <descriptor>` where the
//! descriptor uses `/` for module path segments and `.` for terms inside the
//! module.
//!
//! Output: `ts:<package>/<file-without-ext>.Symbol`. Module is bound to file
//! by language semantics; rename = ID change (documented limitation).

use super::descriptor::{parse_descriptor, Segment};
use super::{split_scip_head, IdError, IdTransformer, ParsedId, PublicId};
use crate::language::Language;

const SCHEME: &str = "scip-typescript";
const MANAGER: &str = "npm";

#[derive(Debug, Default, Clone, Copy)]
pub struct TypeScriptTransformer;

impl IdTransformer for TypeScriptTransformer {
    fn language(&self) -> Language {
        Language::TypeScript
    }

    fn scip_to_public(&self, scip_symbol: &str) -> Result<PublicId, IdError> {
        let head = split_scip_head(scip_symbol)?;
        if head.scheme != SCHEME {
            return Err(IdError::WrongScheme(
                head.scheme.into(),
                Language::TypeScript,
            ));
        }
        if head.manager != MANAGER {
            return Err(IdError::MalformedScip(scip_symbol.into()));
        }
        let segs = parse_descriptor(head.descriptor)?;
        let mut native = String::with_capacity(head.package.len() + head.descriptor.len());
        native.push_str(head.package);
        emit_ts(&segs, &mut native);
        Ok(PublicId::new(Language::TypeScript, &native))
    }

    fn parse_public(&self, id: &str) -> Result<ParsedId, IdError> {
        let parsed = super::parse_public_generic(id)?;
        if parsed.language != Language::TypeScript {
            return Err(IdError::WrongScheme(
                parsed.language.prefix().into(),
                Language::TypeScript,
            ));
        }
        Ok(parsed)
    }
}

fn emit_ts(segs: &[Segment<'_>], out: &mut String) {
    let mut last_was_path = true;
    for seg in segs {
        match seg {
            Segment::Namespace(n) => {
                out.push('/');
                out.push_str(n);
                last_was_path = true;
            }
            Segment::Type(n) | Segment::Term(n) => {
                if last_was_path {
                    out.push('/');
                } else {
                    out.push('.');
                }
                out.push_str(n);
                last_was_path = false;
            }
            Segment::Method { name, .. } => {
                if last_was_path {
                    out.push('/');
                } else {
                    out.push('.');
                }
                out.push_str(name);
                last_was_path = false;
            }
            Segment::TypeParam(_)
            | Segment::Parameter(_)
            | Segment::Meta(_)
            | Segment::Macro(_) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_and_module_path() {
        let scip = "scip-typescript npm @acme/frontend-shared 1.0.0 src/api/`AppError`#";
        let id = TypeScriptTransformer.scip_to_public(scip).unwrap();
        assert_eq!(id.as_str(), "ts:@acme/frontend-shared/src/api/AppError");
    }

    #[test]
    fn term_under_module() {
        let scip = "scip-typescript npm pkg 0.1.0 src/index/foo.";
        let id = TypeScriptTransformer.scip_to_public(scip).unwrap();
        assert_eq!(id.as_str(), "ts:pkg/src/index/foo");
    }

    #[test]
    fn rejects_wrong_scheme() {
        let scip = "scip-go gomod A 0.1.0 Foo#Bar().";
        assert!(matches!(
            TypeScriptTransformer.scip_to_public(scip),
            Err(IdError::WrongScheme(_, _))
        ));
    }
}
