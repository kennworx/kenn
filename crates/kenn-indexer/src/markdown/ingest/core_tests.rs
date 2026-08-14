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

/// A fixed answer set standing in for the workspace, so the attachment rung is
/// testable without touching the filesystem.
///
/// The trailing-slash strip is not cosmetic: the real backing is
/// `Path::exists`, and the filesystem answers the same for `docs` and `docs/`.
/// A set that distinguishes them makes the key-canonicalization guard vacuous —
/// it would reject the bad spelling by accident and pass no matter what the
/// code under test does.
struct Present(&'static [&'static str]);
impl PathExists for Present {
    fn exists(&self, canonical_path: &str) -> bool {
        let probe = canonical_path.trim_end_matches('/');
        self.0.iter().any(|p| p.trim_end_matches('/') == probe)
    }
}

fn raw_link(target: &str) -> RawLink {
    RawLink {
        kind: LinkKind::Link,
        wikilink: false,
        target: target.to_string(),
        anchor: None,
        line: 1,
        external: false,
    }
}

/// An extensionless repository file and a directory both point at something
/// real. Before `honest-link-grades` markdown had no existence check at all, so
/// each was reported dangling — five of the seven rows `kenn check links`
/// produced on this repo.
#[test]
fn an_existing_target_that_is_not_indexed_becomes_a_path_keyed_attachment() {
    let ws = Present(&["LICENSE-MIT", "docs", "indexers/frames.ts"]);
    assert_eq!(
        attachment_key(&raw_link("LICENSE-MIT"), "README.md", &ws).as_deref(),
        Some("LICENSE-MIT")
    );
    // A directory reference: the trailing slash normalizes away.
    assert_eq!(
        attachment_key(&raw_link("docs/"), "README.md", &ws).as_deref(),
        Some("docs")
    );
}

/// The key is the *canonical* path, so two documents at different depths that
/// name one on-disk target produce one node — the property `list_usages`
/// depends on, and the reason HTML already keys its asset stubs this way.
#[test]
fn every_spelling_of_one_target_produces_one_key() {
    let ws = Present(&["LICENSE-MIT"]);
    let from_root = attachment_key(&raw_link("LICENSE-MIT"), "README.md", &ws);
    let from_crate = attachment_key(
        &raw_link("../../LICENSE-MIT"),
        "crates/kenn-indexer/README.md",
        &ws,
    );
    assert_eq!(from_root, from_crate);
    assert_eq!(from_root.as_deref(), Some("LICENSE-MIT"));
}

/// A directory written with and without a trailing slash is one directory, so
/// it must key one node. The first cut of this change returned the written
/// spelling verbatim when it matched, minting `md:@attachment/docs/` alongside
/// `md:@attachment/docs` — visible only after reindexing the real workspace.
#[test]
fn a_trailing_slash_does_not_fork_the_attachment_key() {
    let ws = Present(&["docs"]);
    let slashed = attachment_key(&raw_link("docs/"), "README.md", &ws);
    let bare = attachment_key(&raw_link("docs"), "README.md", &ws);
    assert_eq!(slashed, bare);
    assert_eq!(bare.as_deref(), Some("docs"));
}

/// A relative target must bind to the path it names, not to a same-named path
/// at the repository root. The first cut probed root-relative first, which — on
/// a filesystem where `docs`, `src` and `tests` exist at several depths —
/// silently resolved a nested link to the root directory.
#[test]
fn a_relative_target_binds_nearest_not_to_the_root() {
    let ws = Present(&["docs", "crates/kenn-indexer/docs"]);
    assert_eq!(
        attachment_key(&raw_link("docs"), "crates/kenn-indexer/README.md", &ws).as_deref(),
        Some("crates/kenn-indexer/docs")
    );
    // With no nested candidate, the root-relative fallback still applies.
    let root_only = Present(&["docs"]);
    assert_eq!(
        attachment_key(
            &raw_link("docs"),
            "crates/kenn-indexer/README.md",
            &root_only
        )
        .as_deref(),
        Some("docs")
    );
}

