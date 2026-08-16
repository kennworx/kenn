//! Table identity: the name set a workspace mentions, and how a reference
//! resolves against it.
//!
//! **One trait, shared by every consumer.** The `.sql` producer resolves against
//! the set it just collected; a later bridge resolves against the same set read
//! back from the store. Neither may grow its own copy of the matching rule —
//! `css/usage.rs` and `html/classes.rs` each declare their own class-registry
//! lookup, and that duplication is what this exists to avoid.
//!
//! Two rules carry the design:
//!
//! * **Matching is by name, with the schema as a discriminator.** A qualified
//!   reference matches only the identity bearing that schema; an unqualified one
//!   matches every table of that name whatever schema it carries, because
//!   engines resolve unqualified names through a runtime search path the index
//!   cannot see.
//! * **A reference mints.** A table exists in its database whether or not the
//!   workspace declares it: measured on a real repository, only 25 of 128 tables
//!   were declared by `CREATE TABLE` in `.sql`. Requiring a declaration before a
//!   reference may link would discard most of the graph on exactly the
//!   workspaces that need it.

use std::collections::BTreeMap;

use kenn_model::LinkGrade;

/// A table identity as the source spelled it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct TableKey {
    /// Present only when some statement stated one.
    pub schema: Option<String>,
    pub name: String,
}

impl TableKey {
    #[must_use]
    pub fn new(schema: Option<String>, name: String) -> Self {
        Self { schema, name }
    }
}

/// Resolving a name to the table identities a workspace knows.
///
/// The one lookup trait. A second implementation of the *same* lookup — one for
/// the producer and another for a later consumer — is the drift to avoid.
pub trait TableRegistry {
    /// Every known identity whose table name equals `name`, in a stable order.
    fn identities_named(&self, name: &str) -> Vec<TableKey>;
}

/// The identity set collected from one pass, in memory.
#[derive(Debug, Clone, Default)]
pub struct NameSet {
    by_name: BTreeMap<String, Vec<TableKey>>,
}

impl NameSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Record that the workspace mentions this identity. Idempotent.
    pub fn insert(&mut self, key: TableKey) {
        let bucket = self.by_name.entry(key.name.clone()).or_default();
        if !bucket.contains(&key) {
            bucket.push(key);
            bucket.sort();
        }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.by_name.values().map(Vec::len).sum()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.by_name.is_empty()
    }

    /// Every identity, in a stable order.
    pub fn iter(&self) -> impl Iterator<Item = &TableKey> {
        self.by_name.values().flatten()
    }

    /// Rebuild the identity set from table `pub_id`s read back from the store.
    ///
    /// This is what a barrier step resolves against: the `.sql` pass collected
    /// its set in memory and dropped it, so a later consumer recovers it from
    /// what was written. The round trip is exact because a table's `pub_id`
    /// carries **no path** — `sql:users` and `sql:public.users` decode to the
    /// identity that produced them and nothing else.
    ///
    /// Deliberately a constructor rather than a second `TableRegistry`
    /// implementation. Two store-backed lookups for one job is the
    /// `css/usage.rs` + `html/classes.rs` duplication the registry requirement
    /// exists to prevent; giving every consumer the same `NameSet` means there
    /// is nothing to diverge.
    #[must_use]
    pub fn from_table_pub_ids<I, S>(pub_ids: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let prefix = format!("{}:", kenn_model::Language::Sql.prefix());
        let mut set = Self::new();
        for id in pub_ids {
            let Some(native) = id.as_ref().strip_prefix(&prefix) else {
                continue;
            };
            let key = match native.split_once('.') {
                Some((schema, name)) if !schema.is_empty() && !name.is_empty() => {
                    TableKey::new(Some(schema.to_string()), name.to_string())
                }
                _ => TableKey::new(None, native.to_string()),
            };
            if !key.name.is_empty() {
                set.insert(key);
            }
        }
        set
    }
}

impl TableRegistry for NameSet {
    fn identities_named(&self, name: &str) -> Vec<TableKey> {
        self.by_name.get(name).cloned().unwrap_or_default()
    }
}

/// Two identity sets read as one.
///
/// A barrier step resolves against what the earlier pass wrote *and* what it has
/// itself minted so far. Without the second half, a reference cannot see a
/// sibling minted moments earlier in the same run, so the same table is minted
/// under two spellings and its references split between them — which is how a
/// `createTable` and an `ALTER TABLE users.…` naming one table ended up as two
/// identities on a real corpus.
pub struct Union<'a> {
    pub known: &'a NameSet,
    pub minted: &'a NameSet,
}

