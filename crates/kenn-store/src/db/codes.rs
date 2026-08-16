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
pub(crate) const ALL_EDGE_KINDS: [EdgeKind; 20] = [
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
    EdgeKind::DefinesTable,
    EdgeKind::AltersTable,
    EdgeKind::AccessesTable,
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

/// True for languages whose content is structured **values** rather than
/// identifiers — XML and SQL.
///
/// [`split_identifier`] is right for code: breaking `getUserId` into words is
/// what makes it reachable by "user". It is wrong here, because the punctuation
/// *is* the value. Split, `org.springframework:boot` becomes three words and
/// the exact string someone would search for stops matching. The trigram index
/// handles raw text directly, so these languages are indexed as written.
pub(crate) fn is_verbatim_language(lang: &str) -> bool {
    lang == kenn_model::Language::Xml.db_name() || lang == kenn_model::Language::Sql.db_name()
}

/// The lexical projection for a verbatim language: both surfaces, values intact.
///
/// Signature and content are stored separately so each stays usable on its own
/// — a consumer can re-parse an attribute out of one, or hand the other to a SQL
/// parser untouched. Search wants them together, though: "which document pins
/// this version" is answered by an attribute, and "which migration drops this
/// column" by element text, and a caller should not have to know which.
///
/// **This is a stopgap for SQL, and it names its own retirement condition.** A
/// statement's whole text is searchable verbatim because *columns are not
/// nodes*: `ALTER TABLE users ADD COLUMN last_login` is the only place
/// `last_login` appears in the graph, so dropping the text would make the column
/// unfindable. When columns become nodes with their own identities, this shrinks
/// to a real signature — verb plus tables — and the text stops being indexed
/// wholesale. Until then a statement's `name_text` is bigger than a signature
/// should be, on purpose.
///
/// Markup is flattened to words rather than kept as tags. `<dep groupId="x">`
/// searched as written would need the query to include the angle brackets and
/// the `=`; flattened, `groupId x` matches, and so does a bare `x`. The
/// delimiters produce only boundary-spanning trigrams (`Id=`, `="x`) that no
/// query contains.
pub(crate) fn verbatim_projection(sig: &str, doc: &str) -> String {
    let mut out = String::with_capacity(sig.len() + doc.len() + 1);
    flatten_markup(sig, &mut out);
    if !doc.is_empty() {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(doc);
    }
    out
}

/// Strip markup delimiters to spaces and resolve the entities the renderer
/// introduced, so a value is searchable exactly as its source spelled it.
///
/// Only the five XML predefined entities, and only the three the signature
/// renderer emits plus the two a source may already contain. Anything else is
/// left alone: this is a search projection, not a parser, and an unrecognized
/// `&…;` is likelier to be literal text than an entity.
fn flatten_markup(sig: &str, out: &mut String) {
    // Delimiters first, entities second, and the order is load-bearing: an
    // escaped `&quot;` stands for a quote that is *data*, so resolving it first
    // would hand the delimiter pass a quote to eat. Flattening first leaves the
    // entity untouched, since none of its characters are delimiters.
    let flattened: String = sig
        .chars()
        .map(|c| {
            if matches!(c, '<' | '>' | '=' | '"' | '/') {
                ' '
            } else {
                c
            }
        })
        .collect();
    // `&amp;` resolves LAST, or `&amp;lt;` — a literal "&lt;" in the source —
    // would decode twice and come out as `<`.
    let resolved = flattened
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&apos;", "'")
        .replace("&amp;", "&");
    // The delimiters became spaces, so runs of them collapse; a leading or
    // trailing space would otherwise ride into the indexed text.
    out.push_str(&resolved.split_whitespace().collect::<Vec<_>>().join(" "));
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

    /// `ALL_EDGE_KINDS` is the only list `parse_edge_relation` searches, so a
    /// kind missing from it is unreachable by name — `scan_edges` answers
    /// `unknown relation` for an edge the graph actually holds. Uniqueness
    /// above cannot catch that: it only walks whatever the list already
    /// contains. Codes are dense from 1, so the code space is the completeness
    /// oracle.
    #[test]
    fn every_edge_kind_code_is_listed() {
        let listed: HashSet<u32> = ALL_EDGE_KINDS.into_iter().map(edge_kind_code).collect();
        let mut code = 1;
        while let Ok(kind) = EdgeKind::try_from(code) {
            assert!(
                listed.contains(&code),
                "{kind:?} (code {code}) is missing from ALL_EDGE_KINDS"
            );
            code += 1;
        }
        assert_eq!(
            listed.len(),
            (code - 1) as usize,
            "ALL_EDGE_KINDS holds an entry outside the dense code space"
        );
    }
}

#[cfg(test)]
mod verbatim_tests {
    use super::{is_verbatim_language, split_identifier, verbatim_projection};

    #[test]
    fn an_attribute_and_element_text_are_both_reachable() {
        // The point of deriving from both surfaces. Storing them separately is
        // what makes each usable alone; search should not have to know which
        // one an answer lives on.
        let text = verbatim_projection(r#"<dep groupId="org.springframework">"#, "1.2.3");
        assert!(text.contains("org.springframework"), "attribute: {text}");
        assert!(text.contains("1.2.3"), "element text: {text}");
    }

    #[test]
    fn a_structured_value_is_not_broken_into_words() {
        // The reason these languages skip `split_identifier`: the punctuation
        // IS the value, and splitting makes the exact string someone would
        // search for unfindable.
        let v = "org.springframework:boot";
        let text = verbatim_projection(&format!(r#"<dep id="{v}">"#), "");
        assert!(text.contains(v), "intact: {text}");
        assert_ne!(
            split_identifier(v),
            v,
            "the code projection really would have broken it — otherwise this \
             test passes for the wrong reason"
        );
    }

    #[test]
    fn markup_delimiters_become_word_boundaries() {
        // Kept as written, a query would have to include the angle brackets and
        // the `=`. Flattened, both the pair and the bare value match.
        let text = verbatim_projection(r#"<dep groupId="acme" version="1.0">"#, "");
        assert_eq!(text, "dep groupId acme version 1.0");
    }

    #[test]
    fn the_renderers_escapes_are_resolved_back_to_the_source_spelling() {
        // The signature escapes `"` and `&` so it stays re-parseable. Searching
        // is a different job: someone looking for `a & b` types an ampersand,
        // not `&amp;`, so the projection restores what the source said.
        let text = verbatim_projection(r#"<e cmd="a &amp; b" q="say &quot;hi&quot;">"#, "");
        assert!(text.contains("a & b"), "ampersand resolved: {text}");
        assert!(text.contains(r#"say "hi""#), "quote resolved: {text}");
        assert!(!text.contains("&amp;"), "no entity left behind: {text}");
    }

    #[test]
    fn an_escaped_quote_is_data_and_not_a_delimiter() {
        // The ordering constraint inside `flatten_markup`. Resolving entities
        // before flattening delimiters would turn `&quot;` into a quote and
        // then eat it as a delimiter, silently losing a character the source
        // wrote. Flattening first leaves the entity intact to be resolved.
        let text = verbatim_projection(r#"<e q="say &quot;hi&quot; twice">"#, "");
        assert_eq!(text, r#"e q say "hi" twice"#);
    }

    #[test]
    fn a_literally_written_entity_is_not_decoded_twice() {
        // `&amp;lt;` is how a source spells the literal text "&lt;". Resolving
        // `&amp;` before the others would yield `&lt;` and then `<` — a
        // character the source never wrote. `&amp;` resolves last for this.
        let text = verbatim_projection(r#"<e v="&amp;lt;">"#, "");
        assert_eq!(text, "e v &lt;", "one decode, not two");
    }

    #[test]
    fn a_sql_statement_reaches_the_index_through_its_content_surface() {
        // SQL statements carry no signature yet, so the whole projection is the
        // content — and it must arrive intact, not word-split.
        let stmt = "ALTER TABLE users ADD COLUMN last_login timestamptz";
        assert_eq!(verbatim_projection("", stmt), stmt);
    }

    #[test]
    fn both_languages_take_the_verbatim_arm_and_code_does_not() {
        // One arm covering both, which is the point — two arms would drift.
        assert!(is_verbatim_language("xml"));
        assert!(is_verbatim_language("sql"));
        for code in ["rust", "csharp", "typescript", "python", "go", "swift"] {
            assert!(!is_verbatim_language(code), "{code} is code");
        }
    }
}
