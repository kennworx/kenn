//! [`CodeGraphNodeResolver`] — resolves code-graph node ids against the
//! code graph.
//!
//! A code-node id has the form `<language>:<pub_id>`. A per-call probe of the
//! code graph would be the per-item-query trap (design D3). Instead the
//! resolver is built from a single bulk scan of the `symbols` table — the full
//! set of code-node ids — and `contains` is then an in-memory O(1) lookup.

use std::collections::HashSet;

use super::lifecycle::CodeNodeResolver;

/// A [`CodeNodeResolver`] backed by the snapshot's full set of
/// code-node ids, materialized once at construction.
pub struct CodeGraphNodeResolver {
    /// Every `<language>:<pub_id>` present in the code graph.
    ids: HashSet<String>,
}

impl CodeGraphNodeResolver {
    /// Build a resolver over a pre-scanned code-node id set — see
    /// `GraphReader::code_node_ids`.
    #[must_use]
    pub fn new(ids: HashSet<String>) -> Self {
        Self { ids }
    }
}

impl CodeNodeResolver for CodeGraphNodeResolver {
    fn contains(&self, code_node_id: &str) -> bool {
        self.ids.contains(code_node_id)
    }
}