impl TableRegistry for Union<'_> {
    fn identities_named(&self, name: &str) -> Vec<TableKey> {
        let mut out = self.known.identities_named(name);
        for key in self.minted.identities_named(name) {
            if !out.contains(&key) {
                out.push(key);
            }
        }
        out
    }
}

impl NameSet {
    /// Whether this exact identity is present — schema and all.
    ///
    /// Distinct from [`identities_named`](TableRegistry::identities_named),
    /// which matches on the bare name. Minting must test the whole key: a guard
    /// that tests only the name lets one spelling satisfy it for another, and
    /// the loser's edge then finds no node and is dropped.
    #[must_use]
    pub fn contains(&self, key: &TableKey) -> bool {
        self.identities_named(&key.name).contains(key)
    }
}

/// One resolved candidate: the identity an edge points at, and how sure we are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    pub key: TableKey,
    pub grade: LinkGrade,
}

/// Resolve a reference to the one identity it names.
///
/// Always exactly one candidate, always graded `Exact`. A name the registry
/// does not know still resolves — to itself — because the reference is what
/// mints the table.
///
/// The `Vec` and the grade are both vestigial and deliberately kept: this
/// returned one `Ambiguous` candidate *per* match when an unqualified name
/// matched several identities, and that rule was removed for inventing
/// references (one reference became one edge per schema, so a count against
/// `wallets.transfers` included references belonging to `public.transfers`).
/// Collapsing the signature now would be a wider change than the rule that
/// replaced it warrants; the shape is worth keeping while the identity model
/// is still settling.
#[must_use]
pub fn resolve(reg: &dyn TableRegistry, schema: Option<&str>, name: &str) -> Vec<Resolved> {
    // A qualified reference names its schema, so it matches only that identity.
    //
    // It does NOT adopt an unqualified identity of the same name, though a bare
    // name does mean schema *unstated* rather than schema *empty*, and the two
    // are usually one table. Adoption was implemented and reverted: with no
    // record of *which* schema adopted the bare identity, a second schema
    // adopts it too, and `sales.orders` and `archive.orders` collapse into one
    // table — a worse error than splitting one, because it reports references
    // against a table that never received them. Measured on this workspace's own
    // index, which merged exactly those two.
    //
    // Unifying them properly needs promotion — the adopted identity becoming
    // `sales.orders` so the next schema sees a taken name — which rewrites a
    // `pub_id` already handed out. That is a modelling decision, tracked
    // separately. What matters first is that neither spelling LOSES a reference,
    // which is what the whole-key mint guard now guarantees.
    if let Some(s) = schema {
        return Vec::from([Resolved {
            key: TableKey::new(Some(s.to_string()), name.to_string()),
            grade: LinkGrade::Exact,
        }]);
    }
    // An unqualified reference adopts the one schema that qualifies this name,
    // and refuses to choose when several do.
    //
    // Measured on a real corpus, both halves matter. 23 of 158 identities were
    // names carrying more than one spelling; most had exactly one schema plus a
    // bare form — one table written two ways — and adopting is what keeps their
    // references in one set. But some had TWO qualified spellings beside a bare
    // one (`wallets.transfers` and `public.transfers` beside `transfers`), and
    // for those the reference simply does not say which is meant.
    //
    // Fanning out to every candidate was the previous rule. It never invents a
    // *table*, but it does invent references: 83 bare `transfers` references
    // become 166 edges, and a reader counting references to `wallets.transfers`
    // is counting some that belong to `public.transfers`. A bare identity says
    // the true thing instead — referenced without a schema, and not guessed.
    let qualified: Vec<TableKey> = reg
        .identities_named(name)
        .into_iter()
        .filter(|k| k.schema.is_some())
        .collect();
    let key = match qualified.as_slice() {
        [only] => only.clone(),
        // None, or more than one. Either way the honest identity is the name as
        // the reference wrote it; when nothing of that name is known at all,
        // this is also what mints it.
        _ => TableKey::new(None, name.to_string()),
    };
    Vec::from([Resolved {
        key,
        grade: LinkGrade::Exact,
    }])
}

