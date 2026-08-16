//! SQL text → statements and the table references they make.
//!
//! **Pure.** No file IO, no store access, no knowledge of where the text came
//! from. The `.sql` producer is one consumer; a later bridge hands this module
//! SQL lifted out of markup or source, and must reuse it rather than grow a
//! second extractor — the class-registry lookup is already declared twice in
//! this crate (`css/usage.rs` and `html/classes.rs`) and that is the drift this
//! constraint exists to prevent.
//!
//! Two behaviours here are measured rather than assumed:
//!
//! * **Dialect recovery beats dialect selection.** On a parse failure every
//!   remaining dialect is retried in a fixed order. Over a fixed statement set
//!   the permissive dialect alone scored 12/16 and the sweep 15/16 — better
//!   than any single dialect (the best scored 13/16). Narrowing the retry set by
//!   syntax markers misroutes: Oracle's `(+)` outer-join operator parses only
//!   under the SQL Server dialect.
//! * **Whole text first, split only on failure.** The failure mode splitting
//!   first invites is real: it shears a procedure or anonymous block at its
//!   internal separators, and the leading fragment — carrying the block's
//!   opening statement — fails, taking its table references with it. On the
//!   spike's hand-written statements that cost a third of one file's tables.
//!
//!   **It did not reproduce on real corpora.** Re-measured over two Postgres
//!   repositories (21 and 54 `.sql` files), the two orderings recovered
//!   *identical* table counts — 159/159 and 228/228, with no file favouring
//!   either. Whole-first remains the right default because it is cheaper (one
//!   parse instead of one per piece) and because the shearing hazard is real
//!   where those blocks occur, but the coverage advantage is unproven outside
//!   the spike. Neither corpus is Oracle or T-SQL, where the hazard is most
//!   likely; that case is still unmeasured. `audit_strategy_on_a_real_corpus`
//!   re-runs this.
//!
//!   What the same audit did confirm: the tokenizer stays more permissive than
//!   the parser. In both repositories 9 files failed a whole-file parse and
//!   still tokenized into pieces, which is the property the split tier depends
//!   on.

use std::ops::Range;

use sqlparser::ast::{ObjectName, Statement};
use sqlparser::dialect::dialect_from_str;
use sqlparser::parser::Parser;
use sqlparser::tokenizer::{Token, Tokenizer};

/// What a statement does to a table it names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefRole {
    /// Brings the table into being (`CREATE TABLE`). Marks it internal.
    Defines,
    /// Changes an existing table's definition (`ALTER`, `DROP`).
    Alters,
    /// Reads or writes the table's data.
    Accesses,
}

/// One table named by a statement, as the source spelled it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRef {
    /// Present only when the source stated one — never inferred or defaulted.
    pub schema: Option<String>,
    pub name: String,
    pub role: RefRole,
}

/// One parsed statement and the tables it names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedStatement {
    /// Byte range in the input this statement occupies.
    pub span: Range<usize>,
    pub refs: Vec<TableRef>,
    /// What the statement *does*, as SQL spells it — `SELECT FROM`,
    /// `ALTER TABLE`, `DROP VIEW`. `None` when the parser produced a kind this
    /// module does not map.
    ///
    /// Kept because the role cannot stand in for it: `UPDATE` and `SELECT` are
    /// both `RefRole::Accesses`, so without this they are indistinguishable
    /// downstream and a statement's signature could not say which it was.
    pub verb: Option<&'static str>,
}

/// Result of extracting from one blob of SQL text.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Extraction {
    pub statements: Vec<ParsedStatement>,
    /// Pieces no dialect could parse. Signal for a `.sql` file; noise for text
    /// lifted out of markup, where most content is not SQL at all.
    pub unparsed: usize,
}

/// Dialects tried in a fixed order so recovery is deterministic. The permissive
/// dialect leads when no primary is configured.
const SWEEP: &[&str] = &[
    "generic",
    "postgresql",
    "mysql",
    "mssql",
    "oracle",
    "sqlite",
    "ansi",
    "snowflake",
    "bigquery",
    "clickhouse",
    "duckdb",
    "hive",
    "redshift",
    "databricks",
];

/// A dialect name the parser does not provide.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownDialect(pub String);

/// Validate a configured dialect name up front, so an unrecognized one is a
/// configuration error rather than a silent fallback to the permissive parse.
///
/// # Errors
/// Returns [`UnknownDialect`] when the parser provides no dialect of that name.
pub fn validate_dialect(name: &str) -> Result<(), UnknownDialect> {
    if dialect_from_str(name).is_some() {
        Ok(())
    } else {
        Err(UnknownDialect(name.to_string()))
    }
}

/// The sweep order for a given primary: the primary first, then the rest.
fn sweep_order(primary: Option<&str>) -> Vec<&str> {
    let mut order: Vec<&str> = Vec::with_capacity(SWEEP.len());
    if let Some(p) = primary {
        if SWEEP.contains(&p) {
            order.push(
                SWEEP
                    .iter()
                    .find(|d| **d == p)
                    .copied()
                    .unwrap_or("generic"),
            );
        }
    }
    for d in SWEEP {
        if !order.contains(d) {
            order.push(d);
        }
    }
    order
}

/// Parse one blob, retrying every remaining dialect on failure.
fn parse_with_sweep(text: &str, primary: Option<&str>) -> Option<Vec<Statement>> {
    for name in sweep_order(primary) {
        let Some(dialect) = dialect_from_str(name) else {
            continue;
        };
        if let Ok(stmts) = Parser::parse_sql(dialect.as_ref(), text) {
            return Some(stmts);
        }
    }
    None
}

/// Strip dialect quoting from one identifier part.
///
/// Parsers return identifiers with their quoting intact, so `` `users` `` and
/// `[users]` and `"users"` would otherwise mint three identities for one table —
/// and a quoted reference would fail to match a bare declaration.
fn normalize_ident(raw: &str) -> String {
    raw.trim()
        .trim_start_matches(['`', '[', '"'])
        .trim_end_matches(['`', ']', '"'])
        .to_string()
}

