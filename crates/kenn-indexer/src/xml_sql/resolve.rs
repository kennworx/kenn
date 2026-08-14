//! Turning XML elements into table references.
//!
//! **Pure.** Stored surfaces in, references out — no store, no filesystem. The
//! barrier step supplies the rows and writes what comes back, so everything
//! decided here is testable without a store.
//!
//! Two arms, and they answer different halves of a workspace's schema:
//!
//! * **Element text** is SQL a migration wrote out. It goes to the shared
//!   extractor untouched, which is why the producer stores it verbatim on the
//!   content surface — `sqlparser` rejects `sql ALTER TABLE users` at token 1.
//! * **Attributes** name tables that no statement ever spells. Measured during
//!   design, a real repository declared 25 tables by `CREATE TABLE` in `.sql`
//!   and named 103 by an attribute; without this arm most of the schema is
//!   invisible.
//!
//! The attribute arm needs a vocabulary, and the vocabulary is the workspace's.
//! Nothing here names an element, an attribute, or a namespace.

use kenn_config::{TableRole, TableRule, XmlSqlConfig};
use kenn_model::{LinkGrade, ShortId};
use kenn_store::SymbolSurfaceRow;

use crate::sql::parse::{extract, RefRole};
use crate::sql::registry::{resolve as resolve_name, NameSet, TableKey, TableRegistry};

/// One reference from an XML element to a table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlTableRef {
    /// The element that carried it — never its document.
    pub sym_id: ShortId,
    pub table: TableKey,
    pub role: RefRole,
    pub grade: LinkGrade,
}

/// What the pass saw, so a zero can be read.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct XmlSqlCounts {
    /// Elements considered after the root filter.
    pub elements_scanned: u64,
    /// Elements whose text parsed as SQL.
    pub elements_with_sql: u64,
    /// Elements matched by a configured attribute rule.
    pub elements_with_attribute: u64,
    pub refs_emitted: u64,
    pub tables_minted: u64,
}

/// The references found, plus the identities that had to be minted.
#[derive(Debug, Clone, Default)]
pub struct Resolved {
    pub refs: Vec<XmlTableRef>,
    /// Identities no known table matched, in a stable order.
    pub minted: Vec<TableKey>,
    pub counts: XmlSqlCounts,
}

/// Find every element→table reference the workspace's XML makes.
#[must_use]
pub fn resolve(known: &NameSet, config: &XmlSqlConfig, rows: &[SymbolSurfaceRow]) -> Resolved {
    let mut out = Resolved::default();
    let mut minted: Vec<TableKey> = Vec::new();
    // Identities minted so far, so two elements naming the same unknown table
    // reach one node rather than two.
    let mut seen_minted = NameSet::new();
    let roots = RootFilter::new(&config.roots);

    for row in rows {
        if !roots.admits(&row.path) {
            continue;
        }
        out.counts.elements_scanned += 1;

        let mut found: Vec<(TableKey, RefRole)> = Vec::new();
        if refs_from_text(&row.doc, config.dialect.as_deref(), &mut found) {
            out.counts.elements_with_sql += 1;
        }
        if refs_from_attributes(&row.sig, &config.rules, &mut found) {
            out.counts.elements_with_attribute += 1;
        }

        for (raw, role) in found {
            for candidate in resolve_name(known, raw.schema.as_deref(), &raw.name) {
                if known.identities_named(&candidate.key.name).is_empty()
                    && seen_minted.identities_named(&candidate.key.name).is_empty()
                {
                    seen_minted.insert(candidate.key.clone());
                    minted.push(candidate.key.clone());
                }
                out.refs.push(XmlTableRef {
                    sym_id: row.sym_id,
                    table: candidate.key,
                    role,
                    grade: candidate.grade,
                });
            }
        }
    }

    out.counts.refs_emitted = out.refs.len() as u64;
    out.counts.tables_minted = minted.len() as u64;
    out.minted = minted;
    out
}

/// Table references an element's own text makes, when that text is SQL.
///
/// Returns whether the text parsed as SQL at all. Most element text is not —
/// measured, only 1.8% of real elements carry any text — so a non-parse is
/// ordinary content, never a reported failure.
fn refs_from_text(text: &str, dialect: Option<&str>, out: &mut Vec<(TableKey, RefRole)>) -> bool {
    let t = text.trim();
    if t.len() < 12 || !crate::sql::parse::looks_like_sql(t) {
        return false;
    }
    let extraction = extract(t, dialect);
    // A whole element body is one fragment, like a code literal: a partial
    // parse is where an alias or a CTE name gets read as a table.
    if extraction.unparsed > 0 {
        return false;
    }
    let mut any = false;
    for statement in extraction.statements {
        for r in statement.refs {
            any = true;
            out.push((TableKey::new(r.schema, r.name), r.role));
        }
    }
    any
}

