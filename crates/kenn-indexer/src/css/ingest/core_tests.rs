use super::*;
use std::fs;
use tempfile::TempDir;

fn cfg() -> CssConfig {
    CssConfig {
        enabled: true,
        roots: vec![".".into()],
        ..Default::default()
    }
}

#[test]
fn ingests_css_records_into_the_store() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/button.css"), ".btn { color: red }\n#app {}\n").unwrap();
    // a Sass file is discovered but skipped (deferred) — must not fail.
    fs::write(ws.join("src/theme.scss"), "$c: red;\n.x { color: $c }\n").unwrap();

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
    let (counts, _pending) = ingest_css_phase1(&cfg(), ws, sink).expect("ingest");
    // The `.css` file is parsed (module + btn + app); the `.scss` entry is
    // also compiled when dart-sass is discoverable (so files is 1 or 2).
    assert!(counts.files >= 1);
    assert!(counts.symbols >= 3);
    assert!(counts.edges >= 3);
}

/// The class registry is served by the store's `css_class` nodes, queryable
/// by name: a class defined in two files returns both defs (keep-all for an
/// ambiguous name), a uniquely-named class returns one.
#[test]
fn class_registry_is_queryable_by_name_keeping_all_defs() {
    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("a")).unwrap();
    fs::create_dir_all(ws.join("b")).unwrap();
    fs::write(ws.join("a/one.css"), ".btn { color: red }\n.solo {}\n").unwrap();
    fs::write(ws.join("b/two.css"), ".btn { color: blue }\n").unwrap();

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
    ingest_css_phase1(&cfg(), ws, sink).expect("ingest");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    // `.btn` is defined in both files → both defs returned (ambiguous keep-all).
    let btn = rt
        .block_on(reader.symbols_by_short_name("btn"))
        .expect("query btn");
    let mut paths: Vec<&str> = btn.iter().map(|h| h.relpath.as_str()).collect();
    paths.sort_unstable();
    assert_eq!(paths, ["a/one.css", "b/two.css"]);
    assert!(btn.iter().all(|h| h.qualified.ends_with("#class:btn")));
    // a uniquely-named class resolves to exactly one def.
    let solo = rt
        .block_on(reader.symbols_by_short_name("solo"))
        .expect("query solo");
    assert_eq!(solo.len(), 1);
}

/// CSS-internal graph: `@import` between stylesheets produces an `imports`
/// edge between their module nodes (the dead-stylesheet-detection basis).
#[test]
fn css_internal_import_edges() {
    use kenn_store::api::Reader;

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/main.css"), "@import \"base.css\";\n.x {}\n").unwrap();
    fs::write(ws.join("src/base.css"), ".y {}\n").unwrap();

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
    let sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    ingest_css_phase1(&cfg(), ws, sink).expect("ingest");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let base = rt
        .block_on(Reader::fetch_symbol(&reader, "css", "css:src/base.css"))
        .expect("fetch")
        .expect("base module node");
    assert_eq!(base.kind, "module");
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            base.id,
            "imports",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(total, 1);
    assert_eq!(inbound[0].pub_id, "css:src/main.css");
}

