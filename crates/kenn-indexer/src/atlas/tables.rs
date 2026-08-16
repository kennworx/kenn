//! Table-axis SELECTION: which tables the graph knows, and every site that
//! declares, modifies, or accesses each.
//!
//! Read STRAIGHT from the table edges — explicit, complete, deterministic. Like
//! [`super::contracts`], and for the same reason: the atlas producer and the
//! `kenn tables` query must select the *same* tables from their different
//! inputs, so the rule lives in one place and neither grows a copy.
//!
//! Selection only. Render caps and concept-id slugs stay in the producer: a cap
//! is presentation policy, and a query has to be able to reach every table.
//!
//! **Per-site, not rolled up.** The contracts axis groups implementers by
//! package because a package is where an implementer lives. A table's
//! references have no such home — a statement in a migration, an element in a
//! changelog and a function in application code are three different files in
//! three different languages, and which file made the reference is the whole
//! answer to "what touches `orders`, and where". Rolling them up to their
//! aggregate would collapse exactly that.

use std::collections::{BTreeMap, BTreeSet};

use kenn_model::ShortId;

/// What a reference does to the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum RefKind {
    Declares,
    Modifies,
    Accesses,
}

impl RefKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Declares => "declares",
            Self::Modifies => "modifies",
            Self::Accesses => "accesses",
        }
    }
}

/// One reference to a table, as the caller projected it from the store.
#[derive(Debug, Clone)]
pub struct RefSite<'a> {
    /// The symbol that made the reference — a statement, an element, a function.
    pub symbol: ShortId,
    /// Its display name.
    pub name: &'a str,
    /// The file it lives in. The grouping key: "where" is the question.
    pub file: &'a str,
    /// That file's language, so a reader can see a table named by a migration,
    /// a changelog and application code at a glance.
    pub language: &'a str,
    pub kind: RefKind,
}

/// One table and everything that names it.
#[derive(Debug, Clone)]
pub struct SelectedTable<'a> {
    pub node: ShortId,
    /// Display name, schema-qualified when the source qualified it.
    pub name: &'a str,
    /// True when some statement in this workspace declares the table. False
    /// means the schema is owned elsewhere — which is the common case, not an
    /// error: measured on a real repository, 85 of 133 tables were named only
    /// by an XML attribute and declared by no `.sql` file at all.
    pub internal: bool,
    /// References grouped by file, each file's sites sorted by name (ties by
    /// id). Files are ordered by reference count, then path, and UNCAPPED.
    pub by_file: Vec<(&'a str, Vec<RefSite<'a>>)>,
    /// Distinct referencing files, before any render cap.
    pub file_span: u64,
    /// Distinct referencing languages, before any render cap.
    pub language_span: u64,
    /// Total reference sites, before any render cap.
    pub total_refs: u64,
}

