//! Turning a workspace's source into code→table references.
//!
//! **Pure.** Store rows and a source reader in, records to emit out — no sink,
//! no filesystem of its own. The barrier step supplies the inputs and writes
//! what comes back, so everything decided here is testable without a store.

use std::collections::BTreeMap;

use kenn_model::{LinkGrade, ShortId};
use kenn_store::SymbolBodyRow;

use super::attribute::{owner, Extent};
use super::literals::literals;
use crate::sql::parse::{extract, RefRole};
use crate::sql::registry::{resolve as resolve_name, NameSet, TableKey, Union};

/// One code→table reference.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodeTableRef {
    /// The symbol whose body carried the SQL — never its enclosing scope.
    pub sym_id: ShortId,
    pub table: TableKey,
    pub role: RefRole,
    pub grade: LinkGrade,
}

/// What the pass saw, so a reader can tell a table no code touches from one
/// whose access was not visible.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CodeSqlCounts {
    pub bodies_scanned: u64,
    pub bodies_with_literals: u64,
    pub refs_emitted: u64,
    /// Tables named by code that nothing in the workspace declares.
    pub tables_minted: u64,
    /// References that reached no table node and were discarded.
    ///
    /// Should be zero. It is reported rather than assumed because a silent
    /// version of this hid a lost declaration through a full corpus run.
    pub refs_dropped: u64,
}

/// The references found, plus the identities that had to be minted for them.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub refs: Vec<CodeTableRef>,
    /// Identities no known table matched, in a stable order — the caller mints
    /// one node each and points the matching refs at them.
    pub minted: Vec<TableKey>,
    pub counts: CodeSqlCounts,
}

/// Group extents by the file they live in, so each file is read once.
///
/// Reading once is not only cheaper. Extents nest, so slicing per symbol
/// re-reads the same bytes once per enclosing scope — and it is that shape
/// which hands every ancestor its descendants' tables.
fn by_file(bodies: &[SymbolBodyRow]) -> BTreeMap<(String, String), Vec<Extent>> {
    let mut out: BTreeMap<(String, String), Vec<Extent>> = BTreeMap::new();
    for b in bodies {
        out.entry((b.path.clone(), b.language.clone()))
            .or_default()
            .push(Extent {
                sym_id: b.sym_id,
                start: b.body_start_line,
                end: b.body_end_line,
            });
    }
    out
}

/// Find every code→table reference in the workspace.
///
/// `known` is the identity set read back from the store — what the `.sql` pass
/// wrote. `read_source` returns a file's text, or `None` when it cannot be read
/// (deleted, unreadable, binary); an unreadable file contributes nothing and is
/// not a failure.
pub fn resolve(
    known: &NameSet,
    bodies: &[SymbolBodyRow],
    read_source: &dyn Fn(&str) -> Option<String>,
) -> Resolved {
    let mut out = Resolved::default();
    let mut minted: Vec<TableKey> = Vec::new();
    // Identities minted so far, so two functions naming the same unknown table
    // reach one node rather than two.
    let mut seen_minted: NameSet = NameSet::new();

    for ((path, language), spans) in by_file(bodies) {
        out.counts.bodies_scanned += spans.len() as u64;

        let Some(src) = read_source(&path) else {
            continue;
        };
        let mut bodies_with_lits: std::collections::BTreeSet<ShortId> =
            std::collections::BTreeSet::new();

        for lit in literals(&language, &src) {
            let Some(sym) = owner(&spans, lit.line) else {
                // Outside every recorded extent — no symbol owns it, and
                // falling back to the file would collapse the same way one
                // level up.
                continue;
            };
            bodies_with_lits.insert(sym);
            for r in refs_of_literal(&lit.text) {
                // Resolve against what the `.sql` pass wrote AND what this pass
                // has minted, so a reference can reach a sibling minted moments
                // ago rather than starting a second identity for one table.
                let candidates = resolve_name(
                    &Union {
                        known,
                        minted: &seen_minted,
                    },
                    r.schema.as_deref(),
                    &r.name,
                );
                for c in candidates {
                    // Test the whole key, not the bare name. A name-only guard
                    // lets `users.orders` satisfy it for `orders`, whose edge
                    // then targets a node nothing minted and is silently
                    // dropped by `emit_table_edges`.
                    if !known.contains(&c.key) && !seen_minted.contains(&c.key) {
                        seen_minted.insert(c.key.clone());
                        minted.push(c.key.clone());
                    }
                    out.refs.push(CodeTableRef {
                        sym_id: sym,
                        table: c.key,
                        role: r.role,
                        grade: c.grade,
                    });
                }
            }
        }
        out.counts.bodies_with_literals += bodies_with_lits.len() as u64;
    }

    out.counts.refs_emitted = out.refs.len() as u64;
    out.counts.tables_minted = minted.len() as u64;
    out.minted = minted;
    out
}

