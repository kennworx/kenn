//! Backend-neutral, stable on-disk codes + knowledge-text helpers, shared by
//! the `SQLite` backend (and, until they are deleted, the Lance modules).
//!
//! Relocated here from `db/graph/schema/edge_codes` and `db/lance/schema`
//! so the `SQLite` backend no longer reaches into the Lance/graph modules,
//! letting those be deleted (replace-lance-with-sqlite, Phase 6 step 1).

use std::num::NonZeroU32;

use kenn_model::{EdgeKind, FieldOp, ImportKind, IsomorphismSource, LinkGrade};

pub(crate) const fn field_op_code(op: FieldOp) -> u8 {
    match op {
        FieldOp::Read => 0,
        FieldOp::Write => 1,
    }
}

pub(crate) const fn import_kind_code(kind: ImportKind) -> u8 {
    match kind {
        ImportKind::Explicit => 0,
        ImportKind::ReExport => 1,
    }
}

pub(crate) const fn iso_source_code(source: IsomorphismSource) -> u8 {
    match source {
        IsomorphismSource::Config => 0,
        IsomorphismSource::AutoInferred => 1,
        IsomorphismSource::Codegen => 2,
    }
}

/// Stable on-disk `u8` discriminant for a markdown link's [`LinkGrade`].
pub(crate) const fn link_grade_code(grade: LinkGrade) -> u8 {
    match grade {
        LinkGrade::Exact => 0,
        LinkGrade::Drifted => 1,
        LinkGrade::Fuzzy => 2,
        LinkGrade::Ambiguous => 3,
        LinkGrade::Dangling => 4,
    }
}

/// Inverse of [`link_grade_code`]: the grade name for a stored discriminant
/// (the `check_links` read path). Unknown codes fall back to `"exact"`.
pub(crate) const fn link_grade_name(code: u8) -> &'static str {
    match code {
        1 => "drifted",
        2 => "fuzzy",
        3 => "ambiguous",
        4 => "dangling",
        _ => "exact",
    }
}

/// Stable on-disk `u32` discriminant for [`EdgeKind`] — the store-side name for
/// the enum's canonical `NonZeroU32` wire code (`0` is reserved as a null sentinel).
pub(crate) fn edge_kind_code(kind: EdgeKind) -> u32 {
    NonZeroU32::from(kind).get()
}

/// Resolve an edge-relation name (`EdgeKind::db_name`) to its kind.
pub(crate) fn parse_edge_relation(name: &str) -> Option<EdgeKind> {
    ALL_EDGE_KINDS.into_iter().find(|k| k.db_name() == name)
}

/// Resolve a stored `edge_kind_code` back to its relation name. Falls back to the
/// numeric code for an unknown value (forward-compat with a future variant).
pub(crate) fn edge_kind_name(code: u32) -> String {
    EdgeKind::try_from(code).map_or_else(|_| code.to_string(), |k| k.db_name().to_owned())
}

/// Resolve a stored `edge_kind_code` back to its typed [`EdgeKind`] — the O(1)
/// [`TryFrom`] inverse of the wire code, with the store's forward-compat policy:
/// an unknown code (a future variant read by an older binary) falls back to
/// `DefinedIn` rather than erroring the whole scan.
pub(crate) fn edge_kind_from_code(code: u32) -> EdgeKind {
    EdgeKind::try_from(code).unwrap_or(EdgeKind::DefinedIn)
}

/// Every [`EdgeKind`] variant, in discriminant order.
pub(crate) const ALL_EDGE_KINDS: [EdgeKind; 17] = [
    EdgeKind::DefinedIn,
    EdgeKind::Contains,
    EdgeKind::Calls,
    EdgeKind::TypeUse,
    EdgeKind::FieldAccess,
    EdgeKind::Implements,
    EdgeKind::Overrides,
    EdgeKind::Instantiates,
    EdgeKind::GenericConstraint,
    EdgeKind::Imports,
    EdgeKind::CorrespondsTo,
    EdgeKind::LinksTo,
    EdgeKind::Embeds,
    EdgeKind::LinksToFile,
    EdgeKind::UsesCssClass,
    EdgeKind::ExtendsRule,
    EdgeKind::ExtendsType,
];

/// Split an identifier into space-separated lowercase words at `camelCase` /
/// `PascalCase` boundaries, digit/letter boundaries, and non-alphanumeric
/// separators. Applied at index + query time so a multi-word query aligns
/// with an indexed `UserId`.
#[expect(
    clippy::indexing_slicing,
    reason = "chars[i - 1] is reached only inside `!current.is_empty()`, false on the first iteration so i >= 1"
)]
pub(crate) fn split_identifier(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut words: Vec<String> = Vec::new();
    let mut current = String::new();
    for (i, &c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !current.is_empty() {
                words.push(std::mem::take(&mut current));
            }
            continue;
        }
        if !current.is_empty() {
            let prev = chars[i - 1];
            let lower_to_upper = !prev.is_uppercase() && c.is_uppercase();
            let digit_boundary = prev.is_numeric() != c.is_numeric();
            let acronym_tail = prev.is_uppercase()
                && c.is_uppercase()
                && chars.get(i + 1).is_some_and(|n| n.is_lowercase());
            if lower_to_upper || digit_boundary || acronym_tail {
                words.push(std::mem::take(&mut current));
            }
        }
        current.push(c.to_ascii_lowercase());
    }
    if !current.is_empty() {
        words.push(current);
    }
    words.join(" ")
}

/// `xxh3-64` fingerprint of a row's `embeddable_text` — drives
/// embedding reconciliation (an unchanged fingerprint reuses the vector).
pub(crate) fn text_fingerprint(text: &str) -> u64 {
    xxhash_rust::xxh3::xxh3_64(text.as_bytes())
}

#[cfg(test)]
mod tests {
    use super::{edge_kind_code, edge_kind_name, ALL_EDGE_KINDS};
    use kenn_model::EdgeKind;
    use std::collections::HashSet;

    /// Every `EdgeKind` has a unique on-disk code that round-trips back to its
    /// relation name — so a stored edge of any kind (including the augmentation
    /// `extends_type`) is addressable by name on read.
    #[test]
    fn edge_kind_codes_are_unique_and_round_trip() {
        let mut seen = HashSet::new();
        for k in ALL_EDGE_KINDS {
            let code = edge_kind_code(k);
            assert!(
                seen.insert(code),
                "duplicate edge_kind_code {code} for {k:?}"
            );
            assert_eq!(
                edge_kind_name(code),
                k.db_name(),
                "name round-trip for {k:?}"
            );
        }
        // Codes are 1-based (0 is the null sentinel); the augmentation edge is
        // stored under its appended, stable code (last).
        assert_eq!(edge_kind_code(EdgeKind::DefinedIn), 1);
        assert_eq!(edge_kind_code(EdgeKind::ExtendsType), 17);
        assert_eq!(edge_kind_name(17), "extends_type");
        // 0 maps to no edge kind (reserved null sentinel).
        kenn_model::EdgeKind::try_from(0).unwrap_err();
    }
}