/// Split a parser object name into an optional schema and a table name.
///
/// Only the last two parts matter: a three-part `db.schema.table` keeps
/// `schema` as the qualifier, since that is what a reference elsewhere in the
/// workspace would spell.
fn split_object_name(name: &ObjectName) -> Option<(Option<String>, String)> {
    let parts: Vec<String> = name
        .0
        .iter()
        .map(|p| normalize_ident(&p.to_string()))
        .filter(|p| !p.is_empty())
        .collect();
    last_two(&parts)
}

/// `(schema, table)` from dotted parts: the last part is the table, the one
/// before it (when present) the schema. A three-part `db.schema.table` keeps
/// `schema`, since that is what a reference elsewhere would spell.
fn last_two(parts: &[String]) -> Option<(Option<String>, String)> {
    match parts {
        [] => None,
        [only] => Some((None, only.clone())),
        [.., schema, name] => Some((Some(schema.clone()), name.clone())),
    }
}

/// Statement keywords that can introduce a table reference.
///
/// DDL is here on purpose. Migrations are routinely *written in code* — C#
/// `FluentMigrator` calls `Execute.Sql("CREATE TABLE …")`, knex and kysely do
/// the same in TypeScript, and kenn's own schema is a Rust `const`. Measured across
/// three workspaces, 30, 6 and 57 of the code→table edges were `Defines` or
/// `Alters`; narrowing this to the read/write verbs would drop all of them.
/// Every other statement keyword is deliberately absent, and each omission was
/// settled by re-indexing a real workspace and diffing the resulting edge set —
/// not by argument. Two rounds of that overturned two decisions.
///
/// Omitted because they are **subsumed by another verb in the same text**:
///
/// * `with` — a `WITH` clause always resolves to a `SELECT`/`INSERT`/`UPDATE`/
///   `DELETE`.
/// * `merge` — `MERGE … WHEN MATCHED THEN UPDATE`/`INSERT` carries its own.
/// * `replace` — `REPLACE(str, from, to)` is a scalar function inside a
///   `SELECT`, and `CREATE OR REPLACE` carries `CREATE`. Only `MySQL`'s
///   standalone `REPLACE INTO` is lost, and it occurs nowhere in the corpus.
///
/// Omitted because they are **subsumed by another statement on the same
/// symbol** — the subtler case, and the one reasoning got wrong:
///
/// * `lock`, `analyze` — each names a table with no other verb, so dropping the
///   keyword really does drop the statement, and the audit duly reports the
///   literal as a miss. But a symbol that locks or analyses a table also reads
///   or writes it; that is *why* it locked it. Measured both ways on a workspace
///   with `LOCK TABLE` in 8 files: **169 distinct edges either way, byte-for-byte
///   identical**. The literal is missed; the edge is not.
///
/// That distinction is the thing to keep hold of: what matters is whether an
/// *edge* disappears, not whether a literal is skipped. `audit_prefilter_false_negatives`
/// reports the latter and will overstate the loss. Re-index and diff the edge
/// set before adding a keyword back.
///
/// Since "with", "replace", "lock" and "analyze" are all common English, every
/// one of these omissions is also a large share of the sweeps avoided.
const STATEMENT_KEYWORDS: &[&str] = &[
    "select", "insert", "update", "delete", "upsert", "create", "alter", "drop", "truncate",
];

/// A cheap pre-filter: could this text possibly be SQL?
///
/// Text lifted out of source or markup is overwhelmingly *not* SQL — measured
/// on a self-index, 3320 bodies carried literals and 154 named a table. Every
/// one of the other 97% otherwise runs the full 14-dialect sweep before failing,
/// and that sweep is what the pass actually spends its time on: the smallest of
/// the three measured workspaces was also the slowest, because cost tracks
/// failed parses rather than body count.
///
/// Whole-word and case-insensitive. A false positive is free — it only means
/// the sweep runs, which is what would have happened anyway — so the check is
/// deliberately generous. Only a false *negative* would cost recall, which is
/// why every verb that can name a table is listed.
///
/// Not applied to `.sql` files: their content is SQL by construction, and a file
/// whose first statement is a `SET` or a `\set` would be dropped whole.
#[must_use]
pub fn looks_like_sql(text: &str) -> bool {
    text.split(|c: char| !c.is_ascii_alphanumeric() && c != '_')
        .any(|word| {
            STATEMENT_KEYWORDS
                .iter()
                .any(|k| word.len() == k.len() && word.eq_ignore_ascii_case(k))
        })
}

/// How SQL spells what a statement does, for its signature.
///
/// A standalone table rather than a branch inside `refs_of`, which already
/// carries enough complexity — and a mapping table is easier to check against
/// the grammar when it reads as one list.
///
/// Only table-bearing kinds appear. sqlparser 0.62 has 135 `Statement`
/// variants, but a statement naming no table produces no node, so the rest are
/// unreachable from here and `None` is the honest answer for them.
///
/// `Drop` renders its `object_type`, because one variant covers table, view and
/// index and signing a `DROP VIEW` as `DROP TABLE` would be a lie about the
/// schema.
/// No `match_same_arms` allow is needed, and that is a property worth keeping:
/// every arm renders a *distinct* verb, so two kinds sharing a rendering would
/// be a real collapse rather than a lint to silence. `#[expect]` was added here
/// on the assumption it would be required and clippy reported it unfulfilled.
#[must_use]
pub fn verb_of(stmt: &Statement) -> Option<&'static str> {
    use sqlparser::ast::ObjectType;
    Some(match stmt {
        Statement::Query(_) => "SELECT FROM",
        Statement::Insert(_) => "INSERT INTO",
        Statement::Update { .. } => "UPDATE",
        Statement::Delete(_) => "DELETE FROM",
        Statement::CreateTable(_) => "CREATE TABLE",
        Statement::AlterTable(_) => "ALTER TABLE",
        Statement::Drop { object_type, .. } => match object_type {
            ObjectType::Table => "DROP TABLE",
            ObjectType::View => "DROP VIEW",
            ObjectType::MaterializedView => "DROP MATERIALIZED VIEW",
            ObjectType::Index => "DROP INDEX",
            ObjectType::Schema => "DROP SCHEMA",
            ObjectType::Sequence => "DROP SEQUENCE",
            ObjectType::Role => "DROP ROLE",
            ObjectType::Database => "DROP DATABASE",
            _ => "DROP",
        },
        Statement::Truncate { .. } => "TRUNCATE TABLE",
        Statement::Merge { .. } => "MERGE INTO",
        Statement::RenameTable(_) => "RENAME TABLE",
        Statement::CreateView { .. } => "CREATE VIEW",
        Statement::CreateIndex(_) => "CREATE INDEX",
        Statement::CreateSchema { .. } => "CREATE SCHEMA",
        Statement::Analyze { .. } => "ANALYZE",
        Statement::Copy { .. } => "COPY",
        Statement::Comment { .. } => "COMMENT ON",
        Statement::Grant { .. } => "GRANT",
        Statement::Revoke { .. } => "REVOKE",
        _ => return None,
    })
}