/// The table references one literal makes, or none when it is not SQL.
///
/// A literal can be either of two things, and telling them apart is the whole
/// job here. Most are one fragment of a query assembled at runtime, where a
/// partial parse is where a query-local name gets read as a table. Some are a
/// complete schema constant, where the unparsed leftovers are whole statements
/// the parser does not know — a vendor extension such as
/// `CREATE VIRTUAL TABLE … USING fts5(…)`, which no supported dialect accepts.
///
/// So a partial parse keeps the statements whose names cannot lie
/// ([`names_are_positional`]) and drops the rest. Refusing the whole literal
/// instead used to cost this workspace its own schema: 26 statements parsed,
/// one did not, and all 26 were discarded along with every declaration of the
/// 14 tables they create.
///
/// Most literals in any codebase are messages, paths, and formats — measured,
/// 4103 bodies carried literals and 154 named a table — so a non-parse is
/// ordinary text, never a reported failure.
fn refs_of_literal(text: &str) -> Vec<crate::sql::parse::TableRef> {
    let t = text.trim();
    // A statement needs more than a few characters; skipping the short ones
    // keeps the dialect sweep off the overwhelming majority of literals.
    if t.len() < 12 {
        return Vec::new();
    }
    // The expensive part is the 14-dialect sweep on text that is not SQL, and
    // measured, 97% of literals are not. A literal naming no statement keyword
    // cannot yield a table, so it never reaches the parser.
    if !crate::sql::parse::looks_like_sql(t) {
        return Vec::new();
    }
    let ex = extract(t, None);
    let whole = ex.unparsed == 0;
    ex.statements
        .into_iter()
        .filter(|s| {
            whole
                || s.verb
                    .is_some_and(crate::sql::parse::Verb::names_positional)
        })
        .flat_map(|s| s.refs)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn body(sym: u32, path: &str, start: u32, end: u32) -> SymbolBodyRow {
        SymbolBodyRow {
            sym_id: sym,
            path: path.to_owned(),
            language: "rust".to_owned(),
            body_start_line: start,
            body_end_line: end,
            test: false,
        }
    }

    fn src_of(text: &'static str) -> impl Fn(&str) -> Option<String> {
        move |_| Some(text.to_owned())
    }

    /// Two schemas holding the same table name each get their own node.
    ///
    /// The code barrier's copy of `two_schemas_sharing_a_name_both_get_nodes`.
    /// Both steps carried the same name-keyed mint guard, so fixing one and
    /// leaving the other would put a table's fate down to whether its schema was
    /// written in a `.cs` file or a changelog.
    #[test]
    fn two_schemas_sharing_a_name_both_get_nodes() {
        let known = NameSet::from_table_pub_ids(Vec::<&str>::new());
        let bodies = [body(1, "a.rs", 1, 4)];
        let got = resolve(
            &known,
            &bodies,
            &src_of(
                "fn f() {\n  \
                 let a = \"CREATE TABLE sales.orders (id INT)\";\n  \
                 let b = \"CREATE TABLE archive.orders (id INT)\";\n}",
            ),
        );
        let keys: std::collections::BTreeSet<&TableKey> =
            got.refs.iter().map(|r| &r.table).collect();
        assert_eq!(keys.len(), 2, "two distinct tables: {keys:?}");
        for r in &got.refs {
            assert!(
                got.minted.contains(&r.table),
                "reference to {:?} was never minted; minted = {:?}",
                r.table,
                got.minted
            );
        }
    }

    /// The `CREATE VIRTUAL TABLE` spelling that started all this. Kept verbatim
    /// — `tokenize='unicode61'` is the named argument sqlparser rejects, and a
    /// simplified `USING fts5(words)` may well parse, in which case the two
    /// tests below would never reach the partial-parse branch they are named
    /// for (CLAUDE.md §9, "suspect the fixture").
    const SCHEMA_CONST: &str = "\
fn schema() {
  let ddl = \"
    CREATE TABLE symbols (id INTEGER NOT NULL, name TEXT NOT NULL);
    CREATE INDEX symbols_id ON symbols(id);
    CREATE VIRTUAL TABLE name_words USING fts5(words, tokenize='unicode61');
    CREATE TABLE defs (sym_id INTEGER NOT NULL);
  \";
}";

    /// The fixture reaches the branch: this asserts the literal really does
    /// parse only in part, so the recovery below is not passing for some
    /// unrelated reason.
    #[test]
    fn the_schema_fixture_really_is_a_partial_parse() {
        let ddl = SCHEMA_CONST
            .split_once("let ddl = \"")
            .and_then(|(_, r)| r.split_once("\";"))
            .expect("literal")
            .0;
        let ex = crate::sql::parse::extract(ddl, None);
        assert!(
            ex.unparsed > 0,
            "fixture parses whole — it no longer exercises the partial-parse path"
        );
        assert!(!ex.statements.is_empty(), "nothing parsed at all");
    }

    /// A schema constant survives one statement no dialect can read.
    ///
    /// Mutation-checked against the rule it guards: restore the old
    #[test]
    fn a_schema_constant_survives_one_unreadable_statement() {
        let known = NameSet::from_table_pub_ids(Vec::<&str>::new());
        let bodies = [body(1, "schema.rs", 1, 9)];
        let got = resolve(&known, &bodies, &src_of(SCHEMA_CONST));

        let mut named: Vec<&str> = got.refs.iter().map(|r| r.table.name.as_str()).collect();
        named.sort_unstable();
        named.dedup();
        assert_eq!(
            named,
            ["defs", "symbols"],
            "both real tables, and not the virtual one: {named:?}"
        );
        // The two `CREATE TABLE`s declare; the `CREATE INDEX` reads the table it
        // indexes. All three are DDL, which is why all three survive — a rule
        // that kept only `Defines` would have dropped the index.
        assert!(
            got.refs
                .iter()
                .any(|r| r.table.name == "symbols" && r.role == RefRole::Defines),
            "symbols is declared"
        );
        assert!(
            got.refs
                .iter()
                .any(|r| r.table.name == "symbols" && r.role == RefRole::Accesses),
            "the CREATE INDEX reference survives too"
        );
    }

    /// And the protection the rule exists for still holds: a torn-off query
    /// piece contributes nothing, even though it parses cleanly on its own.
    ///
    /// Mutation-checked the other way: drop the `names_are_positional` filter
    /// (trust every statement on a partial parse) and this finds `recent`.
    #[test]
    fn a_torn_query_fragment_still_contributes_nothing() {
        let known = NameSet::from_table_pub_ids(Vec::<&str>::new());
        let bodies = [body(1, "q.rs", 1, 5)];
        // The `WITH recent AS (…)` that would have made `recent` a CTE lives in
        // another literal; here the tail parses as a plain SELECT and `recent`
        // reads as a table. That is the false positive being prevented.
        let got = resolve(
            &known,
            &bodies,
            &src_of(
                "fn f() {\n  let q = \"\
                 SELECT id FROM recent WHERE x = 1;\n\
                 ) SELECT\";\n}",
            ),
        );
        assert!(
            got.refs.is_empty(),
            "a query verb must not be trusted under a partial parse: {:?}",
            got.refs.iter().map(|r| &r.table.name).collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_function_that_queries_a_table_references_it() {
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let bodies = [body(1, "a.rs", 1, 3)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("fn f() {\n  let q = \"SELECT id FROM users\";\n}"),
        );
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.refs[0].sym_id, 1);
        assert_eq!(got.refs[0].table.name, "users");
        assert_eq!(got.refs[0].role, RefRole::Accesses);
    }

    #[test]
    fn the_enclosing_module_does_not_inherit_the_reference() {
        // The measured failure this whole design turns on.
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let bodies = [body(1, "a.rs", 1, 9), body(2, "a.rs", 2, 4)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("mod m {\n fn f() {\n  let q = \"SELECT id FROM users\";\n }\n}"),
        );
        assert_eq!(got.refs.len(), 1, "one reference, not one per scope");
        assert_eq!(got.refs[0].sym_id, 2, "the function, not the module");
    }

    #[test]
    fn ddl_in_code_declares() {
        let known = NameSet::new();
        let bodies = [body(1, "a.rs", 1, 3)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("fn f() {\n let s = \"CREATE TABLE sessions (id INT)\";\n}"),
        );
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.refs[0].role, RefRole::Defines);
    }

    #[test]
    fn an_unknown_table_is_minted_once_however_many_functions_name_it() {
        // Two functions naming the same undeclared table must reach one node.
        let known = NameSet::new();
        let bodies = [body(1, "a.rs", 1, 2), body(2, "a.rs", 3, 4)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("let a = \"SELECT x FROM audit_log\";\n\nlet b = \"DELETE FROM audit_log\";\n"),
        );
        assert_eq!(got.minted.len(), 1, "one identity: {:?}", got.minted);
        assert_eq!(got.minted[0].name, "audit_log");
        assert_eq!(got.refs.len(), 2, "both functions still reference it");
    }

    #[test]
    fn ordinary_literals_are_silent() {
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let bodies = [body(1, "a.rs", 1, 5)];
        let got = resolve(
            &known,
            &bodies,
            &src_of(
                "fn f() {\n let a = \"failed to open the configuration file\";\n \
                 let b = \"/usr/local/share/kenn\";\n let c = \"{name}: {count} items\";\n}",
            ),
        );
        assert!(got.refs.is_empty(), "no references: {:?}", got.refs);
        assert_eq!(got.counts.refs_emitted, 0);
    }

    #[test]
    fn a_wholly_unparseable_fragment_contributes_nothing() {
        // The easy half: a concatenated query's middle piece parses as nothing
        // at all. Note this passes with or without the partial-parse guard —
        // it is `extract` returning no statements that protects here.
        let known = NameSet::new();
        let bodies = [body(1, "a.rs", 1, 3)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("fn f() {\n let q = \" JOIN orders o ON o.id = a.id \";\n}"),
        );
        assert!(got.refs.is_empty(), "fragment ignored: {:?}", got.refs);
    }

    #[test]
    fn a_partially_parsing_query_contributes_nothing() {
        // The guard's actual job, and the case the test above does NOT reach.
        // When part of a literal parses and part does not, `extract` falls back
        // to splitting and returns the good pieces alongside an `unparsed`
        // count. That is a runtime-assembled query seen mid-assembly, and its
        // parseable half is exactly where an alias or a CTE name reads as a
        // table.
        //
        // Note what this asserts and what it does not. It used to be named for
        // the whole literal, because the whole literal was discarded. The rule
        // is now narrower — a partial parse keeps its DDL and drops its queries
        // — so what makes `users` invisible here is the SELECT verb, not the
        // literal. Swap the SELECT for a `CREATE TABLE` and the table WOULD be
        // reported, which is `a_schema_constant_survives_one_unreadable_statement`.
        let known = NameSet::new();
        let bodies = [body(1, "a.rs", 1, 3)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("fn f() {\n let q = \"SELECT id FROM users; ((( not sql at all\";\n}"),
        );
        assert!(
            got.refs.is_empty(),
            "a query's names are not trusted beside an unparsed piece: {:?}",
            got.refs
        );
    }

    #[test]
    fn an_unreadable_file_is_not_a_failure() {
        let known = NameSet::new();
        let bodies = [body(1, "gone.rs", 1, 3)];
        let got = resolve(&known, &bodies, &|_| None);
        assert!(got.refs.is_empty());
        assert_eq!(got.counts.bodies_scanned, 1, "counted as seen");
        assert_eq!(got.counts.bodies_with_literals, 0);
    }

    #[test]
    fn counts_distinguish_scanned_from_carrying_literals() {
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let bodies = [body(1, "a.rs", 1, 2), body(2, "a.rs", 3, 4)];
        let got = resolve(
            &known,
            &bodies,
            &src_of("let q = \"SELECT id FROM users\";\n\nlet n = 1;\n"),
        );
        assert_eq!(got.counts.bodies_scanned, 2);
        assert_eq!(got.counts.bodies_with_literals, 1, "only one carried one");
    }

    #[test]
    fn a_test_symbols_reference_is_emitted_not_dropped() {
        // The half this module decides: no index-time filter. "Which tests
        // exercise this table" stays answerable only if the edge is written.
        //
        // The other half — that querying excludes it by default — is not
        // asserted here and deliberately not carried on the reference. The
        // referencing symbol's own row holds the test flag and `RowNarrow`
        // filters on it, exactly as for every other edge kind. A `test` field
        // on `CodeTableRef` would only be able to prove itself.
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let mut b = body(1, "a.rs", 1, 3);
        b.test = true;
        let got = resolve(
            &known,
            &[b],
            &src_of("fn t() {\n let q = \"SELECT id FROM users\";\n}"),
        );
        assert_eq!(got.refs.len(), 1, "emitted despite being test code");
        assert_eq!(got.refs[0].sym_id, 1);
    }
}
