use super::*;
use crate::markdown::DiscoveredMarkdown;
use kenn_config::MarkdownRoot;
use kenn_model::LinkGrade;
use std::fs;
use tempfile::TempDir;

fn cfg() -> MarkdownConfig {
    MarkdownConfig {
        enabled: true,
        roots: vec![MarkdownRoot {
            glob: "docs".into(),
            label: None,
        }],
        excludes: vec![],
        includes: vec![],
    }
}

#[test]
fn external_stub_classifies_assets_vs_notes() {
    // Classification reads the raw (unescaped) name's extension: png/pdf/css
    // → Attachment; an extension-less or `.md` name → Document.
    let pid = "md:@unresolved/x";
    for asset in ["diagram.png", "spec.pdf", "x.css"] {
        assert_eq!(
            external_stub(1, pid, asset.to_string()).kind,
            Kind::Attachment,
            "{asset}"
        );
    }
    for note in ["Some Note", "auth", "note.md", "2024.01.05"] {
        assert_eq!(
            external_stub(1, pid, note.to_string()).kind,
            Kind::Document,
            "{note}"
        );
    }
}

#[test]
fn ingests_markdown_records_into_the_store() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("docs/sub")).unwrap();
    fs::write(ws.join("docs/a.md"), "# A\nintro\n## B\nbody\n").unwrap();
    fs::write(ws.join("docs/sub/c.md"), "# C\ntext\n").unwrap();

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

    // Phase 1 (md↔md) then the post-code resolution with no code: a green
    // run means write_batch accepted every markdown record — the
    // document/section symbols, the md FileRecord, and the
    // contains/defined_in edges — through the real store.
    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (counts, pending) = ingest_markdown_phase1(&cfg(), ws, p1).expect("phase1");
    assert_eq!(counts.files, 2);
    // Counts feed the run report (kenn status / regression check): 2 docs +
    // their sections + the root/dir modules, defs and contains/defined_in.
    assert!(counts.symbols >= 2 && counts.defs >= 2 && counts.edges >= 2);
    let p2 = BatchSink::new(writer, rt.handle().clone(), 16);
    resolve_markdown_code(pending, None, p2).expect("resolve");
}

// --- file_link_edges (pure) -------------------------------------------

fn disc(relpath: &str) -> DiscoveredMarkdown {
    DiscoveredMarkdown {
        abs_path: std::path::PathBuf::from(relpath),
        label: "workspace".into(),
        relpath: relpath.into(),
        in_repo: true,
    }
}

/// Walk a set of files, returning the node-id map + per-file state +
/// index + contents, so link-edge resolution can be exercised without a
/// sink. All vectors are in `files` order; every file is in-repo.
fn harness(
    files: &[(&str, &str)],
) -> (
    HashMap<String, ShortId>,
    Vec<FileState>,
    ResolutionIndex,
    Vec<String>,
) {
    let discs: Vec<_> = files.iter().map(|(p, _)| disc(p)).collect();
    let collected: Vec<_> = files.iter().map(|(_, c)| collect(c)).collect();
    let index = ResolutionIndex::build(discs.iter().zip(collected.iter()));
    let mut ids = MarkdownIds::new();
    let mut node_ids = HashMap::new();
    let mut states = Vec::new();
    for ((d, c), (_, content)) in discs.iter().zip(collected.iter()).zip(files.iter()) {
        let module = ids.mint_symbol(); // stand-in dir module for this file
        let records = walk_markdown(d, content, c, &mut ids, module);
        let doc_id = document_id(&d.label, &d.relpath).into_string();
        let doc_sym = records.symbols.first().map_or(0, |s| s.id);
        for s in &records.symbols {
            node_ids.insert(s.pub_id.clone(), s.id);
        }
        states.push(FileState {
            doc_id,
            doc_sym,
            relpath: d.relpath.clone(),
            in_repo: d.in_repo,
            defs: records.defs,
        });
    }
    let contents = files.iter().map(|(_, c)| (*c).to_string()).collect();
    (node_ids, states, index, contents)
}

/// Resolve one in-repo file's md↔md links, returning the edges + the
/// deferred (md→code-pending) links.
fn resolve_file(
    content: &str,
    state: &FileState,
    node_ids: &HashMap<String, ShortId>,
    index: &ResolutionIndex,
) -> (Vec<EdgeRecord>, Vec<DeferredLink>, Vec<SymbolRecord>) {
    let mut ids = MarkdownIds::new();
    let mut stubs = HashMap::new();
    let mut stub_records = Vec::new();
    let mut deferred = Vec::new();
    let edges = file_link_edges(
        content,
        state,
        node_ids,
        index,
        &mut ids,
        &mut stubs,
        &mut stub_records,
        &mut deferred,
    );
    (edges, deferred, stub_records)
}