/// Recover the table identities the `.sql` pass wrote, from its table nodes.
///
/// Two views of the same set, because resolution and emission each need one:
/// the [`NameSet`] answers "does a table by this name exist, and how exactly
/// does it match", and the map answers "which node is it" — so a reference
/// reaches the node that exists rather than a duplicate of it.
///
/// Pure, and separated for that reason: it is the only part of this module that
/// decides anything, and the only part testable without a store.
#[must_use]
pub fn known_tables(
    symbols: &[kenn_store::SymbolRow],
) -> (
    NameSet,
    std::collections::BTreeMap<TableKey, kenn_model::ShortId>,
) {
    let table_kind = kenn_model::Kind::SqlTable.db_name();
    let mut ids: std::collections::BTreeMap<TableKey, kenn_model::ShortId> = BTreeMap::new();
    let mut pub_ids: Vec<String> = Vec::new();
    for s in symbols.iter().filter(|s| s.kind == table_kind) {
        pub_ids.push(s.pub_id.clone());
        if let Some(key) = key_of(&s.pub_id) {
            ids.insert(key, s.id);
        }
    }
    (NameSet::from_table_pub_ids(&pub_ids), ids)
}

/// Decode a table `pub_id` back into its identity. Exact: a table id carries no
/// path, so the only parts are an optional schema and the name.
pub(crate) fn key_of(pub_id: &str) -> Option<TableKey> {
    let native = pub_id.strip_prefix(&format!("{}:", kenn_model::Language::Sql.prefix()))?;
    Some(match native.split_once('.') {
        Some((schema, name)) if !schema.is_empty() && !name.is_empty() => {
            TableKey::new(Some(schema.to_owned()), name.to_owned())
        }
        _ => TableKey::new(None, native.to_owned()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set(keys: &[(Option<&str>, &str)]) -> NameSet {
        let mut s = NameSet::new();
        for (schema, name) in keys {
            s.insert(TableKey::new(
                schema.map(ToString::to_string),
                (*name).to_string(),
            ));
        }
        s
    }

    /// A qualified reference does NOT adopt a bare identity of the same name,
    /// and this test exists to keep it that way until promotion is modelled.
    ///
    /// Adoption looks obviously right — a bare name means schema unstated, and
    /// the two are usually one table — and was implemented. It over-merges:
    /// nothing records *which* schema adopted the bare identity, so the next
    /// schema adopts it too. On this workspace's own index that collapsed
    /// `sales.orders` and `archive.orders` into one table, which reports
    /// references against a table that never received them.
    ///
    /// The unit test that was supposed to catch that passed a *empty* registry,
    /// so the adoption branch never ran — CLAUDE.md §9's "suspect the fixture"
    /// again. Hence the bare identity in `known` here: without it this test
    /// cannot fail.
    #[test]
    fn a_qualified_reference_does_not_absorb_a_bare_identity() {
        let known = set(&[(None, "orders")]);
        let got = resolve(&known, Some("sales"), "orders");
        assert_eq!(
            got,
            vec![Resolved {
                key: TableKey::new(Some("sales".into()), "orders".into()),
                grade: LinkGrade::Exact,
            }],
            "sales.orders keeps its own identity; merging is unsafe without promotion"
        );
    }

    /// The case that made adoption unsafe, stated directly: with a bare identity
    /// already present, two schemas must still reach two identities.
    #[test]
    fn two_schemas_do_not_collapse_onto_a_bare_identity() {
        let known = set(&[(None, "orders")]);
        let sales = resolve(&known, Some("sales"), "orders");
        let archive = resolve(&known, Some("archive"), "orders");
        assert_ne!(
            sales[0].key, archive[0].key,
            "two schemas, two tables — even when a bare `orders` exists"
        );
    }

    /// The asymmetry that keeps this from being the WORSE bug: two schemas can
    /// each hold an `events`, and merging them would report references against a
    /// table that never received them.
    #[test]
    fn two_qualified_identities_never_merge() {
        let known = set(&[(Some("sales"), "orders")]);
        let got = resolve(&known, Some("archive"), "orders");
        assert_eq!(
            got,
            vec![Resolved {
                key: TableKey::new(Some("archive".into()), "orders".into()),
                grade: LinkGrade::Exact,
            }],
            "archive.orders is its own table, not sales.orders"
        );
    }

    /// Adoption must not fire when the qualified identity is already the known
    /// one — otherwise a workspace that consistently qualifies would resolve
    /// onto a bare key nothing minted.
    #[test]
    fn a_qualified_reference_matching_a_qualified_identity_is_unchanged() {
        let known = set(&[(Some("sales"), "orders")]);
        let got = resolve(&known, Some("sales"), "orders");
        assert_eq!(
            got,
            vec![Resolved {
                key: TableKey::new(Some("sales".into()), "orders".into()),
                grade: LinkGrade::Exact,
            }]
        );
    }

    /// Nothing known: the reference still mints itself, qualified as written.
    #[test]
    fn a_qualified_reference_to_an_unknown_table_mints_itself() {
        let known = NameSet::new();
        let got = resolve(&known, Some("sales"), "orders");
        assert_eq!(
            got,
            vec![Resolved {
                key: TableKey::new(Some("sales".into()), "orders".into()),
                grade: LinkGrade::Exact,
            }]
        );
    }

    #[test]
    fn a_qualified_reference_does_not_match_another_schema() {
        let reg = set(&[(None, "users"), (Some("analytics"), "users")]);
        let got = resolve(&reg, Some("analytics"), "users");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key.schema.as_deref(), Some("analytics"));
        assert_eq!(got[0].grade, LinkGrade::Exact);
    }

    #[test]
    fn an_unqualified_reference_reaches_a_schema_qualified_table() {
        // The common real shape: migrations qualify, queries do not.
        let reg = set(&[(Some("analytics"), "users")]);
        let got = resolve(&reg, None, "users");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key.schema.as_deref(), Some("analytics"));
        assert_eq!(got[0].grade, LinkGrade::Exact);
    }

    #[test]
    fn an_unqualified_reference_adopts_the_one_schema_that_qualifies_it() {
        // Was `an_unqualified_reference_matching_two_keeps_both`, which asserted
        // a fan-out to every candidate. One table written two ways is the common
        // case — measured, most split names on a real corpus were exactly this —
        // and fanning out split its references between the two spellings.
        let reg = set(&[(None, "users"), (Some("analytics"), "users")]);
        let got = resolve(&reg, None, "users");
        assert_eq!(
            got,
            vec![Resolved {
                key: TableKey::new(Some("analytics".into()), "users".into()),
                grade: LinkGrade::Exact,
            }],
            "one schema qualifies `users`, so the bare reference means that one"
        );
    }

    /// And the half that keeps adoption honest: when two schemas qualify the
    /// same name, a bare reference does not pick one.
    ///
    /// Fanning out would not invent a table but it would invent references —
    /// 83 bare `transfers` references on the measured corpus becoming 166 edges,
    /// so a count against `wallets.transfers` includes some that belong to
    /// `public.transfers`. The bare identity says the true thing: referenced
    /// without a schema, and not guessed.
    #[test]
    fn an_unqualified_reference_refuses_to_choose_between_two_schemas() {
        let reg = set(&[
            (Some("wallets"), "transfers"),
            (Some("public"), "transfers"),
        ]);
        let got = resolve(&reg, None, "transfers");
        assert_eq!(
            got,
            vec![Resolved {
                key: TableKey::new(None, "transfers".into()),
                grade: LinkGrade::Exact,
            }],
            "two schemas qualify it, so the bare name stands for itself"
        );
    }

    #[test]
    fn a_reference_to_an_unknown_table_mints_it_rather_than_dropping() {
        // The rule that decides whether a workspace whose schema lives
        // elsewhere gets a graph at all.
        let reg = set(&[]);
        let got = resolve(&reg, None, "users");
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].key, TableKey::new(None, "users".to_string()));
        assert_eq!(got[0].grade, LinkGrade::Exact);
    }

    #[test]
    fn a_table_pub_id_round_trips_back_into_its_identity() {
        // What a barrier step depends on: the `.sql` pass's identity set is
        // recoverable from what it wrote, because a table id carries no path.
        let set = NameSet::from_table_pub_ids(["sql:users", "sql:public.users", "sql:orders"]);
        assert_eq!(set.len(), 3);
        assert_eq!(
            set.identities_named("users"),
            vec![
                TableKey::new(None, "users".into()),
                TableKey::new(Some("public".into()), "users".into()),
            ],
            "qualified and unqualified stay distinct identities"
        );
    }

    #[test]
    fn a_rebuilt_set_resolves_exactly_as_the_collected_one_did() {
        // The property that matters: a consumer resolving against the rebuilt
        // set gets what the `.sql` pass would have got. If these diverge, the
        // same reference grades differently depending on which pass saw it.
        let collected = set(&[(None, "users"), (Some("analytics"), "users")]);
        let rebuilt = NameSet::from_table_pub_ids(["sql:users", "sql:analytics.users"]);
        assert_eq!(
            resolve(&collected, None, "users"),
            resolve(&rebuilt, None, "users")
        );
        assert_eq!(
            resolve(&collected, Some("analytics"), "users"),
            resolve(&rebuilt, Some("analytics"), "users")
        );
    }

    #[test]
    fn a_pub_id_of_another_language_is_ignored() {
        // The scan is filtered by kind, but the decoder must not depend on that
        // — a `rs:` id decoding into a table named `rs:foo` would be a phantom.
        let set = NameSet::from_table_pub_ids(["rs:kenn::users", "sql:users"]);
        assert_eq!(set.len(), 1);
        assert_eq!(set.identities_named("users").len(), 1);
    }

    #[test]
    fn the_name_set_is_stable_and_deduplicated() {
        let mut s = NameSet::new();
        for _ in 0..3 {
            s.insert(TableKey::new(None, "users".to_string()));
        }
        s.insert(TableKey::new(Some("a".into()), "users".to_string()));
        assert_eq!(s.len(), 2);
        assert_eq!(
            s.identities_named("users"),
            s.identities_named("users"),
            "order is stable across calls"
        );
    }

    fn sym(id: u32, pub_id: &str, kind: &str) -> kenn_store::SymbolRow {
        kenn_store::SymbolRow {
            id,
            pub_id: pub_id.to_owned(),
            language: "sql".to_owned(),
            pkg_id: 0,
            kind: kind.to_owned(),
            name: String::new(),
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
            enclosing_sym_id: 0,
        }
    }

    #[test]
    fn a_table_keeps_the_id_of_the_node_that_already_exists() {
        // Miss the id and a reference mints a duplicate of a table the `.sql`
        // pass already wrote, splitting one table's references across two nodes.
        let (_, ids) = known_tables(&[
            sym(10, "sql:users", "sql_table"),
            sym(11, "sql:public.orders", "sql_table"),
        ]);
        assert_eq!(ids.get(&TableKey::new(None, "users".into())), Some(&10));
        assert_eq!(
            ids.get(&TableKey::new(Some("public".into()), "orders".into())),
            Some(&11)
        );
    }

    #[test]
    fn a_sql_file_and_statement_node_are_not_read_as_tables() {
        // The kind filter's actual job, and it is not about language. File and
        // statement nodes carry the same `sql:` prefix a table does, so
        // `key_of` decodes them rather than rejecting them: `sql:mig/001.sql`
        // splits at the extension into schema `mig/001`, table `sql`. Without
        // the filter a table named `sql` joins the known set, and any literal
        // saying `FROM sql` resolves to a migration file.
        let (known, ids) = known_tables(&[
            sym(1, "sql:mig/001_init.sql", "document"),
            sym(2, "sql:mig/001_init.sql#0", "sql_statement"),
            sym(3, "sql:users", "sql_table"),
        ]);
        assert_eq!(ids.len(), 1, "only the table: {ids:?}");
        assert!(
            known.identities_named("sql").is_empty(),
            "a file extension is not a table name"
        );
    }

    #[test]
    fn a_workspace_that_declares_no_tables_yields_an_empty_set() {
        // The case the whole step most needs to serve: schema owned elsewhere,
        // so every table a literal names gets minted.
        let (known, ids) = known_tables(&[sym(1, "rs:kenn::main", "function")]);
        assert!(ids.is_empty());
        assert!(known.identities_named("users").is_empty());
    }

    #[test]
    fn a_table_pub_id_decodes_to_the_identity_that_made_it() {
        assert_eq!(
            key_of("sql:users"),
            Some(TableKey::new(None, "users".into()))
        );
        assert_eq!(
            key_of("sql:public.users"),
            Some(TableKey::new(Some("public".into()), "users".into()))
        );
        assert_eq!(
            key_of("rs:kenn::users"),
            None,
            "another language is not a table"
        );
    }
}