/// A table name written somewhere that is not a statement — an XML attribute —
/// normalized to the identity a statement naming the same table would produce.
///
/// Exposed rather than reimplemented at the call site: a quoted or
/// schema-qualified attribute value has to land on the SAME identity as the
/// `CREATE TABLE` that declared it, or the graph grows two nodes for one table
/// and each carries half the references. That equivalence is exactly what
/// [`normalize_ident`] and the dotted split already decide.
///
/// `None` when the value names nothing usable — empty, or a runtime
/// substitution, which an attribute can carry as readily as a statement.
#[must_use]
pub fn normalize_table_name(raw: &str) -> Option<crate::sql::registry::TableKey> {
    let (schema, name) = parse_relation(raw)?;
    if name.is_empty() || is_substituted(&name) || schema.as_deref().is_some_and(is_substituted) {
        return None;
    }
    Some(crate::sql::registry::TableKey::new(schema, name))
}

/// True when a `DROP`'s object type is a table or something that stands in for
/// one in a query.
///
/// Views are included deliberately: code selects from a view exactly as it does
/// from a table, and the reference is the same fact either way. Everything else
/// a `DROP` can name — indexes, roles, schemas, sequences, types, policies — is
/// a different kind of object that merely shares the statement.
const fn is_table_like(object_type: sqlparser::ast::ObjectType) -> bool {
    use sqlparser::ast::ObjectType;
    matches!(
        object_type,
        ObjectType::Table | ObjectType::View | ObjectType::MaterializedView
    )
}

/// Names that look like relations but are function invocations —
/// `FROM generate_series(1, 2000)`, `FROM tenant.uses_key($1, $2)`.
///
/// `TableFactor::Table` carries `args`, which is `Some` exactly when the source
/// wrote a call rather than a name. The relation walk cannot see that
/// distinction (it is handed the `ObjectName` alone), so the names are
/// collected here and subtracted from it.
///
/// Both examples are from real code. A set-returning function in `FROM` is
/// ordinary SQL, not an edge case.
fn table_function_names(stmt: &Statement) -> std::collections::HashSet<String> {
    use sqlparser::ast::{TableFactor, Visit, Visitor};

    struct Collect(std::collections::HashSet<String>);
    impl Visitor for Collect {
        type Break = ();
        fn pre_visit_table_factor(&mut self, tf: &TableFactor) -> core::ops::ControlFlow<()> {
            if let TableFactor::Table {
                name,
                args: Some(_),
                ..
            } = tf
            {
                self.0.insert(name.to_string());
            }
            core::ops::ControlFlow::Continue(())
        }
    }
    let mut c = Collect(std::collections::HashSet::new());
    let walked = stmt.visit(&mut c);
    debug_assert!(
        matches!(walked, core::ops::ControlFlow::Continue(())),
        "the visitor never breaks — it only collects"
    );
    c.0
}

/// True when a name is not statically knowable — a runtime substitution rather
/// than a table. Such a reference yields no edge and no node.
///
/// A bare brace counts, not only `${`. Measured on a C# service: the `$` of an
/// interpolated string sits on the string *prefix*, which the literal scanner
/// consumes, so `$"… FROM {schema.Value}.orders"` reaches here as
/// `{schema.Value}` with nothing to key on but the brace. Python f-strings and
/// `str.format` templates arrive the same way. Left in, the placeholder mints a
/// table node named after a variable — an identity that exists in no database.
fn is_substituted(raw: &str) -> bool {
    let t = raw.trim();
    t.contains('{') || t.contains('}') || t.starts_with(':') || t.starts_with('@')
}

/// Byte offsets of every top-level statement boundary, from the tokenizer.
///
/// The tokenizer is more permissive than the parser: input the parser rejects
/// still tokenizes, which is what makes the split tier able to recover anything
/// at all.
fn statement_pieces(text: &str, primary: Option<&str>) -> Vec<Range<usize>> {
    let dialect = sweep_order(primary)
        .first()
        .and_then(|n| dialect_from_str(n))
        .unwrap_or_else(|| dialect_from_str("generic").expect("generic dialect exists"));
    let whole = || -> Vec<Range<usize>> { core::iter::once(0..text.len()).collect() };
    let Ok(tokens) = Tokenizer::new(dialect.as_ref(), text).tokenize_with_location() else {
        return whole();
    };
    // Line/column → byte offset.
    let mut line_starts = vec![0usize];
    for (i, b) in text.bytes().enumerate() {
        if b == b'\n' {
            line_starts.push(i + 1);
        }
    }
    let offset = |line: u64, col: u64| -> usize {
        let li = usize::try_from(line)
            .unwrap_or(usize::MAX)
            .saturating_sub(1);
        let col = usize::try_from(col).unwrap_or(1).saturating_sub(1);
        line_starts
            .get(li)
            .map_or(text.len(), |s| s.saturating_add(col).min(text.len()))
    };

    // `get` rather than `[..]` throughout: a slice landing inside a multi-byte
    // character would panic, and SQL comments carry non-ASCII in the wild.
    let mut pieces = Vec::new();
    let mut start = 0usize;
    for t in &tokens {
        if t.token == Token::SemiColon {
            let end = offset(t.span.start.line, t.span.start.column);
            if end > start && text.get(start..end).is_some_and(|s| !s.trim().is_empty()) {
                pieces.push(start..end);
            }
            start = end.saturating_add(1).min(text.len());
        }
    }
    if text.get(start..).is_some_and(|s| !s.trim().is_empty()) {
        pieces.push(start..text.len());
    }
    pieces
}