/// Existence is the whole gate: a target the workspace does not hold is still
/// broken, and must keep dangling by its written string.
#[test]
fn a_target_the_workspace_does_not_hold_is_not_an_attachment() {
    let ws = Present(&["LICENSE-MIT"]);
    assert_eq!(
        attachment_key(&raw_link("missing-file"), "README.md", &ws),
        None
    );
}

/// A bare inline destination is a *path* by `CommonMark`'s reading, so an
/// existing directory must not be shadowed by a code symbol that happens to
/// share its name — `[the docs](docs)` in a README means the directory, not a
/// `fn docs`. A wikilink is the opposite convention and keeps symbol-first.
#[test]
fn a_bare_inline_name_prefers_an_existing_path_over_a_symbol() {
    // `is_code_path` is what routes a target to the symbol branch; a bare name
    // is exactly the case that can be shadowed.
    assert!(!crate::markdown::is_code_path("docs"));
    assert!(crate::markdown::is_code_path("src/order.rs"));
    assert!(crate::markdown::is_code_path("order.rs"));

    // The bare name resolves as a path when the workspace holds one...
    let ws = Present(&["docs"]);
    assert_eq!(
        attachment_key(&raw_link("docs"), "README.md", &ws).as_deref(),
        Some("docs")
    );
    // ...and not when it does not, leaving the symbol branch to answer.
    let empty = Present(&[]);
    assert_eq!(attachment_key(&raw_link("docs"), "README.md", &empty), None);
}

/// An attachment's sections are unknown — the target is not in the corpus — so
/// an anchor on it cannot be verified and the edge must not claim `exact`.
/// Mirrors `apply_anchor`, which downgrades an unmatched md↔md anchor.
#[test]
fn an_unverifiable_anchor_downgrades_the_attachment_grade() {
    let ws = Present(&["vendor/CHANGELOG.md"]);
    let anchored = RawLink {
        anchor: Some("v1-0-0".into()),
        ..raw_link("vendor/CHANGELOG.md")
    };
    // The key is unaffected — the anchor addresses a place *inside* the target.
    assert_eq!(
        attachment_key(&anchored, "README.md", &ws).as_deref(),
        Some("vendor/CHANGELOG.md")
    );
    assert_eq!(attachment_grade(&anchored), LinkGrade::Drifted);
    assert_eq!(
        attachment_grade(&raw_link("vendor/CHANGELOG.md")),
        LinkGrade::Exact
    );
}

/// D4 applies the existence check to *every* unresolved target, so a wikilink —
/// a bare name, not a path — resolves when the workspace holds a path of that
/// name and dangles otherwise.
///
/// The first cut of this test passed `raw_link("[[gone]]")`, a string
/// `extract_links` can never produce: it strips the brackets, yielding
/// `RawLink { wikilink: true, target: "gone" }`. The assertion therefore only
/// restated the `missing-file` case above and could not go red for any change
/// to wikilink handling (CLAUDE.md §9).
#[test]
fn a_wikilink_resolves_by_existence_of_its_bare_name() {
    let ws = Present(&["docs"]);
    let hit = RawLink {
        wikilink: true,
        ..raw_link("docs")
    };
    assert_eq!(
        attachment_key(&hit, "README.md", &ws).as_deref(),
        Some("docs")
    );
    let miss = RawLink {
        wikilink: true,
        ..raw_link("gone")
    };
    assert_eq!(attachment_key(&miss, "README.md", &ws), None);
}