#[test]
fn links_to_edge_targets_resolved_section() {
    // a links to b#flow; expect one links_to edge a→(b#flow).
    let files = [
        ("docs/a.md", "# A\nsee [[b#flow]]\n"),
        ("docs/b.md", "# B\n## Flow\nbody\n"),
    ];
    let (node_ids, states, index, contents) = harness(&files);
    let (edges, deferred, stub_records) = resolve_file(&contents[0], &states[0], &node_ids, &index);
    assert_eq!(edges.len(), 1);
    let e = &edges[0];
    assert!(matches!(
        e.properties,
        EdgeProperties::LinksTo {
            grade: LinkGrade::Exact,
            ..
        }
    ));
    // target is the b#flow section node
    let b_flow = node_ids["md:workspace/docs/b.md#flow"];
    assert_eq!(e.target_id, b_flow);
    // src is a's "see [[b#flow]]" section (the `# A` section), not the file
    let a_sec = node_ids["md:workspace/docs/a.md#a"];
    assert_eq!(e.src_id, a_sec);
    assert!(stub_records.is_empty());
    assert!(deferred.is_empty());
}

#[test]
fn transclusion_emits_embeds_edge() {
    let files = [("docs/a.md", "# A\n![[b]]\n"), ("docs/b.md", "# B\n")];
    let (node_ids, states, index, contents) = harness(&files);
    let (edges, _deferred, _stubs) = resolve_file(&contents[0], &states[0], &node_ids, &index);
    assert_eq!(edges.len(), 1);
    assert!(matches!(edges[0].properties, EdgeProperties::Embeds { .. }));
}

#[test]
fn in_repo_dangling_link_is_deferred_not_stubbed() {
    // In-repo: a link that fails md↔md is held for the md→code barrier — no
    // edge and no stub yet.
    let files = [("docs/a.md", "# A\n[[ghost]] and again [[ghost]]\n")];
    let (node_ids, states, index, contents) = harness(&files);
    let (edges, deferred, stub_records) = resolve_file(&contents[0], &states[0], &node_ids, &index);
    assert!(edges.is_empty());
    assert!(stub_records.is_empty());
    assert_eq!(deferred.len(), 2); // both occurrences deferred
    assert!(deferred.iter().all(|d| d.raw.target == "ghost"));
}

#[test]
fn external_vault_dangling_link_mints_stub() {
    // External vault: no md→code resolution (D6), so a failed link dangles
    // immediately to one deduped stub.
    let files = [("docs/a.md", "# A\n[[ghost]] and again [[ghost]]\n")];
    let (node_ids, mut states, index, contents) = harness(&files);
    states[0].in_repo = false; // pretend this file is an external vault
    let (edges, deferred, stub_records) = resolve_file(&contents[0], &states[0], &node_ids, &index);
    assert!(deferred.is_empty());
    assert_eq!(edges.len(), 2);
    assert!(edges.iter().all(|e| matches!(
        e.properties,
        EdgeProperties::LinksTo {
            grade: LinkGrade::Dangling,
            ..
        }
    )));
    assert_eq!(stub_records.len(), 1); // deduped
    assert!(stub_records[0].external);
    assert_eq!(edges[0].target_id, edges[1].target_id);
}

/// End-to-end md→code barrier: a prose `[[OrderHandler]]` reference resolves
/// against the building code graph, and the code symbol gains a `links_to`
/// backlink to the markdown section (the user's code→md priority).
#[test]
fn md_to_code_link_resolves_and_backlinks() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord};
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("docs")).unwrap();
    fs::write(
        ws.join("docs/guide.md"),
        "# Guide\nsee [[OrderHandler]] for details\n",
    )
    .unwrap();

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

    // Simulate a code ingest unit: one Rust symbol `OrderHandler`.
    let code_file = compose_short_id(Language::Rust, 1);
    let code_sym = compose_short_id(Language::Rust, 2);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/order.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: code_sym,
        pub_id: "rs:billing::OrderHandler".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Struct,
        name: "OrderHandler".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: code_sym,
        file_id: code_file,
        start_line: 1,
        start_col: 0,
        end_line: 9,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    rt.block_on(writer.write_batch(&b))
        .expect("write code batch");

    // Phase 1: emits md nodes and defers the in-repo [[OrderHandler]] link.
    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (counts, pending) = ingest_markdown_phase1(&cfg(), ws, p1).expect("phase1");
    assert_eq!(counts.files, 1);
    assert_eq!(pending.deferred.len(), 1);

    // Post-code barrier: resolve md→code against the building store.
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    resolve_markdown_code(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    drop(reader);

    // code→md backlink: list_inbound on the code symbol over `links_to`
    // returns the markdown section.
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            code_sym,
            "links_to",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(total, 1);
    assert_eq!(inbound.len(), 1);
    assert_eq!(inbound[0].kind, "section");
    assert!(inbound[0].pub_id.starts_with("md:workspace/docs/guide.md"));
}