/// Table references the configured rules find in an element's signature.
///
/// Reads the *rendered* signature rather than re-reading the source. The
/// producer renders it canonically, so one form has to be handled here rather
/// than every spelling XML permits.
fn refs_from_attributes(
    sig: &str,
    rules: &[TableRule],
    out: &mut Vec<(TableKey, RefRole)>,
) -> bool {
    if rules.is_empty() {
        return false;
    }
    let Some(tag) = tag_of(sig) else {
        return false;
    };
    let mut any = false;
    for rule in rules {
        if rule.element.as_deref().is_some_and(|e| e != tag) {
            continue;
        }
        let Some(value) = attribute_of(sig, &rule.attribute) else {
            continue;
        };
        // Through the same normalization a statement's identifier takes, so a
        // quoted or schema-qualified attribute value resolves to the identity a
        // statement would have produced rather than a second one beside it.
        let Some(key) = crate::sql::parse::normalize_table_name(&value) else {
            continue;
        };
        any = true;
        out.push((key, role_of(rule)));
    }
    any
}

/// The role a matched rule confers. An unbound rule means a plain access: a
/// reference is a fact, but calling one a declaration would mark a table
/// internal that the workspace may not own.
const fn role_of(rule: &TableRule) -> RefRole {
    match rule.role {
        Some(TableRole::Declares) => RefRole::Defines,
        Some(TableRole::Modifies) => RefRole::Alters,
        Some(TableRole::Accesses) | None => RefRole::Accesses,
    }
}

/// The tag name out of a rendered start tag.
fn tag_of(sig: &str) -> Option<&str> {
    let rest = sig.strip_prefix('<')?;
    let end = rest.find([' ', '>']).unwrap_or(rest.len());
    rest.get(..end).filter(|t| !t.is_empty())
}

/// One attribute's value out of a rendered start tag, unescaping what the
/// renderer escaped.
///
/// Sound only because the signature is *rendered*: XML permits
/// `tableName = "users"` with spaces, but a canonical rendering admits exactly
/// `tableName="users"`, so matching that one form cannot miss a value that
/// parsing would have found.
fn attribute_of(sig: &str, attribute: &str) -> Option<String> {
    let needle = format!(" {attribute}=\"");
    let start = sig.find(&needle)? + needle.len();
    let rest = sig.get(start..)?;
    let end = rest.find('"')?;
    let raw = rest.get(..end)?;
    Some(
        raw.replace("&quot;", "\"")
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&apos;", "'")
            .replace("&amp;", "&"),
    )
}

/// Confines the pass to the configured roots.
struct RootFilter {
    /// `None` when the roots admit the whole workspace, so the common case
    /// costs no per-row comparison.
    prefixes: Option<Vec<String>>,
}

impl RootFilter {
    fn new(roots: &[String]) -> Self {
        let whole = roots.is_empty() || roots.iter().any(|r| r == "." || r == "./" || r.is_empty());
        Self {
            prefixes: if whole {
                None
            } else {
                Some(
                    roots
                        .iter()
                        .map(|r| r.trim_start_matches("./").trim_end_matches('/').to_owned())
                        .collect(),
                )
            },
        }
    }

