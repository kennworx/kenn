//! Python (scip-python) ID transformer.
//!
//! Input: `scip-python python <distribution> <version> <descriptor>` where
//! descriptor uses `/` for module path, `#` for classes, `.` for terms.
//!
//! Output: `py:module.Class.method`. Distribution is metadata, not in the ID.

use super::descriptor::{parse_descriptor, Segment};
use super::{split_scip_head, IdError, IdTransformer, ParsedId, PublicId};
use crate::language::Language;

const SCHEME: &str = "scip-python";
const MANAGER: &str = "python";

#[derive(Debug, Default, Clone, Copy)]
pub struct PythonTransformer;

impl IdTransformer for PythonTransformer {
    fn language(&self) -> Language {
        Language::Python
    }

    fn scip_to_public(&self, scip_symbol: &str) -> Result<PublicId, IdError> {
        let head = split_scip_head(scip_symbol)?;
        if head.scheme != SCHEME {
            return Err(IdError::WrongScheme(head.scheme.into(), Language::Python));
        }
        if head.manager != MANAGER {
            return Err(IdError::MalformedScip(scip_symbol.into()));
        }
        let segs = parse_descriptor(head.descriptor)?;
        let mut parts: Vec<&str> = Vec::new();
        for seg in &segs {
            match seg {
                Segment::Namespace(n) | Segment::Type(n) | Segment::Term(n) => parts.push(n),
                Segment::Method { name, .. } => parts.push(name),
                Segment::TypeParam(_)
                | Segment::Parameter(_)
                | Segment::Meta(_)
                | Segment::Macro(_) => {}
            }
        }
        Ok(PublicId::new(Language::Python, &parts.join(".")))
    }

    fn parse_public(&self, id: &str) -> Result<ParsedId, IdError> {
        let parsed = super::parse_public_generic(id)?;
        if parsed.language != Language::Python {
            return Err(IdError::WrongScheme(
                parsed.language.prefix().into(),
                Language::Python,
            ));
        }
        Ok(parsed)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn module_class_method() {
        let scip = "scip-python python click 8.1.0 click/core/Context#invoke().";
        let id = PythonTransformer.scip_to_public(scip).unwrap();
        assert_eq!(id.as_str(), "py:click.core.Context.invoke");
    }

    #[test]
    fn rejects_wrong_scheme() {
        let scip = "scip-go gomod foo 0.1.0 Bar.";
        assert!(matches!(
            PythonTransformer.scip_to_public(scip),
            Err(IdError::WrongScheme(_, _))
        ));
    }
}
