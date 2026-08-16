//! The `.sql` producer: a barrier-free phase-1 sibling unit.
//!
//! Discovery, parsing, resolution, and writing complete in one pass, leaving no
//! pending state for the post-code barrier — the shape the text producer
//! establishes. It can be barrier-free because every input it needs is a `.sql`
//! file it walks itself.
//!
//! Each file is parsed **once**; the two passes that follow run over the
//! retained results, never over the source again. Parsing is the expensive step
//! (a failure sweeps every dialect), so a second parse pass would double the
//! cost of the whole producer.
//!
//! * **Pass 1** collects the full identity set from *every* reference — not
//!   only declarations — and emits the file, statement, and table nodes. A table
//!   some statement declares is internal; one only ever referenced is external.
//! * **Pass 2** resolves each reference against that set and emits graded edges.
//!
//! Building the set from all references is what lets an undeclared table link.
//! Collecting it before resolving is what lets a query in an early-sorted file
//! reach a table declared in a later one.

use std::collections::BTreeMap;
use std::path::Path;

use globset::{Glob, GlobSet, GlobSetBuilder};
use kenn_model::id::sql::{file_id as sql_file_id, statement_id, table_id};
use kenn_model::{
    compose_short_id, DefRecord, EdgeKind, EdgeProperties, FileRecord, Kind, Language, LinkGrade,
    ShortId, SymbolDocsRecord, SymbolRecord,
};

use thiserror::Error;

use super::parse::{extract, Extraction, RefRole};
use super::registry::{resolve, NameSet, TableKey, TableRegistry};
use crate::sink::BatchSink;

