//! `IdRegistry`: per-language `short_id` interning for files, symbols, and
//! packages, plus the cross-stream stub buffer that edge derivation relies
//! on.

use std::collections::HashMap;

use kenn_model::{compose_short_id, Language, ShortId, SymbolRecord};

/// Auto-increments short ids for files and symbols within one language
/// partition. Maintains `(language, scip_symbol) -> short_id` so edge
/// derivation can resolve later occurrences to the same id.
///
/// One registry per language ingester: the `language` it is constructed
/// with selects the `short_id` partition (see [`kenn_model::short_id`]),
/// so two ingesters never share interning state and their ids never
/// collide.
#[derive(Debug)]
pub struct IdRegistry {
    /// The language this registry interns for — selects the `short_id`
    /// partition every allocated id lands in.
    language: Language,
    /// Next 1-based per-language counter for file ids.
    next_file_id: ShortId,
    /// Next 1-based per-language counter for symbol ids.
    next_symbol_id: ShortId,
    /// Next 1-based per-language counter for package ids.
    next_package_id: ShortId,
    files: HashMap<String, ShortId>,
    /// SCIP-symbol → `short_id` (used by edge derivation to resolve
    /// occurrences). A single `short_id` may have multiple SCIP-symbol
    /// aliases when an upstream indexer emits the same conceptual symbol
    /// from several units with different package suffixes.
    symbols: HashMap<(Language, String), ShortId>,
    /// public-id → `short_id`. The (language, `pub_id`) tuple is the
    /// identity key per source-data-model D7. Two distinct SCIP strings
    /// that resolve to the same `pub_id` share a `short_id` and emit only
    /// one `SymbolRecord`.
    by_pub_id: HashMap<(Language, String), ShortId>,
    /// Package intern table keyed by `(name, version)`. Shared across
    /// every stream consumed by the registry so that two kenn-dotnet
    /// invocations referencing the same logical package collapse to one
    /// `packages` row (the registry survives across units; per-stream
    /// state does not).
    packages: HashMap<(String, String), ShortId>,
    /// Stub records buffered cross-stream: keyed by symbol short id. A
    /// stub seen in one stream may be upgraded by a full `SymbolFrame` in
    /// a later stream (the symbol's owning project might be in a
    /// different `.sln` than the body that referenced it). Drained at
    /// end-of-job by [`Self::drain_pending_stubs`].
    pending_stub_records: HashMap<ShortId, SymbolRecord>,
    /// Symbols whose full record has already been pushed to the sink.
    /// Used during cross-wire-id dedup to distinguish a real duplicate
    /// (full + full → mark as duplicate) from a cross-stream stub
    /// upgrade (stub + full → emit the full record). Stored on the
    /// shared registry rather than per-stream because the second sighting
    /// can land in a different `.jsonl` stream than the first.
    full_emitted: std::collections::HashSet<ShortId>,
}

impl IdRegistry {
    /// Construct an empty registry for one language partition.
    #[must_use]
    pub fn new(language: Language) -> Self {
        Self {
            language,
            next_file_id: 1,
            next_symbol_id: 1,
            next_package_id: 1,
            files: HashMap::new(),
            symbols: HashMap::new(),
            by_pub_id: HashMap::new(),
            packages: HashMap::new(),
            pending_stub_records: HashMap::new(),
            full_emitted: std::collections::HashSet::new(),
        }
    }

    /// Buffer a stub record under `short_id`. Returns the prior buffered
    /// record (if any) so the caller can decide whether to overwrite —
    /// callers should only insert when the previous value was `None`.
    pub fn buffer_stub(&mut self, short_id: ShortId, rec: SymbolRecord) -> Option<SymbolRecord> {
        self.pending_stub_records.insert(short_id, rec)
    }

    /// Take the buffered stub for `short_id`, if any. Used by the
    /// consumer's cross-stream upgrade path: when a `SymbolFrame` arrives
    /// and the existing row was a stub, the stub is removed from the
    /// buffer (it never reached the sink) and the full record is emitted
    /// in its place.
    pub fn take_pending_stub(&mut self, short_id: ShortId) -> Option<SymbolRecord> {
        self.pending_stub_records.remove(&short_id)
    }