/// Extract statements and their table references from one blob of SQL.
///
/// `primary` is the configured dialect name, already validated. `None` means
/// the permissive cross-dialect parse, which is the normal case.
#[must_use]
pub fn extract(text: &str, primary: Option<&str>) -> Extraction {
    if text.trim().is_empty() {
        return Extraction::default();
    }
    let pieces = statement_pieces(text, primary);

    // Tier 1 — the whole blob. Keeps block bodies intact.
    if let Some(stmts) = parse_with_sweep(text, primary) {
        let spans: Vec<Range<usize>> = if stmts.len() == pieces.len() {
            pieces
        } else {
            // A block swallowed its own separators, so pieces and statements
            // disagree. Rather than attribute a statement to the wrong bytes,
            // every statement carries the whole blob's span.
            vec![0..text.len(); stmts.len()]
        };
        let statements = stmts
            .iter()
            .zip(spans)
            .filter_map(|(s, span)| {
                let refs = refs_of(s);
                (!refs.is_empty()).then_some(ParsedStatement {
                    span,
                    refs,
                    verb: verb_of(s),
                })
            })
            .collect();
        return Extraction {
            statements,
            unparsed: 0,
        };
    }

    // Tier 2 — split and parse each piece. A piece that still fails is dropped
    // and counted; one that parses but names nothing (a bare block terminator)
    // contributes no node.
    let mut statements = Vec::new();
    let mut unparsed = 0usize;
    for piece in pieces {
        let Some(src) = text.get(piece.clone()) else {
            continue;
        };
        match parse_with_sweep(src, primary) {
            Some(stmts) => {
                let refs: Vec<TableRef> = stmts.iter().flat_map(refs_of).collect();
                if !refs.is_empty() {
                    // A split piece can hold more than one statement; the first
                    // is what the piece is *about*, and it is what the span
                    // starts at.
                    let verb = stmts.first().and_then(verb_of);
                    statements.push(ParsedStatement {
                        span: piece,
                        refs,
                        verb,
                    });
                }
            }
            None => unparsed += 1,
        }
    }
    Extraction {
        statements,
        unparsed,
    }
}

/// Table references made by one statement, classified by what it does to each.
fn push_ref(out: &mut Vec<TableRef>, schema: Option<String>, name: String, role: RefRole) {
    // The schema is checked too, not just the name: a dotted placeholder splits
    // across both (`{schema.Value}` → schema `{schema`, name `Value}`), so
    // checking one half lets the other through.
    if name.is_empty() || is_substituted(&name) || schema.as_deref().is_some_and(is_substituted) {
        return;
    }
    if !out
        .iter()
        .any(|r| r.name == name && r.schema == schema && r.role == role)
    {
        out.push(TableRef { schema, name, role });
    }
}

fn refs_of(stmt: &Statement) -> Vec<TableRef> {
    let mut out: Vec<TableRef> = Vec::new();

    match stmt {
        Statement::CreateTable(ct) => {
            if let Some((schema, name)) = split_object_name(&ct.name) {
                push_ref(&mut out, schema, name, RefRole::Defines);
            }
        }
        Statement::AlterTable(at) => {
            if let Some((schema, n)) = split_object_name(&at.name) {
                push_ref(&mut out, schema, n, RefRole::Alters);
            }
        }
        // `DROP` names whatever kind it was told to. Taking every name as a
        // table is how index, role, schema and type names became tables:
        // measured on one real workspace, 9 of 74 "tables" were index or
        // constraint names from `DROP INDEX`, and 3 more were roles.
        Statement::Drop {
            object_type, names, ..
        } if is_table_like(*object_type) => {
            for name in names {
                if let Some((schema, n)) = split_object_name(name) {
                    push_ref(&mut out, schema, n, RefRole::Alters);
                }
            }
        }
        _ => {}
    }

    // Everything the statement reads or writes, including a create-as-select's
    // sources — so one statement can carry both a `Defines` and an `Accesses`.
    let defined: Vec<(Option<String>, String)> = out
        .iter()
        .filter(|r| r.role == RefRole::Defines)
        .map(|r| (r.schema.clone(), r.name.clone()))
        .collect();
    let mut relations: Vec<String> = Vec::new();
    let stmts = vec![stmt.clone()];
    let walked = sqlparser::ast::visit_relations(&stmts, |name| {
        relations.push(name.to_string());
        core::ops::ControlFlow::<()>::Continue(())
    });
    debug_assert!(
        matches!(walked, core::ops::ControlFlow::Continue(())),
        "the visitor never breaks — it only collects"
    );
    // Names a `WITH` clause introduces are query-local, not tables. The
    // relation visitor cannot tell them apart — `WITH cnt AS (…) SELECT FROM
    // cnt` reads exactly like a table read — so a CTE would mint a table node
    // and carry an edge to something that exists only for the length of one
    // statement. Found by scanning real code: kenn's own recursive-CTE query
    // minted a `cnt` table.
    let cte_names = cte_names(stmt);
    let fn_names = table_function_names(stmt);
    for raw in relations {
        // A function call in `FROM` reaches the relation walk as a bare name,
        // indistinguishable from a table. Matched on the raw spelling, before
        // normalization, because that is what both sides carry.
        if fn_names.contains(&raw) {
            continue;
        }
        let Some(parsed) = parse_relation(&raw) else {
            continue;
        };
        // Unqualified only: `WITH x AS (…)` shadows a bare `x`, never a
        // schema-qualified `s.x`, which always denotes the real table.
        if parsed.0.is_none() && cte_names.contains(&parsed.1) {
            continue;
        }
        if defined.contains(&parsed) {
            continue;
        }
        let already_altered = out
            .iter()
            .any(|r| r.role == RefRole::Alters && r.schema == parsed.0 && r.name == parsed.1);
        if already_altered {
            continue;
        }
        push_ref(&mut out, parsed.0, parsed.1, RefRole::Accesses);
    }
    out
}

/// Every name a `WITH` clause introduces anywhere in `stmt`, at any nesting
/// depth — a subquery may carry its own CTEs, and they shadow just as locally.
fn cte_names(stmt: &Statement) -> std::collections::HashSet<String> {
    use sqlparser::ast::{Query, Visit, Visitor};

    struct Collect(std::collections::HashSet<String>);
    impl Visitor for Collect {
        type Break = ();
        fn pre_visit_query(&mut self, query: &Query) -> core::ops::ControlFlow<()> {
            if let Some(with) = &query.with {
                for cte in &with.cte_tables {
                    self.0.insert(normalize_ident(&cte.alias.name.to_string()));
                }
            }
            core::ops::ControlFlow::Continue(())
        }
    }

    let mut collect = Collect(std::collections::HashSet::new());
    let walked = stmt.visit(&mut collect);
    debug_assert!(
        matches!(walked, core::ops::ControlFlow::Continue(())),
        "the visitor never breaks — it only collects"
    );
    collect.0
}