    fn admits(&self, path: &str) -> bool {
        let Some(prefixes) = &self.prefixes else {
            return true;
        };
        let p = path.trim_start_matches("./");
        prefixes
            .iter()
            .any(|r| p == r || p.starts_with(&format!("{r}/")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(sym: ShortId, path: &str, sig: &str, doc: &str) -> SymbolSurfaceRow {
        SymbolSurfaceRow {
            sym_id: sym,
            pub_id: format!("xml:{path}#e{sym}"),
            path: path.to_owned(),
            sig: sig.to_owned(),
            doc: doc.to_owned(),
        }
    }

    fn rule(attribute: &str, element: Option<&str>, role: Option<TableRole>) -> TableRule {
        TableRule {
            attribute: attribute.to_owned(),
            element: element.map(ToOwned::to_owned),
            role,
        }
    }

    fn config(rules: Vec<TableRule>) -> XmlSqlConfig {
        XmlSqlConfig {
            rules,
            ..Default::default()
        }
    }

    #[test]
    fn element_text_that_is_sql_references_its_tables() {
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let rows = [row(
            1,
            "db/log.xml",
            "<sql>",
            "ALTER TABLE users ADD COLUMN nickname VARCHAR(64)",
        )];
        let got = resolve(&known, &config(vec![]), &rows);
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.refs[0].table.name, "users");
        assert_eq!(got.refs[0].role, RefRole::Alters, "alters, not accesses");
        assert_eq!(got.counts.elements_with_sql, 1);
    }

    #[test]
    fn a_configured_attribute_reaches_a_table_no_statement_names() {
        // The measured gap this arm exists for: on a real repository 25 tables
        // were declared by `CREATE TABLE` and 103 named only by an attribute.
        let known = NameSet::new();
        let rows = [row(
            1,
            "db/log.xml",
            r#"<createTable tableName="orders">"#,
            "",
        )];
        let cfg = config(vec![rule(
            "tableName",
            Some("createTable"),
            Some(TableRole::Declares),
        )]);
        let got = resolve(&known, &cfg, &rows);
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.refs[0].table.name, "orders");
        assert_eq!(got.refs[0].role, RefRole::Defines);
        assert_eq!(got.minted, vec![TableKey::new(None, "orders".into())]);
    }

    #[test]
    fn an_unbound_rule_emits_a_plain_reference_whatever_the_element() {
        // A rule with no element applies anywhere; with no role it is an
        // access, because calling an unknown reference a declaration would
        // mark a table internal that the workspace may not own.
        let known = NameSet::from_table_pub_ids(["sql:orders"]);
        let rows = [row(
            1,
            "db/log.xml",
            r#"<dropTable tableName="orders">"#,
            "",
        )];
        let got = resolve(&known, &config(vec![rule("tableName", None, None)]), &rows);
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.refs[0].role, RefRole::Accesses);
    }

    #[test]
    fn a_rule_bound_to_an_element_ignores_other_elements() {
        let known = NameSet::new();
        let rows = [row(
            1,
            "db/log.xml",
            r#"<addColumn tableName="orders">"#,
            "",
        )];
        let cfg = config(vec![rule("tableName", Some("createTable"), None)]);
        assert!(resolve(&known, &cfg, &rows).refs.is_empty());
    }

    #[test]
    fn an_attribute_value_lands_on_the_identity_a_statement_would_produce() {
        // Both surfaces must reach ONE node, or a table's references split in
        // half and each half looks like the whole answer.
        let known = NameSet::from_table_pub_ids(["sql:public.orders"]);
        let rows = [
            row(
                1,
                "db/log.xml",
                r#"<createTable tableName="public.orders">"#,
                "",
            ),
            row(2, "db/log.xml", "<sql>", "SELECT id FROM public.orders"),
        ];
        let got = resolve(&known, &config(vec![rule("tableName", None, None)]), &rows);
        assert_eq!(got.refs.len(), 2);
        assert_eq!(
            got.refs[0].table, got.refs[1].table,
            "attribute and statement reach one identity: {:?}",
            got.refs
        );
        assert!(got.minted.is_empty(), "it was already known");
    }

    #[test]
    fn two_elements_reference_their_own_tables_not_the_document() {
        let known = NameSet::from_table_pub_ids(["sql:users", "sql:orders"]);
        let rows = [
            row(7, "db/log.xml", "<sql>", "SELECT id FROM users"),
            row(9, "db/log.xml", "<sql>", "SELECT id FROM orders"),
        ];
        let got = resolve(&known, &config(vec![]), &rows);
        let mut pairs: Vec<(ShortId, &str)> = got
            .refs
            .iter()
            .map(|r| (r.sym_id, r.table.name.as_str()))
            .collect();
        pairs.sort_unstable();
        assert_eq!(pairs, [(7, "users"), (9, "orders")]);
    }

    #[test]
    fn ordinary_element_text_contributes_nothing_and_reports_nothing() {
        // Measured, only 1.8% of real elements carry any text, and most of it
        // is versions, descriptions and numbers.
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let rows = [
            row(1, "db/log.xml", "<version>", "4.31.0"),
            row(2, "db/log.xml", "<comment>", "Track frozen balance for AML"),
            row(3, "db/log.xml", "<count>", "128"),
        ];
        let got = resolve(&known, &config(vec![]), &rows);
        assert!(got.refs.is_empty(), "{:?}", got.refs);
        assert_eq!(got.counts.elements_with_sql, 0);
        assert_eq!(got.counts.elements_scanned, 3, "seen, just not SQL");
    }