/// Select every table in the graph, broadest reference first.
///
/// **No earned-span floor**, unlike `MIN_CONTRACT_PKGS`. A contract implemented
/// in one package is local detail its package concept already covers, but a
/// table referenced from one file is not covered by anything else — nothing in
/// the atlas is organised around it, which is the whole reason this axis exists.
/// Excluding it would hide the single-owner tables a reader most needs to find.
///
/// Tables are small enough to enumerate honestly: a real repository carried 128
/// distinct tables against tens of thousands of code symbols.
#[must_use]
pub fn select_tables<'a>(
    tables: &[(ShortId, &'a str)],
    refs: &[(ShortId, RefSite<'a>)],
) -> Vec<SelectedTable<'a>> {
    let mut by_table: BTreeMap<ShortId, Vec<RefSite<'a>>> = BTreeMap::new();
    for (table, site) in refs {
        by_table.entry(*table).or_default().push(site.clone());
    }

    let mut out: Vec<SelectedTable<'a>> = tables
        .iter()
        .map(|(node, name)| {
            let sites = by_table.remove(node).unwrap_or_default();
            let internal = sites.iter().any(|s| s.kind == RefKind::Declares);
            let languages: BTreeSet<&str> = sites.iter().map(|s| s.language).collect();
            let total_refs = sites.len() as u64;

            let mut grouped: BTreeMap<&'a str, Vec<RefSite<'a>>> = BTreeMap::new();
            for s in sites {
                grouped.entry(s.file).or_default().push(s);
            }
            let mut by_file: Vec<(&'a str, Vec<RefSite<'a>>)> = grouped
                .into_iter()
                .map(|(file, mut sites)| {
                    sites.sort_by(|a, b| a.name.cmp(b.name).then(a.symbol.cmp(&b.symbol)));
                    (file, sites)
                })
                .collect();
            // Heaviest file first, then path — deterministic either way.
            by_file.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

            SelectedTable {
                node: *node,
                name,
                internal,
                file_span: by_file.len() as u64,
                language_span: languages.len() as u64,
                total_refs,
                by_file,
            }
        })
        .collect();

    // Reference BREADTH, not volume: a table named by a migration, a mapper
    // document and application code is the architecturally interesting one, and
    // a hundred reads from one file does not make a table more central than
    // that. Files first, then languages, then total, then name for stability.
    out.sort_by(|a, b| {
        b.file_span
            .cmp(&a.file_span)
            .then(b.language_span.cmp(&a.language_span))
            .then(b.total_refs.cmp(&a.total_refs))
            .then(a.name.cmp(b.name))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn site<'a>(
        symbol: ShortId,
        name: &'a str,
        file: &'a str,
        language: &'a str,
        kind: RefKind,
    ) -> RefSite<'a> {
        RefSite {
            symbol,
            name,
            file,
            language,
            kind,
        }
    }

    #[test]
    fn a_table_referenced_from_one_file_still_earns_a_concept() {
        // The no-floor rule. A contract implemented in one package is covered by
        // that package's concept; a table referenced from one file is covered by
        // nothing, because nothing else in the atlas is organised around tables.
        let refs = [(
            1,
            site(10, "create#0", "schema.sql", "sql", RefKind::Declares),
        )];
        let got = select_tables(&[(1, "users")], &refs);
        assert_eq!(got.len(), 1, "selected: {:?}", got.len());
        assert_eq!(got[0].file_span, 1);
    }

    #[test]
    fn breadth_outranks_volume() {
        // Ten reads from one file do not make a table more central than one
        // named by a migration, a changelog and application code.
        let refs = [
            (1, site(10, "a", "app.rs", "rust", RefKind::Accesses)),
            (1, site(11, "b", "app.rs", "rust", RefKind::Accesses)),
            (1, site(12, "c", "app.rs", "rust", RefKind::Accesses)),
            (1, site(13, "d", "app.rs", "rust", RefKind::Accesses)),
            (2, site(20, "e", "schema.sql", "sql", RefKind::Declares)),
            (2, site(21, "f", "log.xml", "xml", RefKind::Modifies)),
            (2, site(22, "g", "svc.cs", "csharp", RefKind::Accesses)),
        ];
        let got = select_tables(&[(1, "deep"), (2, "broad")], &refs);
        assert_eq!(
            got.iter().map(|t| t.name).collect::<Vec<_>>(),
            ["broad", "deep"],
            "breadth first"
        );
        assert_eq!(got[0].language_span, 3);
        assert_eq!(
            got[1].total_refs, 4,
            "the deep one still reports its volume"
        );
    }

    #[test]
    fn a_table_nothing_declares_is_external_and_still_selected() {
        // The common case, not an error: measured on a real repository, 85 of
        // 133 tables were named only by an attribute and declared nowhere.
        let refs = [(1, site(10, "q", "svc.cs", "csharp", RefKind::Accesses))];
        let got = select_tables(&[(1, "orders")], &refs);
        assert!(!got[0].internal, "no statement declares it");
        assert_eq!(got[0].file_span, 1, "but it is still on the map");
    }

    #[test]
    fn a_declaration_anywhere_marks_the_table_internal() {
        let refs = [
            (1, site(10, "q", "svc.cs", "csharp", RefKind::Accesses)),
            (
                1,
                site(11, "create#0", "schema.sql", "sql", RefKind::Declares),
            ),
        ];
        assert!(select_tables(&[(1, "users")], &refs)[0].internal);
    }

    #[test]
    fn references_group_by_file_not_by_aggregate() {
        // The per-site rule. "What touches `orders`, and where" is answered by
        // the file; rolling up to an aggregate would collapse the answer.
        let refs = [
            (1, site(10, "b", "two.sql", "sql", RefKind::Accesses)),
            (1, site(11, "a", "one.sql", "sql", RefKind::Declares)),
            (1, site(12, "c", "one.sql", "sql", RefKind::Modifies)),
        ];
        let got = select_tables(&[(1, "orders")], &refs);
        let files: Vec<&str> = got[0].by_file.iter().map(|(f, _)| *f).collect();
        assert_eq!(files, ["one.sql", "two.sql"], "heaviest file first");
        let names: Vec<&str> = got[0].by_file[0].1.iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "c"], "sites sorted within the file");
    }

    #[test]
    fn a_table_with_no_references_is_still_a_table() {
        // It exists because something declared it; an empty reference list is a
        // fact worth showing, not a reason to drop the row.
        let got = select_tables(&[(1, "orphan")], &[]);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].total_refs, 0);
        assert!(!got[0].internal);
    }

    #[test]
    fn selection_is_deterministic_for_equal_breadth() {
        let refs = [
            (1, site(10, "a", "x.sql", "sql", RefKind::Accesses)),
            (2, site(20, "b", "y.sql", "sql", RefKind::Accesses)),
        ];
        let first = select_tables(&[(1, "beta"), (2, "alpha")], &refs);
        let second = select_tables(&[(2, "alpha"), (1, "beta")], &refs);
        assert_eq!(
            first.iter().map(|t| t.name).collect::<Vec<_>>(),
            second.iter().map(|t| t.name).collect::<Vec<_>>(),
            "ties break by name, whatever the input order"
        );
    }
}