/// An md→code link to a source *file* emits a `links_to_file` edge (not
/// `links_to`), so the code file gains a sound backlink to the md section.
#[test]
fn md_to_code_file_link_uses_links_to_file_edge() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord};
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("docs")).unwrap();
    fs::write(
        ws.join("docs/guide.md"),
        "# Guide\nsee [src](src/order.rs)\n",
    )
    .unwrap();

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

    let code_file = compose_short_id(Language::Rust, 1);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/order.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    // A symbol so the file has a def row (mirrors a real ingest).
    b.symbols.push(SymbolRecord {
        id: compose_short_id(Language::Rust, 2),
        pub_id: "rs:order::handle".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "handle".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: compose_short_id(Language::Rust, 2),
        file_id: code_file,
        start_line: 1,
        start_col: 0,
        end_line: 9,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    rt.block_on(writer.write_batch(&b))
        .expect("write code batch");

    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_files, pending) = ingest_markdown_phase1(&cfg(), ws, p1).expect("phase1");
    assert_eq!(pending.deferred.len(), 1);

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    resolve_markdown_code(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    drop(reader);

    // The code file has a `links_to_file` backlink from the md section;
    // it does NOT appear under plain `links_to` (kind disambiguates).
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let (ltf, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            code_file,
            "links_to_file",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound links_to_file");
    assert_eq!(total, 1);
    assert!(ltf[0].pub_id.starts_with("md:workspace/docs/guide.md"));
    let (lt, _) = rt
        .block_on(Reader::list_inbound(
            &reader,
            code_file,
            "links_to",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound links_to");
    assert!(lt.is_empty());
}

/// Nested directories become a `Kind::Module` chain: each document is a
/// `defined_in` member of its directory module, which nests under its parent
/// module up to the root — so `list_in_scope` browses the corpus by folder.
#[test]
fn nested_directory_modules_chain_and_own_documents() {
    use kenn_store::api::Reader;

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("docs/a")).unwrap();
    fs::write(ws.join("docs/a/today.md"), "# Today\n").unwrap();

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
    let sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    ingest_markdown_phase1(&cfg(), ws, sink).expect("phase1");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let fetch = |pub_id: &str| {
        rt.block_on(Reader::fetch_symbol(&reader, "markdown", pub_id))
            .expect("fetch")
    };
    let children = |id| {
        rt.block_on(Reader::list_inbound(
            &reader,
            id,
            "defined_in",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound")
        .0
    };

    // The module chain exists: root → docs → docs/a, all Kind::Module.
    for m in ["md:workspace", "md:workspace/docs", "md:workspace/docs/a"] {
        assert_eq!(
            fetch(m).unwrap_or_else(|| panic!("{m} missing")).kind,
            "module"
        );
    }
    // docs/a owns the document; docs owns the docs/a module; root owns docs.
    let docs_a = fetch("md:workspace/docs/a").unwrap();
    assert!(children(docs_a.id)
        .iter()
        .any(|s| s.pub_id == "md:workspace/docs/a/today.md"));
    let docs = fetch("md:workspace/docs").unwrap();
    assert!(children(docs.id)
        .iter()
        .any(|s| s.pub_id == "md:workspace/docs/a"));
    let root = fetch("md:workspace").unwrap();
    assert!(children(root.id)
        .iter()
        .any(|s| s.pub_id == "md:workspace/docs"));
}

/// Task 8.1 — one fixture over the whole graph: in-repo nested docs + an
/// external vault + code, asserting module nesting, md↔md, md→code (symbol
/// and file backlinks), the in-repo-only code gate (D6), and the link report.
#[test]
fn end_to_end_corpus_graph() {
    use kenn_config::MarkdownRoot;
    use kenn_model::{compose_short_id, DefRecord, FileRecord};
    use kenn_store::api::Reader;

    let ws_dir = TempDir::new().unwrap();
    let ws = ws_dir.path();
    let vault_dir = TempDir::new().unwrap();
    let vault = vault_dir.path();
    let mk = |root: &std::path::Path, rel: &str, body: &str| {
        let p = root.join(rel);
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, body).unwrap();
    };
    mk(
        ws,
        "docs/a/guide.md",
        "# Guide\n[[other]], [[OrderHandler]], [src](src/order.rs), \
             [old](../old/other.md), [[ghost]]\n",
    );
    mk(ws, "docs/a/other.md", "# Other\nbody\n");
    mk(vault, "today.md", "# Today\nuses [[OrderHandler]]\n");

    let building = ws.join(".kenn/local/building");
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
    let code_file = compose_short_id(Language::Rust, 1);
    let code_sym = compose_short_id(Language::Rust, 2);
    let mut batch = kenn_store::api::WriteBatch::default();
    batch.files.push(FileRecord {
        id: code_file,
        path: "src/order.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    batch.symbols.push(SymbolRecord {
        id: code_sym,
        pub_id: "rs:order::OrderHandler".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Struct,
        name: "OrderHandler".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    batch.defs.push(DefRecord {
        sym_id: code_sym,
        file_id: code_file,
        start_line: 1,
        start_col: 0,
        end_line: 9,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    rt.block_on(writer.write_batch(&batch)).expect("write code");

    let config = MarkdownConfig {
        enabled: true,
        roots: vec![
            MarkdownRoot {
                glob: ".".into(),
                label: None,
            },
            MarkdownRoot {
                glob: vault.to_string_lossy().into_owned(),
                label: Some("notes".into()),
            },
        ],
        excludes: vec!["**/.kenn/**".into()],
        includes: vec![],
    };
    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 32);
    let (_files, pending) = ingest_markdown_phase1(&config, ws, p1).expect("phase1");
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 32);
    resolve_markdown_code(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    drop(reader);

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let fetch = |p: &str| {
        rt.block_on(Reader::fetch_symbol(&reader, "markdown", p))
            .expect("fetch")
    };
    let inbound = |id, rel: &str| {
        rt.block_on(Reader::list_inbound(
            &reader,
            id,
            rel,
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound")
        .0
        .into_iter()
        .map(|s| s.pub_id)
        .collect::<Vec<_>>()
    };
    let from_guide = |p: &str| p.starts_with("md:workspace/docs/a/guide.md");

    // 1. Nesting: docs/a module owns both documents.
    let docs_a = fetch("md:workspace/docs/a").expect("docs/a module");
    assert_eq!(docs_a.kind, "module");
    let a_kids = inbound(docs_a.id, "defined_in");
    assert!(a_kids.iter().any(|p| p == "md:workspace/docs/a/guide.md"));
    assert!(a_kids.iter().any(|p| p == "md:workspace/docs/a/other.md"));

    // 2. md↔md: guide links the sibling document.
    let other = fetch("md:workspace/docs/a/other.md").expect("other");
    assert!(inbound(other.id, "links_to").iter().any(|p| from_guide(p)));

    // 3. md→code: guide backlinks both the code symbol and the code file.
    let sym_back = inbound(code_sym, "links_to");
    assert!(sym_back.iter().any(|p| from_guide(p)));
    assert!(inbound(code_file, "links_to_file")
        .iter()
        .any(|p| from_guide(p)));

    // 4. D6 gate: the external vault's [[OrderHandler]] did NOT resolve to
    //    code, but the vault document is still indexed.
    assert!(!sym_back.iter().any(|p| p.starts_with("md:notes/")));
    assert!(fetch("md:notes/today.md").is_some());

    // 5. Link report: drifted + dangling are listed; exact is not.
    let (diags, _total) = rt
        .block_on(reader.scan_link_diagnostics(None, 10_000))
        .expect("diagnostics");
    assert!(diags.iter().any(|d| d.grade == "drifted"));
    // scan_link_diagnostics returns the raw stub id; the check_links tool
    // decodes the `md:@unresolved/` prefix to the written target.
    assert!(diags
        .iter()
        .any(|d| d.grade == "dangling" && d.target == "md:@unresolved/ghost"));
    assert!(diags.iter().all(|d| d.grade != "exact"));
}