    #[test]
    fn an_element_outside_the_roots_contributes_nothing() {
        let known = NameSet::from_table_pub_ids(["sql:users"]);
        let rows = [
            row(1, "db/log.xml", "<sql>", "SELECT id FROM users"),
            row(2, "vendor/other.xml", "<sql>", "SELECT id FROM users"),
        ];
        let cfg = XmlSqlConfig {
            roots: vec!["db".into()],
            ..Default::default()
        };
        let got = resolve(&known, &cfg, &rows);
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.refs[0].sym_id, 1);
        assert_eq!(got.counts.elements_scanned, 1, "the other is never read");
    }

    #[test]
    fn an_attribute_written_with_spaces_still_matches() {
        // The prefilter and this matcher are sound only because the producer
        // RENDERS the signature: XML permits `tableName = "users"`, but a
        // canonical rendering admits exactly one form, so matching that form
        // cannot miss a value that parsing would have found.
        let known = NameSet::new();
        let sig = crate::xml::parse::signature(
            &crate::xml::parse::parse(r#"<createTable tableName = "users" />"#).expect("parse")[0],
        );
        let got = resolve(
            &NameSet::new(),
            &config(vec![rule("tableName", None, None)]),
            &[row(1, "db/log.xml", &sig, "")],
        );
        assert_eq!(got.refs.len(), 1, "sig was {sig:?}");
        assert_eq!(got.refs[0].table.name, "users");
        drop(known);
    }

    #[test]
    fn a_value_carrying_a_quote_survives_the_rendering_round_trip() {
        let el = &crate::xml::parse::parse(r#"<t name="a&quot;b"/>"#).expect("parse")[0];
        let sig = crate::xml::parse::signature(el);
        let got = resolve(
            &NameSet::new(),
            &config(vec![rule("name", None, None)]),
            &[row(1, "db/log.xml", &sig, "")],
        );
        assert_eq!(got.refs.len(), 1);
        assert_eq!(
            got.refs[0].table.name, "a\"b",
            "unescaped back to the source"
        );
    }

    #[test]
    fn an_undeclared_table_is_minted_once_however_many_elements_name_it() {
        let known = NameSet::new();
        let rows = [
            row(1, "db/log.xml", "<sql>", "SELECT id FROM audit_log"),
            row(2, "db/log.xml", "<sql>", "DELETE FROM audit_log"),
        ];
        let got = resolve(&known, &config(vec![]), &rows);
        assert_eq!(got.minted.len(), 1, "one identity: {:?}", got.minted);
        assert_eq!(got.refs.len(), 2, "both elements still reference it");
    }

    #[test]
    fn a_workspace_with_no_xml_elements_resolves_to_nothing() {
        // The step must be skippable without an error: an XML-only workspace
        // and a SQL-only one are both legitimate, and neither is degraded.
        let got = resolve(&NameSet::new(), &config(vec![]), &[]);
        assert!(got.refs.is_empty());
        assert!(got.minted.is_empty());
        assert_eq!(got.counts, XmlSqlCounts::default());
    }

    #[test]
    fn a_workspace_with_no_known_tables_still_bridges_by_minting() {
        // The SQL-producer-less case. Nothing declares a table, so every
        // reference mints — which is the whole point for a workspace whose
        // schema lives in XML.
        let rows = [row(1, "db/log.xml", "<sql>", "SELECT id FROM users")];
        let got = resolve(&NameSet::new(), &config(vec![]), &rows);
        assert_eq!(got.refs.len(), 1);
        assert_eq!(got.minted.len(), 1);
    }

    #[test]
    fn an_ambiguous_reference_keeps_every_candidate() {
        // Through the SAME matching rule the `.sql` producer uses, not a
        // reimplementation: an unqualified name matches every table of that
        // name, and all the matches are kept and marked rather than one being
        // picked. A bridge that silently chose would answer confidently and
        // wrongly.
        let known = NameSet::from_table_pub_ids(["sql:a.users", "sql:b.users"]);
        let rows = [row(1, "db/log.xml", "<sql>", "SELECT id FROM users")];
        let got = resolve(&known, &config(vec![]), &rows);
        assert_eq!(got.refs.len(), 2, "both kept: {:?}", got.refs);
        assert!(
            got.refs.iter().all(|r| r.grade == LinkGrade::Ambiguous),
            "and marked: {:?}",
            got.refs
        );
        assert!(got.minted.is_empty(), "both already exist");
    }

    #[test]
    fn a_substituted_attribute_value_names_no_table() {
        // An attribute carries a runtime placeholder as readily as a statement
        // does, and it must not become a table named after a variable.
        let got = resolve(
            &NameSet::new(),
            &config(vec![rule("tableName", None, None)]),
            &[row(
                1,
                "db/log.xml",
                r#"<createTable tableName="${target}">"#,
                "",
            )],
        );
        assert!(got.refs.is_empty(), "{:?}", got.refs);
        assert!(got.minted.is_empty());
    }
}
