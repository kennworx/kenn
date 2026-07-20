//! Text-fallback public-ID construction.
//!
//! Like markdown, text-fallback nodes are not produced from SCIP; their native
//! IDs are built directly from the corpus root label, the file's
//! workspace-relative path, and (for chunks) the chunk index. The public ID is
//! `text:<root>` for the corpus module, `text:<root>/<relpath>` for the file
//! node, and `text:<root>/<relpath>#<index>` for a chunk.

use crate::id::PublicId;
use crate::language::Language;

/// Public ID of the text-fallback corpus module (`module` kind): `text:<root>`.
/// Every file node is a member of this single root module.
#[must_use]
pub fn module_id(root: &str) -> PublicId {
    PublicId::new(Language::Text, root)
}

/// Public ID of a text-fallback file-as-node (`document` kind).
#[must_use]
pub fn document_id(root: &str, relpath: &str) -> PublicId {
    PublicId::new(Language::Text, &format!("{root}/{relpath}"))
}

/// Public ID of one text-fallback chunk (`chunk` kind), disambiguated by its
/// 0-based index within the file.
#[must_use]
pub fn chunk_id(root: &str, relpath: &str, index: usize) -> PublicId {
    PublicId::new(Language::Text, &format!("{root}/{relpath}#{index}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_carry_root_relpath_and_chunk_index() {
        assert_eq!(module_id("workspace").as_str(), "text:workspace");
        assert_eq!(
            document_id("workspace", "config/app.yaml").as_str(),
            "text:workspace/config/app.yaml"
        );
        assert_eq!(
            chunk_id("workspace", "config/app.yaml", 2).as_str(),
            "text:workspace/config/app.yaml#2"
        );
    }
}