/// CSS-Modules `composes` produces `extends_rule` edges between class nodes:
/// same-file (`composes: base`) and cross-file (`composes: big from './u'`).
/// A target that resolves to nothing emits no edge.
#[test]
fn composes_extends_rule_edges() {
    use kenn_store::api::Reader;

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/u.css"), ".big { font-size: 2rem }\n").unwrap();
    fs::write(
        ws.join("src/main.css"),
        ".base { color: red }\n\
             .card { composes: base; }\n\
             .hero { composes: big from './u.css'; }\n\
             .ghost { composes: nope; }\n",
    )
    .unwrap();

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
    let sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    ingest_css_phase1(&cfg(), ws, sink).expect("ingest");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    // same-file: .card → .base
    let base = rt
        .block_on(Reader::fetch_symbol(
            &reader,
            "css",
            "css:src/main.css#class:base",
        ))
        .expect("fetch")
        .expect("base class node");
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            base.id,
            "extends_rule",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound base");
    assert_eq!(total, 1);
    assert_eq!(inbound[0].pub_id, "css:src/main.css#class:card");
    // cross-file: .hero → .big (in u.css)
    let big = rt
        .block_on(Reader::fetch_symbol(
            &reader,
            "css",
            "css:src/u.css#class:big",
        ))
        .expect("fetch")
        .expect("big class node");
    let (binbound, btotal) = rt
        .block_on(Reader::list_inbound(
            &reader,
            big.id,
            "extends_rule",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound big");
    assert_eq!(btotal, 1);
    assert_eq!(binbound[0].pub_id, "css:src/main.css#class:hero");
    // unresolved `composes: nope` minted no edge (and no stub node).
    assert!(rt
        .block_on(reader.symbols_by_short_name("nope"))
        .expect("query nope")
        .is_empty());
}

/// Regression: a Sass barrel entry that only `@use`s (no selectors of its
/// own) still gets a `module` node and an `imports` edge to the partial —
/// the common Bootstrap/Bulma pattern. Gated on a discoverable dart-sass.
#[test]
fn sass_barrel_entry_gets_module_and_imports() {
    use kenn_config::SassConfig;
    use kenn_store::api::Reader;

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::write(ws.join("main.scss"), "@use 'tokens';\n").unwrap();
    fs::write(ws.join("_tokens.scss"), ".token-x { color: red }\n").unwrap();
    if crate::css::discover_sass_compiler(&SassConfig::default(), ws).is_none() {
        eprintln!("no dart-sass; skipping sass barrel test");
        return;
    }

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
    let sink = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    ingest_css_phase1(&cfg(), ws, sink).expect("ingest");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    // The selector-less barrel still has a module node.
    let main = rt
        .block_on(Reader::fetch_symbol(&reader, "sass", "sass:main.scss"))
        .expect("fetch")
        .expect("main.scss module (selector-less barrel)");
    assert_eq!(main.kind, "module");
    // …and imports the partial it `@use`s.
    let tokens = rt
        .block_on(Reader::fetch_symbol(&reader, "sass", "sass:_tokens.scss"))
        .expect("fetch")
        .expect("_tokens module");
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            tokens.id,
            "imports",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(total, 1);
    assert_eq!(inbound[0].pub_id, "sass:main.scss");
}

/// End-to-end usage barrier: a `.ts` file using `class="btn"` inside a
/// function gets a `uses_css_class` edge from that function to the `.btn`
/// class node (the code→class backlink).
#[test]
fn usage_edge_links_enclosing_symbol_to_class() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord, Kind, SymbolRecord};
    use kenn_store::api::WriteBatch;

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/button.css"), ".btn { color: red }\n").unwrap();
    fs::write(
        ws.join("src/app.ts"),
        "export function App() {\n  return `<div class=\"btn\">`;\n}\n",
    )
    .unwrap();

    let mut config = cfg();
    config.usage_sources = vec!["**/*.ts".into()];

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

    // Simulate a code ingest unit: the `App` function spanning app.ts.
    let code_file = compose_short_id(Language::TypeScript, 1);
    let code_sym = compose_short_id(Language::TypeScript, 2);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/app.ts".into(),
        language: Language::TypeScript,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: code_sym,
        pub_id: "ts:app.App".into(),
        language: Language::TypeScript,
        pkg_id: 0,
        kind: Kind::Function,
        name: "App".into(),
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
        end_line: 3,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    rt.block_on(writer.write_batch(&b)).expect("write code");

    // CSS phase 1: emits the `.btn` registry node + returns the usage files.
    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_counts, pending) = ingest_css_phase1(&config, ws, p1).expect("phase1");
    assert_eq!(pending.usage_files.len(), 1);

    // Post-code barrier: resolve usages against the building store.
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let ucounts = resolve_css_usage(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    assert_eq!(ucounts.edges, 1);
    drop(reader);

    // The `.btn` class node has a `uses_css_class` backlink from `App`.
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let btn_id = rt
        .block_on(reader.symbols_by_short_name("btn"))
        .expect("btn")
        .into_iter()
        .find(|h| h.qualified.contains("#class:"))
        .expect("btn class node")
        .id;
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            btn_id,
            "uses_css_class",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(total, 1);
    assert_eq!(inbound[0].pub_id, "ts:app.App");
}