    /// Drain every remaining buffered stub. Stubs that never received a
    /// full `SymbolFrame` upgrade across the whole job — typically
    /// external symbols (standard library, vendored / third-party
    /// packages) — are returned here and the caller pushes them to
    /// the sink as bare rows.
    pub fn drain_pending_stubs(&mut self) -> impl Iterator<Item = SymbolRecord> + '_ {
        self.pending_stub_records.drain().map(|(_, v)| v)
    }

    /// Mark a symbol's full record as having reached the sink. Used by
    /// the cross-wire-id dedup path to distinguish real duplicates (mark
    /// the second wire id as a duplicate, skip its outgoing edges) from
    /// cross-stream stub upgrades (do not mark; emit the full record).
    ///
    /// Also clears any pending stub buffered for this `short_id` — the
    /// real record supersedes it. Without this, `flush_registry_stubs`
    /// would emit the stub at end-of-job and the writer would see the
    /// same `short_id` twice (design D5 violation).
    pub fn mark_full_emitted(&mut self, short_id: ShortId) {
        self.full_emitted.insert(short_id);
        self.pending_stub_records.remove(&short_id);
    }

    #[must_use]
    pub fn was_full_emitted(&self, short_id: ShortId) -> bool {
        self.full_emitted.contains(&short_id)
    }

    /// Look up or assign a `short_id` for a package keyed by
    /// `(name, version)`. Returns `(short_id, is_new)`; callers emit a
    /// `PackageRecord` only when `is_new == true`.
    pub fn intern_package(&mut self, name: &str, version: &str) -> (ShortId, bool) {
        let key = (name.to_string(), version.to_string());
        if let Some(id) = self.packages.get(&key).copied() {
            return (id, false);
        }
        let id = compose_short_id(self.language, self.next_package_id);
        self.next_package_id = self
            .next_package_id
            .checked_add(1)
            .expect("package id partition overflow");
        self.packages.insert(key, id);
        (id, true)
    }

    /// Look up or assign a `short_id` for a workspace-relative file path.
    pub fn intern_file(&mut self, path: &str) -> ShortId {
        self.intern_file_with_seen(path).0
    }

    /// Variant of [`intern_file`] that reports whether this is the first
    /// time the path was seen. Callers that emit `FileRecord`s should use
    /// this and skip emission when `is_new = false` — the file already
    /// has a row for the same `(path, short_id)` pair.
    pub fn intern_file_with_seen(&mut self, path: &str) -> (ShortId, bool) {
        if let Some(id) = self.files.get(path) {
            return (*id, false);
        }
        let id = compose_short_id(self.language, self.next_file_id);
        self.next_file_id = self
            .next_file_id
            .checked_add(1)
            .expect("file id partition overflow");
        self.files.insert(path.to_string(), id);
        (id, true)
    }

    /// Look up or assign a `short_id` for a SCIP symbol string in a language.
    /// Used by edge derivation; does **not** dedupe by `pub_id` (callers
    /// that emit `SymbolRecord`s should use [`intern_with_pub_id`]).
    pub fn intern_symbol(&mut self, language: Language, scip_symbol: &str) -> ShortId {
        let key = (language, scip_symbol.to_string());
        if let Some(id) = self.symbols.get(&key) {
            return *id;
        }
        let id = compose_short_id(self.language, self.next_symbol_id);
        self.next_symbol_id = self
            .next_symbol_id
            .checked_add(1)
            .expect("symbol id partition overflow");
        self.symbols.insert(key, id);
        id
    }

    /// Intern a SCIP symbol that's about to become a `SymbolRecord`.
    /// Deduplicates by `(language, pub_id)`: when a different SCIP string
    /// resolves to a previously-seen `pub_id`, the SCIP string aliases to
    /// the existing `short_id` and `is_new = false` tells the caller to
    /// skip emitting a duplicate `SymbolRecord`.
    pub fn intern_with_pub_id(
        &mut self,
        language: Language,
        scip_symbol: &str,
        pub_id: &str,
    ) -> (ShortId, bool) {
        let pub_key = (language, pub_id.to_string());
        if let Some(existing) = self.by_pub_id.get(&pub_key).copied() {
            // Alias the SCIP string so edge derivation hits the same id.
            self.symbols
                .insert((language, scip_symbol.to_string()), existing);
            return (existing, false);
        }
        let scip_key = (language, scip_symbol.to_string());
        let id = if let Some(existing) = self.symbols.get(&scip_key).copied() {
            existing
        } else {
            let next = compose_short_id(self.language, self.next_symbol_id);
            self.next_symbol_id = self
                .next_symbol_id
                .checked_add(1)
                .expect("symbol id partition overflow");
            self.symbols.insert(scip_key, next);
            next
        };
        self.by_pub_id.insert(pub_key, id);
        // If we've already emitted a SymbolRecord for this short_id under
        // a different pub_id alias, the caller must NOT emit again — that
        // would violate the design-D5 exactly-once invariant. The
        // `symbols` table can collide on `scip_key` when a SCIP string is
        // first seen via edge derivation (no emit) and later via two
        // separate `Document.symbols` entries with different pub_ids; the
        // second entry resolves to the same `short_id` but produces a new
        // `pub_id` — record the alias, skip the emit.
        let is_new = !self.full_emitted.contains(&id);
        (id, is_new)
    }

    #[must_use]
    pub fn lookup_symbol(&self, language: Language, scip_symbol: &str) -> Option<ShortId> {
        self.symbols
            .get(&(language, scip_symbol.to_string()))
            .copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_registry_assigns_increasing_ids() {
        let mut r = IdRegistry::new(Language::Rust);
        let a = r.intern_file("a.rs");
        let b = r.intern_file("b.rs");
        assert_eq!(a, compose_short_id(Language::Rust, 1));
        assert_eq!(b, compose_short_id(Language::Rust, 2));
        assert_eq!(
            r.intern_file("a.rs"),
            compose_short_id(Language::Rust, 1),
            "interning is idempotent"
        );

        let s1 = r.intern_symbol(Language::Rust, "rust-analyzer cargo k 0.1 foo().");
        let s2 = r.intern_symbol(Language::Rust, "rust-analyzer cargo k 0.1 bar().");
        assert_eq!(s1, compose_short_id(Language::Rust, 1));
        assert_eq!(s2, compose_short_id(Language::Rust, 2));
        assert_eq!(
            r.lookup_symbol(Language::Rust, "rust-analyzer cargo k 0.1 foo()."),
            Some(compose_short_id(Language::Rust, 1))
        );
    }

    #[test]
    fn intern_with_pub_id_skips_second_emit_for_same_short_id() {
        // Regression: when a scip_symbol is interned twice via
        // `intern_with_pub_id` with DIFFERENT pub_ids (e.g., same SCIP
        // string appears in two `Document.symbols` entries with
        // distinct public ids), the second call must return
        // `is_new = false` once a SymbolRecord has been emitted for the
        // short_id, otherwise the caller would double-write and trip
        // the design-D5 exactly-once guard in
        // `kenn-store/.../graph/writer.rs`.
        let mut r = IdRegistry::new(Language::Rust);
        let scip = "rust-analyzer cargo k 0.1 Foo#";
        let (id1, is_new1) = r.intern_with_pub_id(Language::Rust, scip, "pub_one");
        assert!(is_new1, "first pub_id yields a fresh emit");
        // Caller emits SymbolRecord and marks emitted (mirrors the
        // production callsite in `transform_document`).
        r.mark_full_emitted(id1);
        let (id2, is_new2) = r.intern_with_pub_id(Language::Rust, scip, "pub_two");
        assert_eq!(id2, id1, "same scip_symbol → same short_id (alias)");
        assert!(
            !is_new2,
            "must NOT claim new — short_id was already emitted"
        );
    }

    #[test]
    fn distinct_language_partitions_get_distinct_ids() {
        // Each registry owns its own language partition, so the same
        // 1-based counter composes to a different `short_id`.
        let mut ts = IdRegistry::new(Language::TypeScript);
        let mut rs = IdRegistry::new(Language::Rust);
        let a = ts.intern_symbol(Language::TypeScript, "scip-typescript npm . . Foo#");
        let b = rs.intern_symbol(Language::Rust, "rust-analyzer cargo . . Foo#");
        assert_ne!(a, b);
    }
}