#[derive(Debug, Error)]
pub enum SqlIngestError {
    #[error("bad {kind} glob `{pattern}`: {source}")]
    BadGlob {
        kind: &'static str,
        pattern: String,
        source: globset::Error,
    },
    #[error(transparent)]
    Db(#[from] kenn_store::DbError),
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct SqlCounts {
    pub files: u64,
    pub tables: u64,
    pub statements: u64,
    pub edges: u64,
    /// Files that could not be read.
    pub unreadable: u64,
    /// Pieces no dialect parsed — real signal for a `.sql` file.
    pub unparsed: u64,
}

/// Per-language short ids, partitioned so they never collide with another
/// producer's.
struct SqlIds {
    next_file: u32,
    next_symbol: u32,
}

impl SqlIds {
    fn new() -> Self {
        Self {
            next_file: 1,
            next_symbol: 1,
        }
    }
    fn file(&mut self) -> ShortId {
        let id = compose_short_id(Language::Sql, self.next_file);
        self.next_file += 1;
        id
    }
    fn symbol(&mut self) -> ShortId {
        let id = compose_short_id(Language::Sql, self.next_symbol);
        self.next_symbol += 1;
        id
    }
}

/// One `.sql` file, parsed and retained for the resolution passes.
struct ParsedFile {
    relpath: String,
    content: String,
    extraction: Extraction,
}

fn build_set(patterns: &[String], kind: &'static str) -> Result<GlobSet, SqlIngestError> {
    let mut b = GlobSetBuilder::new();
    for pat in patterns {
        let g = Glob::new(pat).map_err(|source| SqlIngestError::BadGlob {
            kind,
            pattern: pat.clone(),
            source,
        })?;
        b.add(g);
    }
    b.build().map_err(|source| SqlIngestError::BadGlob {
        kind,
        pattern: String::new(),
        source,
    })
}

/// Recursively collect `.sql` files under `root`, honouring the exclude set.
fn discover(root: &Path, excludes: &GlobSet, out: &mut Vec<std::path::PathBuf>) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for path in entries {
        let rel = path.to_string_lossy().to_string();
        if excludes.is_match(&rel) {
            continue;
        }
        if path.is_dir() {
            discover(&path, excludes, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("sql") {
            out.push(path);
        }
    }
}

/// Index every `.sql` file under the workspace, emitting through `sink`.
///
/// # Errors
/// Returns an error only for a bad configured glob or a store write failure —
/// an unreadable or unparseable file degrades the counts and the run continues.
pub fn ingest_sql(
    config: &kenn_config::SqlConfig,
    workspace_root: &Path,
    mut sink: BatchSink,
) -> Result<SqlCounts, SqlIngestError> {
    let mut counts = SqlCounts::default();
    let excludes = build_set(&config.effective_excludes(), "exclude")?;
    let mut paths = Vec::new();
    discover(workspace_root, &excludes, &mut paths);
    if paths.is_empty() {
        sink.finish()?;
        return Ok(counts);
    }

    let files = parse_all(
        &paths,
        workspace_root,
        config.dialect.as_deref(),
        &mut counts,
    );
    let (names, declared) = collect_identities(&files);
    let mut ids = SqlIds::new();
    let table_sym = emit_tables(&mut sink, &names, &declared, &mut ids, &mut counts)?;

    // ── Pass 2: file + statement nodes and graded edges ──────────────
    for f in &files {
        if f.extraction.statements.is_empty() {
            continue;
        }
        let file_id = ids.file();
        let doc_sym = ids.symbol();
        let total_lines = u32::try_from(f.content.lines().count())
            .unwrap_or(u32::MAX)
            .max(1);

        sink.push_file(FileRecord {
            id: file_id,
            path: f.relpath.clone(),
            language: Language::Sql,
            test: false,
            external: false,
            content_hash: xxhash_rust::xxh3::xxh3_64(f.content.as_bytes()),
        })?;
        // `Document` so statements roll up into their file rather than each
        // becoming its own aggregate.
        sink.push_symbol(SymbolRecord {
            id: doc_sym,
            pub_id: crate::pubid::floor(sql_file_id(&f.relpath).as_str()),
            language: Language::Sql,
            pkg_id: 0,
            kind: Kind::Document,
            name: stem(&f.relpath),
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external: false,
            test: false,
        })?;
        sink.push_def(def(doc_sym, file_id, 1, total_lines))?;
        counts.files += 1;

        for (i, st) in f.extraction.statements.iter().enumerate() {
            let sym = ids.symbol();
            let (start_line, end_line) = line_span(&f.content, st.span.start, st.span.end);
            sink.push_symbol(SymbolRecord {
                id: sym,
                pub_id: crate::pubid::floor(statement_id(&f.relpath, i).as_str()),
                language: Language::Sql,
                pkg_id: 0,
                kind: Kind::SqlStatement,
                name: statement_name(st.refs.first().map(|r| r.role), i),
                enclosing_sym_id: doc_sym,
                partial: false,
                nargs: 0,
                targs: 0,
                external: false,
                test: false,
            })?;
            sink.push_def(def(sym, file_id, start_line, end_line))?;
            sink.push_edge(EdgeRecordShim::defined_in(sym, doc_sym))?;
            // Statement text is prose worth searching: unlike a config value, a
            // query is exactly the kind of thing a conceptual search should find.
            if let Some(text) = f.content.get(st.span.clone()) {
                sink.push_symbol_docs(SymbolDocsRecord {
                    sym_id: sym,
                    sig: statement_signature(st),
                    doc: text.trim().to_string(),
                })?;
            }
            counts.statements += 1;

            for r in &st.refs {
                for cand in resolve(&names, r.schema.as_deref(), &r.name) {
                    let Some(&target) = table_sym.get(&cand.key) else {
                        continue;
                    };
                    sink.push_edge(EdgeRecordShim::table(sym, target, r.role, cand.grade))?;
                    counts.edges += 1;
                }
            }
        }
    }

    sink.finish()?;
    Ok(counts)
}

/// Read and parse every discovered file exactly once. An unreadable file is
/// counted and skipped so one bad file never costs the others.
fn parse_all(
    paths: &[std::path::PathBuf],
    workspace_root: &Path,
    dialect: Option<&str>,
    counts: &mut SqlCounts,
) -> Vec<ParsedFile> {
    let mut files = Vec::new();
    for abs in paths {
        let Ok(content) = std::fs::read_to_string(abs) else {
            counts.unreadable += 1;
            tracing::warn!(target: "kenn_indexer::sql", path = %abs.display(), "unreadable sql file, skipped");
            continue;
        };
        let relpath = abs
            .strip_prefix(workspace_root)
            .unwrap_or(abs)
            .to_string_lossy()
            .replace('\\', "/");
        let extraction = extract(&content, dialect);
        counts.unparsed += extraction.unparsed as u64;
        files.push(ParsedFile {
            relpath,
            content,
            extraction,
        });
    }
    files
}

/// The identity set the workspace mentions, and which of those some statement
/// declares. Built from EVERY reference, not only declarations — that is what
/// lets an undeclared table link.
fn collect_identities(files: &[ParsedFile]) -> (NameSet, std::collections::BTreeSet<TableKey>) {
    let mut names = NameSet::new();
    let mut declared = std::collections::BTreeSet::new();
    let all_refs = || {
        files
            .iter()
            .flat_map(|f| &f.extraction.statements)
            .flat_map(|s| &s.refs)
    };

    // Authoritative first: anything a statement declares, and anything that
    // states a schema. These are identities the source names outright.
    for r in all_refs() {
        if r.role == RefRole::Defines || r.schema.is_some() {
            let key = TableKey::new(r.schema.clone(), r.name.clone());
            names.insert(key.clone());
            if r.role == RefRole::Defines {
                declared.insert(key);
            }
        }
    }

    // Only then unqualified references, and only when nothing of that name is
    // known. An unqualified reference is not a new identity when a qualified
    // one exists — `SELECT FROM users` alongside `CREATE TABLE public.users`
    // means that table, so minting a bare `users` here would split one table
    // into two nodes and make every later reference ambiguous between them.
    for r in all_refs() {
        if r.schema.is_none() && names.identities_named(&r.name).is_empty() {
            names.insert(TableKey::new(None, r.name.clone()));
        }
    }
    (names, declared)
}

/// Emit one node per identity, before any statement so every edge has a target.
fn emit_tables(
    sink: &mut BatchSink,
    names: &NameSet,
    declared: &std::collections::BTreeSet<TableKey>,
    ids: &mut SqlIds,
    counts: &mut SqlCounts,
) -> Result<BTreeMap<TableKey, ShortId>, SqlIngestError> {
    let mut table_sym = BTreeMap::new();
    for key in names.iter() {
        let sym = ids.symbol();
        table_sym.insert(key.clone(), sym);
        sink.push_symbol(SymbolRecord {
            id: sym,
            pub_id: crate::pubid::floor(table_id(key.schema.as_deref(), &key.name).as_str()),
            language: Language::Sql,
            pkg_id: 0,
            kind: Kind::SqlTable,
            name: key.name.clone(),
            // Workspace-global: a table is enclosed by nothing, so it is its own
            // aggregate rather than rolling up into whichever file named it.
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            // Referenced-here-defined-elsewhere, the sense the graph already
            // uses for code symbols.
            external: !declared.contains(key),
            test: false,
        })?;
        counts.tables += 1;
    }
    Ok(table_sym)
}

/// Edge construction kept in one place so the role→kind mapping has a single
/// definition.
struct EdgeRecordShim;

impl EdgeRecordShim {
    fn defined_in(src: ShortId, target: ShortId) -> kenn_model::EdgeRecord {
        kenn_model::EdgeRecord {
            src_id: src,
            target_id: target,
            properties: EdgeProperties::DefinedIn,
        }
    }

    fn table(
        src: ShortId,
        target: ShortId,
        role: RefRole,
        grade: LinkGrade,
    ) -> kenn_model::EdgeRecord {
        let kind = match role {
            RefRole::Defines => EdgeKind::DefinesTable,
            RefRole::Alters => EdgeKind::AltersTable,
            RefRole::Accesses => EdgeKind::AccessesTable,
        };
        kenn_model::EdgeRecord {
            src_id: src,
            target_id: target,
            properties: EdgeProperties::Table { kind, grade },
        }
    }
}

fn def(sym_id: ShortId, file_id: ShortId, start_line: u32, end_line: u32) -> DefRecord {
    DefRecord {
        sym_id,
        file_id,
        start_line,
        start_col: 0,
        end_line,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    }
}

/// 1-based line numbers spanning a byte range.
fn line_span(text: &str, start: usize, end: usize) -> (u32, u32) {
    let line_of = |byte: usize| -> u32 {
        let counted = text
            .get(..byte.min(text.len()))
            .map_or(0, |s| s.matches('\n').count());
        u32::try_from(counted.saturating_add(1)).unwrap_or(u32::MAX)
    };
    (line_of(start), line_of(end))
}

/// A statement's signature: what it does, and to which tables.
///
/// `ALTER TABLE users`, `SELECT FROM users, auth`, `DELETE FROM sessions` — the
/// shape a reader recognises, rather than the whole statement text (which lives
/// on the content surface) or a synthetic name.
///
/// **Signed by what it defines**, when it defines anything. A
/// `CREATE TABLE … AS SELECT` names both its new table and its sources, and the
/// new table is what the statement is *about*; listing the sources first would
/// file a declaration under the tables it read.
///
/// No cap and no truncation. A join list long enough to be a problem is not a
/// thing real SQL produces, and a truncated signature is worse than a long one:
/// it looks complete.
fn statement_signature(st: &crate::sql::parse::ParsedStatement) -> String {
    let defined: Vec<&str> = st
        .refs
        .iter()
        .filter(|r| r.role == RefRole::Defines)
        .map(|r| r.name.as_str())
        .collect();
    let named: Vec<&str> = if defined.is_empty() {
        let mut seen: Vec<&str> = Vec::new();
        for r in &st.refs {
            if !seen.contains(&r.name.as_str()) {
                seen.push(r.name.as_str());
            }
        }
        seen
    } else {
        defined
    };

    // An unmapped kind still signs, by the role of what it touched — an empty
    // signature would make the statement unfindable by shape, which is the one
    // thing this surface is for.
    let verb = st.verb.map_or_else(
        || match st.refs.first().map(|r| r.role) {
            Some(RefRole::Defines) => "CREATE",
            Some(RefRole::Alters) => "ALTER",
            Some(RefRole::Accesses) | None => "QUERY",
        },
        crate::sql::parse::Verb::as_str,
    );
    if named.is_empty() {
        verb.to_owned()
    } else {
        format!("{verb} {}", named.join(", "))
    }
}

fn statement_name(role: Option<RefRole>, index: usize) -> String {
    let verb = match role {
        Some(RefRole::Defines) => "create",
        Some(RefRole::Alters) => "alter",
        Some(RefRole::Accesses) | None => "query",
    };
    format!("{verb}#{index}")
}

/// Last path segment without its extension.
fn stem(relpath: &str) -> String {
    relpath
        .rsplit('/')
        .next()
        .unwrap_or(relpath)
        .rsplit_once('.')
        .map_or_else(|| relpath.to_string(), |(s, _)| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::TempDir;

    fn cfg() -> kenn_config::SqlConfig {
        kenn_config::SqlConfig {
            enabled: true,
            ..Default::default()
        }
    }

    /// Run the producer over a temp workspace and return its counts.
    fn run(files: &[(&str, &str)]) -> (SqlCounts, TempDir) {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        for (rel, body) in files {
            let p = ws.join(rel);
            if let Some(parent) = p.parent() {
                fs::create_dir_all(parent).unwrap();
            }
            fs::write(p, body).unwrap();
        }
        let building = ws.join(".kenn").join("local").join("building");
        fs::create_dir_all(&building).unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let writer = rt
            .block_on(kenn_store::open_writer(
                &building,
                kenn_store::WriterOptions::default(),
            ))
            .expect("open_writer");
        let sink = BatchSink::new(writer, rt.handle().clone(), 16);
        let counts = ingest_sql(&cfg(), ws, sink).expect("ingest");
        (counts, dir)
    }

    #[test]
    fn a_migration_and_a_query_reach_one_table() {
        // Two files, one table node, three edges (create, alter, select).
        let (c, _d) = run(&[
            ("db/0001.sql", "CREATE TABLE users (id INT);"),
            (
                "db/0007.sql",
                "ALTER TABLE users ADD COLUMN email VARCHAR(255);",
            ),
            ("queries/active.sql", "SELECT * FROM users;"),
        ]);
        assert_eq!(c.files, 3);
        assert_eq!(c.tables, 1, "one identity across three files");
        assert_eq!(c.statements, 3);
        assert_eq!(c.edges, 3);
    }

    #[test]
    fn a_query_before_its_declaration_still_resolves() {
        // `a_query.sql` sorts before `b_schema.sql`: collecting the whole
        // identity set before resolving is what makes this work.
        let (c, _d) = run(&[
            ("a_query.sql", "SELECT * FROM users;"),
            ("b_schema.sql", "CREATE TABLE users (id INT);"),
        ]);
        assert_eq!(c.tables, 1);
        assert_eq!(c.edges, 2);
    }

    #[test]
    fn a_table_nothing_declares_is_still_indexed() {
        // The rule that decides whether an ORM-managed workspace gets a graph.
        let (c, _d) = run(&[("q.sql", "SELECT * FROM orders;")]);
        assert_eq!(c.tables, 1, "minted by the reference");
        assert_eq!(c.edges, 1);
    }

    #[test]
    fn an_unreadable_file_does_not_cost_the_others() {
        let dir = TempDir::new().unwrap();
        let ws = dir.path();
        fs::write(ws.join("good.sql"), "CREATE TABLE t (id INT);").unwrap();
        // Invalid UTF-8: `read_to_string` fails, portably and without relying
        // on permissions (root can read a chmod-000 file).
        fs::write(ws.join("bad.sql"), [0x43, 0x52, 0xff, 0xfe, 0x00]).unwrap();
        let building = ws.join(".kenn").join("local").join("building");
        fs::create_dir_all(&building).unwrap();
        let rt = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .unwrap();
        let writer = rt
            .block_on(kenn_store::open_writer(
                &building,
                kenn_store::WriterOptions::default(),
            ))
            .expect("open_writer");
        let sink = BatchSink::new(writer, rt.handle().clone(), 16);
        let c = ingest_sql(&cfg(), ws, sink).expect("ingest");
        assert_eq!(c.files, 1, "the readable file is still indexed");
        assert_eq!(c.unreadable, 1, "the failure is counted, not swallowed");
    }

    #[test]
    fn an_empty_workspace_is_not_a_failure() {
        let (c, _d) = run(&[("notes.md", "# not sql")]);
        assert_eq!(c.files, 0);
        assert_eq!(c.tables, 0);
    }

    #[test]
    fn excluded_directories_are_not_walked() {
        let (c, _d) = run(&[
            ("real.sql", "CREATE TABLE t (id INT);"),
            ("target/generated.sql", "CREATE TABLE junk (id INT);"),
        ]);
        assert_eq!(c.files, 1, "only the first-party file");
        assert_eq!(c.tables, 1);
    }

    #[test]
    fn an_unqualified_query_reaches_a_schema_qualified_declaration() {
        // Regression: an unqualified reference must not mint its own identity
        // alongside a qualified declaration of the same name. Doing so split
        // one table into `sql:users` and `sql:public.users` and made every
        // later reference ambiguous between them.
        let (c, _d) = run(&[
            ("db/init.sql", "CREATE TABLE public.users (id INT);"),
            ("q.sql", "SELECT * FROM users;"),
        ]);
        assert_eq!(c.tables, 1, "one identity, not one per spelling");
        assert_eq!(c.edges, 2, "both statements reach it");
    }

    #[test]
    fn a_create_as_select_emits_both_roles_from_one_statement() {
        let (c, _d) = run(&[("r.sql", "CREATE TABLE report AS SELECT * FROM orders;")]);
        assert_eq!(c.statements, 1, "one statement node");
        assert_eq!(c.tables, 2);
        assert_eq!(c.edges, 2, "defines report, accesses orders");
    }
}

#[cfg(test)]
mod signature_tests {
    use super::statement_signature;
    use crate::sql::parse::extract;

    fn sig(text: &str) -> String {
        let ex = extract(text, None);
        statement_signature(&ex.statements[0])
    }

    #[test]
    fn a_statement_signs_as_its_verb_and_the_tables_it_names() {
        assert_eq!(
            sig("ALTER TABLE users ADD COLUMN x INT"),
            "ALTER TABLE users"
        );
        assert_eq!(sig("UPDATE users SET id = 1"), "UPDATE users");
        assert_eq!(
            sig("DELETE FROM sessions WHERE id = 1"),
            "DELETE FROM sessions"
        );
    }

    #[test]
    fn a_multi_table_join_names_every_table_with_no_cap() {
        // No truncation: a join list long enough to be a problem is not a thing
        // real SQL produces, and a truncated signature is worse than a long one
        // because it looks complete.
        let s = sig("SELECT u.id FROM users u \
             JOIN auth a ON a.uid = u.id \
             JOIN sessions s ON s.uid = u.id \
             JOIN audit_log l ON l.uid = u.id");
        for t in ["users", "auth", "sessions", "audit_log"] {
            assert!(s.contains(t), "{t} missing from {s:?}");
        }
        assert!(s.starts_with("SELECT FROM "), "{s:?}");
    }

    #[test]
    fn a_create_as_select_signs_by_what_it_defines() {
        // It names both its new table and its sources, and the new table is
        // what the statement is ABOUT. Signing by the sources would file a
        // declaration under the tables it merely read.
        let s = sig("CREATE TABLE active AS SELECT id FROM users WHERE ok");
        assert_eq!(s, "CREATE TABLE active");
    }

    #[test]
    fn an_unmapped_kind_still_signs_by_role_rather_than_empty() {
        // An empty verb would make the statement unfindable by shape, which is
        // the one thing this surface exists for.
        //
        // Asserted on the VERB, not on the string being non-empty: the table
        // list alone makes it non-empty, so `!s.is_empty()` passed even with
        // the fallback deleted. And no `if let` — a vacuous pass when the
        // fixture stops producing a statement is exactly how this would rot.
        let ex = extract("EXPLAIN SELECT id FROM users", None);
        let st = ex
            .statements
            .first()
            .expect("the fixture must still yield a statement");
        assert_eq!(st.verb, None, "precondition: the kind is unmapped");
        let s = statement_signature(st);
        assert!(
            s.starts_with("QUERY "),
            "signs by role when the kind is unmapped: {s:?}"
        );
        assert!(s.contains("users"), "and still names the table: {s:?}");
    }

    #[test]
    fn a_table_named_twice_appears_once() {
        // A self-join names one table twice; a signature listing it twice reads
        // as two tables.
        let s = sig("SELECT a.id FROM users a JOIN users b ON b.mgr = a.id");
        assert_eq!(s, "SELECT FROM users");
    }
}