/// Regression: real code indexers (kenn-ts) emit declaration-line-only def
/// ranges, so a usage in a function *body* finds no enclosing symbol. The
/// edge must still attach — to the file's module node (spec §7.2 fallback) —
/// not be silently dropped. (The other usage tests seed body-covering ranges,
/// which would mask this.)
#[test]
fn usage_falls_back_to_module_when_no_enclosing_symbol() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord, Kind, SymbolRecord};
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/button.css"), ".btn { color: red }\n").unwrap();
    // The `class="btn"` use is on line 2 (the body); App's decl is line 1.
    fs::write(
        ws.join("src/app.ts"),
        "export function App() {\n  return `<div class=\"btn\">`;\n}\n",
    )
    .unwrap();

    let mut config = cfg();
    config.usage_sources = vec!["**/*.ts".into()];

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

    // Code ingest as kenn-ts emits it: a module + contains edge, and the App
    // function with a DECLARATION-LINE-ONLY def range (start == end == 1).
    let code_file = compose_short_id(Language::TypeScript, 1);
    let code_mod = compose_short_id(Language::TypeScript, 2);
    let code_fn = compose_short_id(Language::TypeScript, 3);
    let module_sym = |id, pub_id: &str, kind| SymbolRecord {
        id,
        pub_id: pub_id.into(),
        language: Language::TypeScript,
        pkg_id: 0,
        kind,
        name: pub_id.rsplit(['.', '/']).next().unwrap_or(pub_id).into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    };
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/app.ts".into(),
        language: Language::TypeScript,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols
        .push(module_sym(code_mod, "ts:src/app.ts", Kind::Module));
    b.symbols
        .push(module_sym(code_fn, "ts:app.App", Kind::Function));
    b.defs.push(DefRecord {
        sym_id: code_mod,
        file_id: code_file,
        start_line: 0,
        start_col: 0,
        end_line: 0,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    // decl-line-only: covers line 1, NOT the line-2 body usage.
    b.defs.push(DefRecord {
        sym_id: code_fn,
        file_id: code_file,
        start_line: 1,
        start_col: 0,
        end_line: 1,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    b.edges.push(EdgeRecord {
        src_id: code_mod,
        target_id: code_file,
        properties: EdgeProperties::Contains,
    });
    rt.block_on(writer.write_batch(&b)).expect("write code");

    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_c, pending) = ingest_css_phase1(&config, ws, p1).expect("phase1");
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let ucounts = resolve_css_usage(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    assert_eq!(ucounts.edges, 1);
    drop(reader);

    // The edge attached to the MODULE (fallback), since no symbol covers line 2.
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let btn = rt
        .block_on(reader.symbols_by_short_name("btn"))
        .expect("btn")
        .into_iter()
        .find(|h| h.qualified.contains("#class:"))
        .expect("btn class node")
        .id;
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            btn,
            "uses_css_class",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(total, 1);
    assert_eq!(inbound[0].pub_id, "ts:src/app.ts"); // the module, not App
}

/// CSS-Modules binding: `import s from './card.module.css'` then
/// `s.btnPrimary` / `s['btn-primary']` resolve to `.btn-primary` in THAT file
/// (camel↔kebab fold), graded Exact, attributed to the enclosing function.
#[test]
fn css_module_member_resolves_to_bound_file_class() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord, Kind, SymbolRecord};
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(
        ws.join("src/card.module.css"),
        ".btn-primary { color: red }\n",
    )
    .unwrap();
    fs::write(
        ws.join("src/card.tsx"),
        "import s from './card.module.css';\n\
             export function Card() {\n\
             \u{20}\u{20}return s.btnPrimary + s['btn-primary'];\n\
             }\n",
    )
    .unwrap();

    let mut config = cfg();
    config.usage_sources = vec!["**/*.tsx".into()];

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

    // Simulate a code ingest: the Card function spanning card.tsx.
    let code_file = compose_short_id(Language::TypeScript, 1);
    let code_sym = compose_short_id(Language::TypeScript, 2);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/card.tsx".into(),
        language: Language::TypeScript,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: code_sym,
        pub_id: "ts:card.Card".into(),
        language: Language::TypeScript,
        pkg_id: 0,
        kind: Kind::Function,
        name: "Card".into(),
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
        end_line: 4,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    rt.block_on(writer.write_batch(&b)).expect("write code");

    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_c, pending) = ingest_css_phase1(&config, ws, p1).expect("phase1");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    resolve_css_usage(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    drop(reader);

    // `.btn-primary` in card.module.css has uses_css_class backlinks from Card
    // (both `s.btnPrimary` and `s['btn-primary']` resolve to it).
    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let cls = rt
        .block_on(Reader::fetch_symbol(
            &reader,
            "css",
            "css:src/card.module.css#class:btn-primary",
        ))
        .expect("fetch")
        .expect("btn-primary class node");
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            cls.id,
            "uses_css_class",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert!(
        total >= 1,
        "expected a module-member usage edge, got {total}"
    );
    assert!(inbound.iter().all(|r| r.pub_id == "ts:card.Card"));
}

/// Code→stylesheet imports: a `.ts` file `import './button.css'` produces a
/// module→module `imports` edge from the app.ts module to the button.css
/// module. A missing stylesheet and a non-stylesheet (`.json`) import emit
/// nothing (no dangling stubs).
#[test]
fn code_style_import_links_module_to_stylesheet() {
    use kenn_model::{compose_short_id, DefRecord, FileRecord, Kind, SymbolRecord};
    use kenn_store::api::{Reader, WriteBatch};

    let dir = TempDir::new().unwrap();
    let ws = dir.path();
    fs::create_dir_all(ws.join("src")).unwrap();
    fs::write(ws.join("src/button.css"), ".btn { color: red }\n").unwrap();
    fs::write(
        ws.join("src/app.ts"),
        "import './button.css';\nimport './missing.scss';\nimport './data.json';\n",
    )
    .unwrap();

    let mut config = cfg();
    config.usage_sources = vec!["**/*.ts".into()];

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

    // Simulate a code ingest: app.ts file + its module node + contains edge.
    let code_file = compose_short_id(Language::TypeScript, 1);
    let code_mod = compose_short_id(Language::TypeScript, 2);
    let mut b = WriteBatch::default();
    b.files.push(FileRecord {
        id: code_file,
        path: "src/app.ts".into(),
        language: Language::TypeScript,
        test: false,
        external: false,
        content_hash: 1,
    });
    b.symbols.push(SymbolRecord {
        id: code_mod,
        pub_id: "ts:src/app.ts".into(),
        language: Language::TypeScript,
        pkg_id: 0,
        kind: Kind::Module,
        name: "app.ts".into(),
        enclosing_sym_id: 0,
        partial: false,
        nargs: 0,
        targs: 0,
        external: false,
        test: false,
    });
    b.defs.push(DefRecord {
        sym_id: code_mod,
        file_id: code_file,
        start_line: 1,
        start_col: 0,
        end_line: 3,
        end_col: 0,
        body_start_line: 0,
        body_end_line: 0,
    });
    b.edges.push(EdgeRecord {
        src_id: code_mod,
        target_id: code_file,
        properties: EdgeProperties::Contains,
    });
    rt.block_on(writer.write_batch(&b)).expect("write code");

    let p1 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let (_c, pending) = ingest_css_phase1(&config, ws, p1).expect("phase1");

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let p2 = BatchSink::new(writer.clone(), rt.handle().clone(), 16);
    let ucounts = resolve_css_usage(pending, Some((&reader, rt.handle())), p2).expect("resolve");
    // Only button.css resolves; missing.scss (no node) and data.json (not a
    // stylesheet) emit nothing.
    assert_eq!(ucounts.import_edges, 1);
    drop(reader);

    let reader = rt
        .block_on(kenn_store::reader_from_writer(&writer))
        .expect("reader");
    let btn = rt
        .block_on(Reader::fetch_symbol(&reader, "css", "css:src/button.css"))
        .expect("fetch")
        .expect("button.css module");
    let (inbound, total) = rt
        .block_on(Reader::list_inbound(
            &reader,
            btn.id,
            "imports",
            50,
            None,
            &kenn_store::RowNarrow::visibility(false, true),
        ))
        .expect("list_inbound");
    assert_eq!(total, 1);
    assert_eq!(inbound[0].pub_id, "ts:src/app.ts");
}