/// A target whose `..` segments walk above the workspace root has no in-corpus
/// canonical path, so it cannot key a shared node.
#[test]
fn a_target_above_the_workspace_root_is_not_an_attachment() {
    let ws = Present(&["LICENSE-MIT"]);
    assert_eq!(
        attachment_key(&raw_link("../../../LICENSE-MIT"), "a/b.md", &ws),
        None
    );
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
    resolve_markdown_code(pending, None, &FsPaths { workspace_root: ws }, p2).expect("resolve");
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
    resolve_markdown_code(
        pending,
        Some((&reader, rt.handle())),
        &FsPaths { workspace_root: ws },
        p2,
    )
    .expect("resolve");
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

/// One indexed Rust file (`src/order.rs`) plus a symbol and its def, so the file
/// has the def row a real ingest would give it. Extracted from
/// `md_to_code_file_link_uses_links_to_file_edge` to keep that test under the
/// pedantic 100-line limit.
fn rust_file_batch(code_file: kenn_model::ShortId) -> kenn_store::api::WriteBatch {
    use kenn_model::{compose_short_id, DefRecord, FileRecord};
    let mut b = kenn_store::api::WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/order.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
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
    b
}

/// A bare inline destination naming an existing directory must resolve to the
/// directory, not to a code symbol that shares its name. Store-backed on
/// purpose: the unit guards above cover `attachment_key` and `is_code_path`
/// separately, and a mutation of the `path_wins` wiring in
/// `resolve_markdown_code` survives both (CLAUDE.md §9).
#[test]
fn an_existing_directory_is_not_shadowed_by_a_same_named_symbol() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord};
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("docs")).unwrap();
    // The link target: a real directory, and a code symbol of the same name.
    // The destination must be a *bare name* — a slash would route it to the
    // file branch of `resolve_code_link` and never reach the symbol lookup
    // that does the shadowing.
    fs::create_dir_all(ws.join("docs/notes")).unwrap();
    fs::write(
        ws.join("docs/guide.md"),
        "# Guide\nsee [the notes](notes)\n",
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
    let sym = compose_short_id(Language::Rust, 2);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/lib.rs".into(),
        language: Language::Rust,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: sym,
        pub_id: "rs:lib::notes".into(),
        language: Language::Rust,
        pkg_id: 0,
        kind: Kind::Function,
        name: "notes".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: sym,
        file_id: code_file,
        start_line: 1,
        start_col: 0,
        end_line: 2,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    rt.block_on(writer.write_batch(&b)).expect("write code");

    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_c, pending) = ingest_markdown_phase1(&cfg(), ws, p1).expect("phase1");
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    resolve_markdown_code(
        pending,
        Some((&reader, rt.handle())),
        &FsPaths { workspace_root: ws },
        p2,
    )
    .expect("resolve");
    drop(reader);

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    // The symbol gains no backlink — the directory won.
    let (_inbound, to_symbol) = rt
        .block_on(Reader::list_inbound(
            &reader,
            sym,
            "links_to",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(
        to_symbol, 0,
        "a bare name must not resolve to a code symbol when the workspace holds that path"
    );
    // ...and the attachment node exists.
    assert!(rt
        .block_on(Reader::fetch_symbol(
            &reader,
            "markdown",
            "md:@attachment/docs/notes"
        ))
        .expect("fetch")
        .is_some());
}

/// An md→code link to a source *file* emits a `links_to_file` edge (not
/// `links_to`), so the code file gains a sound backlink to the md section.
#[test]
fn md_to_code_file_link_uses_links_to_file_edge() {
    use kenn_model::compose_short_id;
    use kenn_store::api::Reader;

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("docs")).unwrap();
    fs::write(
        ws.join("docs/guide.md"),
        "# Guide\nsee [src](src/order.rs)\n",
    )
    .unwrap();
    // The target exists on disk *and* is an indexed code file — the realistic
    // case, and the one that pins the resolution order (design D4): graph
    // resolution wins, so this is a `links_to_file` edge to the file node and
    // NOT an attachment stub. Without the file on disk the existence rung is
    // unreachable here and the ordering goes unguarded.
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/order.rs"), "pub fn handle() {}\n").unwrap();

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
    rt.block_on(writer.write_batch(&rust_file_batch(code_file)))
        .expect("write code batch");

    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_files, pending) = ingest_markdown_phase1(&cfg(), ws, p1).expect("phase1");
    assert_eq!(pending.deferred.len(), 1);

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    resolve_markdown_code(
        pending,
        Some((&reader, rt.handle())),
        &FsPaths { workspace_root: ws },
        p2,
    )
    .expect("resolve");
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
    resolve_markdown_code(
        pending,
        Some((&reader, rt.handle())),
        &FsPaths { workspace_root: ws },
        p2,
    )
    .expect("resolve");
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