/// Split a relation name rendered by the visitor back into schema and table.
fn parse_relation(raw: &str) -> Option<(Option<String>, String)> {
    let parts: Vec<String> = raw
        .split('.')
        .map(normalize_ident)
        .filter(|p| !p.is_empty())
        .collect();
    last_two(&parts)
}

#[cfg(test)]
mod tests {
    use super::{extract, looks_like_sql, validate_dialect, RefRole};

    /// Every table a blob names, as `(schema, name, role)`, for terse assertions.
    fn refs(text: &str) -> Vec<(Option<String>, String, RefRole)> {
        let mut out: Vec<_> = extract(text, None)
            .statements
            .into_iter()
            .flat_map(|s| s.refs)
            .map(|r| (r.schema, r.name, r.role))
            .collect();
        out.sort_by_key(|r| (r.1.clone(), r.2 as u8));
        out
    }

    fn names(text: &str) -> Vec<String> {
        refs(text).into_iter().map(|r| r.1).collect()
    }

    #[test]
    fn create_defines_and_select_accesses() {
        assert_eq!(
            refs("CREATE TABLE users (id INT)"),
            [(None, "users".to_string(), RefRole::Defines)]
        );
        assert_eq!(
            refs("SELECT * FROM users"),
            [(None, "users".to_string(), RefRole::Accesses)]
        );
    }

    #[test]
    fn alter_and_drop_are_modifications_not_definitions() {
        assert_eq!(
            refs("ALTER TABLE users ADD COLUMN email VARCHAR(255)"),
            [(None, "users".to_string(), RefRole::Alters)]
        );
        assert_eq!(
            refs("DROP TABLE users"),
            [(None, "users".to_string(), RefRole::Alters)]
        );
    }

    #[test]
    fn create_as_select_both_defines_and_accesses() {
        // The case that justifies one statement kind with plural edge roles.
        let r = refs("CREATE TABLE report AS SELECT * FROM orders");
        assert!(r.contains(&(None, "report".to_string(), RefRole::Defines)));
        assert!(r.contains(&(None, "orders".to_string(), RefRole::Accesses)));
    }

    #[test]
    fn a_cte_name_is_not_a_table() {
        // A `WITH` name lives for one statement. Read as a table it mints a
        // node for something with no schema, no file, and no lifetime — found
        // by scanning real code, where kenn's own recursive-CTE query minted a
        // `cnt` table.
        let got = refs(
            "WITH RECURSIVE cnt(x) AS (SELECT 1 UNION ALL SELECT x+1 FROM cnt WHERE x < 9) \
             SELECT count(*) FROM cnt",
        );
        assert!(
            got.is_empty(),
            "a query over only its own CTE names no table: {got:?}"
        );
    }

    #[test]
    fn a_cte_does_not_hide_the_real_tables_around_it() {
        // The exclusion must be surgical: a CTE shadows its own name and
        // nothing else, or a `WITH` clause would silently swallow the reads
        // that motivate indexing the statement at all.
        let got = refs(
            "WITH recent AS (SELECT id FROM orders WHERE ts > 0) \
             SELECT u.name FROM users u JOIN recent r ON r.id = u.id",
        );
        let names: Vec<&str> = got.iter().map(|(_, n, _)| n.as_str()).collect();
        assert!(names.contains(&"orders"), "the CTE's own source: {got:?}");
        assert!(names.contains(&"users"), "the outer read: {got:?}");
        assert!(!names.contains(&"recent"), "the CTE itself: {got:?}");
    }

    #[test]
    fn a_schema_qualified_name_is_never_shadowed_by_a_cte() {
        // `WITH x AS (…)` introduces a bare name. `s.x` always denotes the
        // real table, so qualification must survive an identically-named CTE.
        let got = refs("WITH users AS (SELECT 1) SELECT * FROM public.users");
        let qualified: Vec<&(Option<String>, String, RefRole)> = got
            .iter()
            .filter(|(s, n, _)| s.as_deref() == Some("public") && n == "users")
            .collect();
        assert_eq!(qualified.len(), 1, "public.users survives: {got:?}");
    }

    #[test]
    fn aliases_are_not_mistaken_for_tables() {
        assert_eq!(
            names("SELECT u.id, o.total FROM users u JOIN orders o ON o.uid = u.id"),
            ["orders", "users"]
        );
    }

    #[test]
    fn an_explicit_schema_is_kept_and_never_invented() {
        let qualified = refs("CREATE TABLE analytics.users (id INT)");
        assert_eq!(qualified[0].0.as_deref(), Some("analytics"));
        let bare = refs("CREATE TABLE users (id INT)");
        assert_eq!(bare[0].0, None, "no schema is inferred for a bare name");
    }

    #[test]
    fn dialect_quoting_is_stripped_so_one_table_has_one_identity() {
        for text in [
            "SELECT * FROM `users`",
            "SELECT * FROM [users]",
            "SELECT * FROM \"users\"",
        ] {
            assert_eq!(names(text), ["users"], "quoting stripped from {text}");
        }
    }

    /// Parse one statement and report the verb the mapping table gives it.
    fn verb(text: &str) -> Option<&'static str> {
        let stmts = super::parse_with_sweep(text, None)?;
        super::verb_of(stmts.first()?)
    }

    #[test]
    fn every_mapped_kind_renders_its_own_verb() {
        // Walked arm by arm, not sampled: the gate's own arithmetic makes a
        // partly-covered mapping table expensive, and a kind that silently
        // renders as another is a lie about the schema.
        for (sql, want) in [
            ("SELECT id FROM users", "SELECT FROM"),
            ("INSERT INTO users (id) VALUES (1)", "INSERT INTO"),
            ("UPDATE users SET id = 1", "UPDATE"),
            ("DELETE FROM users WHERE id = 1", "DELETE FROM"),
            ("CREATE TABLE users (id INT)", "CREATE TABLE"),
            ("ALTER TABLE users ADD COLUMN x INT", "ALTER TABLE"),
            ("DROP TABLE users", "DROP TABLE"),
            ("DROP VIEW active_users", "DROP VIEW"),
            ("DROP INDEX users_id_idx", "DROP INDEX"),
            ("DROP SCHEMA tenant CASCADE", "DROP SCHEMA"),
            ("DROP SEQUENCE users_id_seq", "DROP SEQUENCE"),
            ("DROP ROLE app", "DROP ROLE"),
            ("DROP DATABASE olddb", "DROP DATABASE"),
            ("TRUNCATE TABLE audit_log", "TRUNCATE TABLE"),
            (
                "MERGE INTO a USING b ON a.id = b.id WHEN MATCHED THEN UPDATE SET x = 1",
                "MERGE INTO",
            ),
            ("CREATE VIEW v AS SELECT id FROM users", "CREATE VIEW"),
            ("CREATE INDEX users_id_idx ON users (id)", "CREATE INDEX"),
            ("CREATE SCHEMA tenant", "CREATE SCHEMA"),
            ("ANALYZE users", "ANALYZE"),
            ("COMMENT ON TABLE users IS 'people'", "COMMENT ON"),
            ("GRANT SELECT ON users TO app", "GRANT"),
            ("REVOKE SELECT ON users FROM app", "REVOKE"),
        ] {
            assert_eq!(verb(sql), Some(want), "for: {sql}");
        }
    }

    #[test]
    fn a_drop_view_does_not_render_as_a_drop_table() {
        // One `Drop` variant covers every object type, so this is the arm most
        // likely to collapse into a wrong answer.
        assert_ne!(verb("DROP VIEW v"), verb("DROP TABLE t"));
        assert_ne!(verb("DROP INDEX i"), verb("DROP TABLE t"));
    }

    #[test]
    fn the_verb_distinguishes_statements_the_role_cannot() {
        // The reason the kind is retained at all: `UPDATE` and `SELECT` are
        // both `RefRole::Accesses`, so the role alone loses the distinction.
        let update = extract("UPDATE users SET id = 1", None);
        let select = extract("SELECT id FROM users", None);
        assert_eq!(update.statements[0].refs[0].role, RefRole::Accesses);
        assert_eq!(select.statements[0].refs[0].role, RefRole::Accesses);
        assert_ne!(update.statements[0].verb, select.statements[0].verb);
    }

    #[test]
    fn an_unmapped_table_bearing_kind_reports_no_verb() {
        // `None` rather than a wrong guess — the caller signs by role instead.
        // 135 variants exist and most name no table; this pins that the table
        // does not pretend to cover them.
        assert_eq!(verb("EXPLAIN SELECT id FROM users"), None);
    }

    #[test]
    fn a_runtime_substituted_name_yields_nothing() {
        // Not knowable statically — no edge, no node, and the token must not
        // become a table name.
        let out = extract("SELECT * FROM ${tableName}", None);
        assert!(out.statements.iter().all(|s| s.refs.is_empty()));
    }

    #[test]
    fn an_interpolation_placeholder_is_not_a_table() {
        // Found by indexing a real C# service, which crashed the run on the
        // shell-safe-`pub_id` invariant with `sql:{schema.Value}`.
        //
        // The placeholder is *quoted* in the source — `IN SCHEMA
        // "{schema.Value}"` — and that is the whole reason it gets this far. A
        // bare `{…}` fails to parse and dies in the sweep; a quoted one is a
        // perfectly valid identifier, so it parses, `normalize_ident` strips
        // the quotes, and a variable name arrives here looking like a table.
        // The dot never splits it either, since it was one quoted part.
        //
        // The `$` is no help: it sits on the interpolated string's prefix,
        // which the literal scanner consumes before this module sees anything.
        assert!(names(r#"SELECT * FROM "{schema.Value}".orders"#).is_empty());
        assert!(names(r#"GRANT SELECT ON ALL TABLES IN SCHEMA "{schema.Value}""#).is_empty());
        assert!(names(r#"SELECT * FROM "{table}""#).is_empty());
    }

    #[test]
    fn the_prefilter_rejects_what_a_codebase_is_actually_full_of() {
        // The 97%: messages, paths, formats, keys.
        assert!(!looks_like_sql("failed to open the configuration file"));
        assert!(!looks_like_sql("/usr/local/share/kenn/models"));
        assert!(!looks_like_sql("{name}: {count} items remaining"));
        assert!(!looks_like_sql("application/json; charset=utf-8"));
    }

    #[test]
    fn the_prefilter_keeps_every_statement_that_can_name_a_table() {
        // A false negative silently loses edges, so this asserts on statements
        // rather than on the keyword list — the point is that the SQL survives,
        // not that a particular word is listed.
        //
        // DDL included: migrations are routinely written in code.
        for text in [
            "SELECT id FROM users",
            "INSERT INTO users (id) VALUES (1)",
            "UPDATE users SET name = 'x'",
            "DELETE FROM users WHERE id = 1",
            "CREATE TABLE sessions (id INT)",
            "ALTER TABLE users ADD COLUMN x INT",
            "DROP TABLE orders",
            "TRUNCATE TABLE audit_log",
        ] {
            assert!(looks_like_sql(text), "rejected: {text}");
        }
    }

    #[test]
    fn with_and_merge_survive_on_their_inner_verb() {
        // Neither keyword is listed, and neither needs to be: a `WITH` clause
        // always resolves to one of the four read/write statements, and a
        // `MERGE` carries `UPDATE` or `INSERT` in its `WHEN` branches. Listing
        // them would buy no recall and cost a great deal of precision — `with`
        // is one of the commonest words in any codebase's prose.
        //
        // This is the guard on that reasoning. If a form ever appears that
        // names a table under `WITH`/`MERGE` with no inner verb, it goes red.
        assert!(looks_like_sql("WITH t AS (SELECT 1) SELECT * FROM t"));
        assert!(looks_like_sql(
            "WITH d AS (DELETE FROM a RETURNING *) INSERT INTO b SELECT * FROM d"
        ));
        assert!(looks_like_sql(
            "MERGE INTO a USING b ON a.id = b.id WHEN MATCHED THEN UPDATE SET x = 1"
        ));
        // And the bare word alone does not get through.
        assert!(!looks_like_sql("could not merge the two config files"));
        assert!(!looks_like_sql(
            "failed to connect with the upstream server"
        ));
    }

    #[test]
    fn a_maintenance_statement_is_skipped_without_losing_its_table() {
        // `LOCK TABLE x` and `ANALYZE x` name a table with no other verb, so
        // these literals really are skipped — and it costs nothing, because a
        // symbol that locks or analyses a table also reads or writes it. That
        // is why it locked it. Verified by re-indexing a workspace with
        // `LOCK TABLE` in 8 files: 169 distinct edges either way, identical.
        //
        // The shape below is the real one, from a migration that holds a write
        // lock across a duplicate scan — and the `FROM` in that same function
        // is what carries the edge.
        assert!(!looks_like_sql(
            "LOCK TABLE public.useraccounts IN SHARE ROW EXCLUSIVE MODE;"
        ));
        assert!(!looks_like_sql("ANALYZE public.paymentforwards"));
        assert!(looks_like_sql(
            "SELECT user_id, count(*) FROM public.useraccounts GROUP BY user_id"
        ));
    }

    #[test]
    fn a_bare_from_clause_is_not_a_statement() {
        // The prefilter's biggest win, and it was an accident. sqlparser accepts
        // a bare `FROM x` as a statement under some dialect, so a test asserting
        // `query.includes("from transactions")` minted a table — and so did the
        // English phrase "From Root Task", which named a table `Root`.
        //
        // `from` is not a verb, so none of these reach the parser now. The audit
        // counted 26 such "misses" in one workspace; every one was a fragment or
        // prose, not a query.
        assert!(!looks_like_sql("from transactions"));
        assert!(!looks_like_sql(
            "from invoices, customers, merchantaccounts"
        ));
        assert!(!looks_like_sql("From Root Task"));
    }

    #[test]
    fn the_prefilter_matches_whole_words_only() {
        // `deleted_at` is not `DELETE`, and a word-substring match would let
        // most of the corpus back through — which would cost the speedup
        // without costing correctness, so it fails quietly.
        assert!(!looks_like_sql("row deleted_at is null"));
        assert!(!looks_like_sql("selector not found in document"));
        assert!(!looks_like_sql("could not update_cache the entry"));
        // Case and surrounding punctuation do not matter.
        assert!(looks_like_sql("  select 1  "));
        assert!(looks_like_sql("(SeLeCt id from t)"));
    }

    #[test]
    fn drop_names_only_a_table_when_that_is_what_it_dropped() {
        // `DROP` names whatever kind it was told to, and taking every name as a
        // table is where 12 of one real workspace's 74 "tables" came from.
        assert!(names("DROP INDEX public.paymentforwards_operation_id_uidx").is_empty());
        assert!(names("DROP ROLE IF EXISTS probe_owner").is_empty());
        assert!(names("DROP SCHEMA tenant_blank CASCADE").is_empty());
        assert!(names("DROP TYPE payment_status").is_empty());
        assert!(names("DROP SEQUENCE orders_id_seq").is_empty());
    }

    #[test]
    fn drop_table_and_drop_view_are_still_tables() {
        // The other side of the same gate — a view is selected from exactly as
        // a table is, so the reference is the same fact.
        assert_eq!(names("DROP TABLE orders"), ["orders"]);
        assert_eq!(names("DROP VIEW active_users"), ["active_users"]);
        assert_eq!(names("DROP TABLE a, b"), ["a", "b"]);
    }

    #[test]
    fn a_function_in_from_is_not_a_table() {
        // Both from real code. A set-returning function in `FROM` is ordinary
        // SQL: the relation walk sees only the name, so the call has to be
        // recognized where the argument list still exists.
        assert!(names("SELECT * FROM generate_series(1, 2000)").is_empty());
        assert_eq!(
            names(r"SELECT tenant FROM tenant.uses_key($1, $2)"),
            [] as [String; 0]
        );
        // A real table joined against a function keeps the table.
        assert_eq!(
            names("SELECT u.id FROM users u JOIN generate_series(1, 10) g ON true"),
            ["users"]
        );
    }

    #[test]
    fn an_ordinary_name_still_survives_the_placeholder_check() {
        // The guard must not be so broad it drops real tables — the failure
        // mode a `contains('{')` rule invites.
        assert_eq!(names("SELECT * FROM public.orders"), ["orders"]);
        assert_eq!(names("SELECT * FROM order_items"), ["order_items"]);
    }

    #[test]
    fn the_sweep_recovers_what_the_permissive_parse_rejects() {
        // Bracket-quoted identifiers are rejected by the permissive dialect and
        // accepted by another — the sweep is what makes this reachable.
        assert_eq!(
            names("SELECT [id] FROM [dbo].[users] WHERE [id] = 1"),
            ["users"]
        );
    }

    #[test]
    fn recovery_is_deterministic() {
        let text = "SELECT [id] FROM [dbo].[users]";
        assert_eq!(names(text), names(text));
    }

    #[test]
    fn a_block_keeps_the_references_inside_it() {
        // Whole-blob parse first: splitting would shear `BEGIN UPDATE accounts …`
        // into a failing fragment and lose `accounts`.
        let text = "BEGIN\n  UPDATE accounts SET bal = 0;\n  DELETE FROM stale_rows;\nEND;\n\
                    CREATE TABLE ledger (id INT);";
        let n = names(text);
        for expected in ["accounts", "ledger", "stale_rows"] {
            assert!(
                n.contains(&expected.to_string()),
                "{expected} kept, got {n:?}"
            );
        }
    }

    #[test]
    fn an_unparseable_block_does_not_take_the_rest_of_the_file() {
        // No dialect parses this procedure whole, so the split tier runs and the
        // table declared after it still lands.
        let text = "CREATE PROCEDURE p AS BEGIN SELECT * FROM users END;\n\
                    CREATE TABLE reports (id INT);";
        assert!(names(text).contains(&"reports".to_string()));
    }

    #[test]
    fn a_fragment_naming_nothing_produces_no_statement() {
        // Splitting a block body leaves a bare terminator that parses and names
        // nothing; it must not mint a statement node.
        let out = extract(
            "CREATE PROCEDURE p AS BEGIN UPDATE users SET x = 1; END;\nCREATE TABLE t (id INT);",
            None,
        );
        assert!(
            out.statements.iter().all(|s| !s.refs.is_empty()),
            "no statement node without a reference"
        );
    }

    #[test]
    fn spans_select_the_statement_they_describe() {
        let text = "CREATE TABLE a (id INT);\nSELECT * FROM b;";
        let out = extract(text, None);
        assert_eq!(out.statements.len(), 2);
        let first = text
            .get(out.statements[0].span.clone())
            .expect("span is a char boundary");
        let second = text
            .get(out.statements[1].span.clone())
            .expect("span is a char boundary");
        assert!(first.contains("CREATE TABLE a"));
        assert!(second.contains("FROM b"));
    }

    #[test]
    fn empty_input_is_not_a_failure() {
        let out = extract("   \n\t ", None);
        assert!(out.statements.is_empty());
        assert_eq!(out.unparsed, 0);
    }

    #[test]
    fn a_configured_dialect_is_validated_up_front() {
        validate_dialect("mssql").expect("mssql is a provided dialect");
        assert!(
            validate_dialect("not-a-dialect").is_err(),
            "an unknown name is a config error, never a silent fallback"
        );
    }

    #[test]
    fn the_same_text_extracts_identically_whatever_its_origin() {
        // The shared-extractor contract: a bridge handing this module SQL lifted
        // out of markup must get exactly what a `.sql` file would.
        let sql = "SELECT id, email FROM users WHERE id = 1";
        assert_eq!(extract(sql, None), extract(sql, None));
        assert_eq!(names(sql), ["users"]);
    }
}

/// Manual audit: what would the pre-filter reject that the parser would have
/// accepted? Point `KENN_PREFILTER_AUDIT` at a workspace and run with
/// `--ignored --nocapture`. Not a guard — a measuring instrument, kept because
/// the keyword list is a recall/cost trade that must be re-checked against a
/// real corpus whenever it changes.
#[cfg(test)]
#[test]
#[ignore = "manual: needs KENN_PREFILTER_AUDIT=<workspace path>"]
fn audit_prefilter_false_negatives() {
    let Ok(root) = std::env::var("KENN_PREFILTER_AUDIT") else {
        return;
    };
    let mut checked = 0_usize;
    let mut rejected_but_parses = Vec::new();
    let mut stack = vec![std::path::PathBuf::from(&root)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if p.is_dir() {
                if !matches!(
                    name.as_str(),
                    ".git" | ".kenn" | "node_modules" | "target" | "bin" | "obj"
                ) {
                    stack.push(p);
                }
                continue;
            }
            let lang = match p.extension().and_then(|e| e.to_str()) {
                Some("rs") => "rust",
                Some("cs") => "csharp",
                Some("ts") => "typescript",
                _ => continue,
            };
            let Ok(src) = std::fs::read_to_string(&p) else {
                continue;
            };
            for lit in crate::code_sql::literals::literals(lang, &src) {
                let t = lit.text.trim();
                if t.len() < 12 || looks_like_sql(t) {
                    continue;
                }
                checked += 1;
                let ex = extract(t, None);
                if ex.unparsed == 0 && ex.statements.iter().any(|s| !s.refs.is_empty()) {
                    // By chars, not bytes — a byte slice can land mid-character
                    // and panic, and SQL literals carry plenty of non-ASCII.
                    let head: String = t.chars().take(120).collect();
                    rejected_but_parses.push(format!("{}: {head}", p.display()));
                }
            }
        }
    }
    println!("rejected by prefilter: {checked}");
    println!(
        "of those, would have named a table: {}",
        rejected_but_parses.len()
    );
    for r in &rejected_but_parses {
        println!("  MISS {r}");
    }
}

/// Manual audit for task 3.9: does the strategy hold on a real corpus?
///
/// Every number in the module doc came from hand-written statements. This walks
/// a workspace's `.sql` files and reports, for each, what whole-file-first
/// recovers against what split-first would, plus whether the tokenizer stayed
/// more permissive than the parser (the split tier depends on it).
///
/// `KENN_SQL_AUDIT=<workspace>` and `--ignored --nocapture`.
#[cfg(test)]
#[test]
#[ignore = "manual: needs KENN_SQL_AUDIT=<workspace path>"]
fn audit_strategy_on_a_real_corpus() {
    let Ok(root) = std::env::var("KENN_SQL_AUDIT") else {
        return;
    };
    let (mut files, mut whole_tables, mut split_tables) = (0usize, 0usize, 0usize);
    let (mut whole_wins, mut split_wins, mut tokenized_more) = (0usize, 0usize, 0usize);

    let mut stack = vec![std::path::PathBuf::from(&root)];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for e in entries.flatten() {
            let p = e.path();
            let name = p
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .to_string();
            if p.is_dir() {
                if !matches!(name.as_str(), ".git" | ".kenn" | "node_modules" | "target") {
                    stack.push(p);
                }
                continue;
            }
            if p.extension().and_then(|x| x.to_str()) != Some("sql") {
                continue;
            }
            let Ok(src) = std::fs::read_to_string(&p) else {
                continue;
            };
            files += 1;

            // Whole-file-first: the shipped strategy.
            let whole: usize = extract(&src, None)
                .statements
                .iter()
                .map(|s| s.refs.len())
                .sum();
            // Split-first: parse each tokenizer piece independently.
            let pieces = statement_pieces(&src, None);
            let split: usize = pieces
                .iter()
                .filter_map(|r| src.get(r.clone()))
                .map(|piece| {
                    extract(piece, None)
                        .statements
                        .iter()
                        .map(|s| s.refs.len())
                        .sum::<usize>()
                })
                .sum();
            whole_tables += whole;
            split_tables += split;
            if whole > split {
                whole_wins += 1;
            } else if split > whole {
                split_wins += 1;
                println!("  SPLIT-WINS {} whole={whole} split={split}", p.display());
            }
            // The split tier only helps if tokenizing survives what parsing rejects.
            if parse_with_sweep(&src, None).is_none() && pieces.len() > 1 {
                tokenized_more += 1;
            }
        }
    }
    println!("files: {files}");
    println!("table refs — whole-file-first: {whole_tables}, split-first: {split_tables}");
    println!("files where whole-file-first recovered more: {whole_wins}");
    println!("files where split-first recovered more:      {split_wins}");
    println!("files the parser rejected whole but the tokenizer still split: {tokenized_more}");
}
