use super::*;
use crate::html::parse::parse_elements;

/// A document node id in the `html` space (the link source).
fn doc() -> ShortId {
    compose_short_id(Language::Html, 1)
}

/// Build a workspace file set from `(relpath, n)` pairs (n → html short id).
fn files(pairs: &[(&str, u32)]) -> WorkspaceFiles {
    WorkspaceFiles::new(
        pairs
            .iter()
            .map(|(p, n)| ((*p).to_string(), compose_short_id(Language::Html, *n))),
    )
}

/// An on-disk asset set for [`AssetIndex`] tests.
struct Assets(std::collections::HashSet<String>);
impl AssetIndex for Assets {
    fn exists(&self, canonical_path: &str) -> bool {
        self.0.contains(canonical_path)
    }
}
fn assets(paths: &[&str]) -> Assets {
    Assets(paths.iter().map(|p| (*p).to_string()).collect())
}

/// `<a href>` resolution with empty fragment/asset lookups (Phase-1 cases).
fn anchors(html: &str, relpath: &str, files: &dyn CodeLookup) -> (Vec<EdgeRecord>, StubSink) {
    anchors_with(
        html,
        relpath,
        files,
        &FragmentIndex::default(),
        &assets(&[]),
    )
}

/// `<a href>` resolution threading caller-built fragment + asset lookups.
fn anchors_with(
    html: &str,
    relpath: &str,
    files: &dyn CodeLookup,
    frags: &FragmentIndex,
    assets: &dyn AssetIndex,
) -> (Vec<EdgeRecord>, StubSink) {
    let els = parse_elements(html);
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    let edges = anchor_link_edges(
        &els,
        relpath,
        doc(),
        files,
        frags,
        assets,
        &mut ids,
        &mut stubs,
    );
    (edges, stubs)
}

fn imports(html: &str, relpath: &str, files: &dyn CodeLookup) -> (Vec<EdgeRecord>, StubSink) {
    let els = parse_elements(html);
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    let edges = import_edges(&els, relpath, doc(), files, &mut ids, &mut stubs);
    (edges, stubs)
}

#[test]
fn anchor_to_other_document_is_links_to_file() {
    let f = files(&[("b.html", 5)]);
    let (edges, stubs) = anchors(r#"<a href="b.html">y</a>"#, "a.html", &f);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].src_id, doc());
    assert_eq!(edges[0].target_id, compose_short_id(Language::Html, 5));
    assert_eq!(
        edges[0].properties,
        EdgeProperties::LinksToFile {
            grade: LinkGrade::Exact
        }
    );
    assert!(stubs.records.is_empty());
}

#[test]
fn anchor_to_missing_target_is_dangling_stub() {
    let f = files(&[("b.html", 5)]);
    let (edges, stubs) = anchors(r#"<a href="gone.html">y</a>"#, "a.html", &f);
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].properties,
        EdgeProperties::LinksTo {
            grade: LinkGrade::Dangling,
            relation: String::new(),
        }
    );
    // Not dropped: a single external stub was minted as the target.
    assert_eq!(stubs.records.len(), 1);
    assert!(stubs.records[0].external);
    assert_eq!(stubs.records[0].pub_id, "html:@unresolved/gone.html");
    assert_eq!(edges[0].target_id, stubs.records[0].id);
}

#[test]
fn anchor_relative_path_resolves_against_linking_dir() {
    // From pages/a.html, ../shared/b.html → shared/b.html (Exact), the same
    // join-relative grading the markdown resolver applies.
    let f = files(&[("shared/b.html", 7)]);
    let (edges, _) = anchors(r#"<a href="../shared/b.html">y</a>"#, "pages/a.html", &f);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, compose_short_id(Language::Html, 7));
    assert_eq!(
        edges[0].properties,
        EdgeProperties::LinksToFile {
            grade: LinkGrade::Exact
        }
    );
}

/// Build a [`FragmentIndex`] from `(relpath, html)` pairs by running the
/// `html_id` pass over each file (task 4.2 input).
fn frag_index(pairs: &[(&str, &str)]) -> FragmentIndex {
    use crate::html::ids::html_id_nodes;
    FragmentIndex::new(pairs.iter().map(|(rel, html)| {
        let els = parse_elements(html);
        let mut ids = HtmlIds::new(1);
        let nodes = html_id_nodes(
            &els,
            rel,
            compose_short_id(Language::Html, 1),
            compose_short_id(Language::Html, 2),
            &mut ids,
        );
        ((*rel).to_string(), nodes.index)
    }))
}

#[test]
fn same_file_fragment_resolves_to_html_id() {
    // <a href="#intro"> + an element id="intro" → LinksTo the intro html_id.
    let frags = frag_index(&[("page.html", r#"<h2 id="intro">x</h2>"#)]);
    let intro = frags.get("page.html", "intro").expect("intro anchor");
    let (edges, stubs) = anchors_with(
        r##"<a href="#intro">a</a>"##,
        "page.html",
        &files(&[]),
        &frags,
        &assets(&[]),
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, intro);
    assert_eq!(edges[0].properties, links_props(LinkGrade::Exact));
    assert!(stubs.records.is_empty());
}

#[test]
fn cross_file_fragment_resolves_against_target_file_anchors() {
    // page.html#intro from a.html → the intro html_id in page.html, NOT a
    // file edge to page.html (the fragment wins).
    let frags = frag_index(&[("page.html", r#"<h2 id="intro">x</h2>"#)]);
    let intro = frags.get("page.html", "intro").unwrap();
    let (edges, _) = anchors_with(
        r#"<a href="page.html#intro">a</a>"#,
        "a.html",
        &files(&[("page.html", 9)]),
        &frags,
        &assets(&[]),
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, intro);
    assert_eq!(edges[0].properties, links_props(LinkGrade::Exact));
}

#[test]
fn unknown_fragment_is_dangling() {
    // The file has anchors but not this one → dangling by the written href.
    let frags = frag_index(&[("page.html", r#"<h2 id="intro">x</h2>"#)]);
    let (edges, stubs) = anchors_with(
        r##"<a href="#missing">a</a>"##,
        "page.html",
        &files(&[]),
        &frags,
        &assets(&[]),
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].properties, links_props(LinkGrade::Dangling));
    assert_eq!(stubs.records.len(), 1);
    assert_eq!(stubs.records[0].pub_id, "html:@unresolved/#missing");
    assert_eq!(edges[0].target_id, stubs.records[0].id);
}

#[test]
fn anchor_to_existing_asset_is_path_keyed_attachment() {
    // <a href="report.pdf"> to a non-indexed file that exists → a LinksTo
    // attachment stub keyed by the canonical path.
    let (edges, stubs) = anchors_with(
        r#"<a href="report.pdf">x</a>"#,
        "docs/index.html",
        &files(&[]),
        &FragmentIndex::default(),
        &assets(&["docs/report.pdf"]),
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].properties, links_props(LinkGrade::Exact));
    assert_eq!(stubs.records.len(), 1);
    assert_eq!(stubs.records[0].pub_id, "html:docs/report.pdf");
    assert_eq!(stubs.records[0].kind, Kind::Attachment);
    assert_eq!(edges[0].target_id, stubs.records[0].id);
}

#[test]
fn external_anchor_is_not_graphed() {
    let f = files(&[]);
    let (edges, stubs) = anchors(
        r#"<a href="https://example.com">x</a><a href="mailto:a@b.c">m</a>"#,
        "a.html",
        &f,
    );
    assert!(edges.is_empty());
    assert!(stubs.records.is_empty());
}

#[test]
fn stylesheet_link_is_an_import_edge() {
    let f = files(&[("app.css", 3)]);
    let (edges, _) = imports(
        r#"<link rel="stylesheet" href="app.css">"#,
        "index.html",
        &f,
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].src_id, doc());
    assert_eq!(edges[0].target_id, compose_short_id(Language::Html, 3));
    assert_eq!(
        edges[0].properties,
        EdgeProperties::Imports {
            kind: ImportKind::Explicit
        }
    );
}

#[test]
fn script_src_is_an_import_edge() {
    let f = files(&[("app.js", 4)]);
    let (edges, _) = imports(r#"<script src="app.js"></script>"#, "index.html", &f);
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, compose_short_id(Language::Html, 4));
    assert_eq!(
        edges[0].properties,
        EdgeProperties::Imports {
            kind: ImportKind::Explicit
        }
    );
}

#[test]
fn multi_token_rel_still_imports_and_non_stylesheet_link_is_ignored() {
    let f = files(&[("app.css", 3)]);
    let html = r#"<link rel="preload stylesheet" href="app.css">
                      <link rel="icon" href="favicon.ico">"#;
    let (edges, stubs) = imports(html, "index.html", &f);
    // Only the stylesheet link imports; the icon link is not an import.
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].target_id, compose_short_id(Language::Html, 3));
    assert!(stubs.records.is_empty());
}

#[test]
fn inline_script_and_external_src_are_skipped() {
    let f = files(&[("app.js", 4)]);
    let html = r#"<script>const x = 1;</script>
                      <script src="https://cdn.example.com/lib.js"></script>"#;
    let (edges, stubs) = imports(html, "index.html", &f);
    assert!(
        edges.is_empty(),
        "inline + external scripts are not imports"
    );
    assert!(stubs.records.is_empty());
}

#[test]
fn missing_stylesheet_is_a_dangling_import_not_dropped() {
    let f = files(&[]);
    let (edges, stubs) = imports(
        r#"<link rel="stylesheet" href="theme.css">"#,
        "index.html",
        &f,
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(
        edges[0].properties,
        EdgeProperties::Imports {
            kind: ImportKind::Explicit
        }
    );
    assert_eq!(stubs.records.len(), 1);
    assert_eq!(stubs.records[0].pub_id, "html:@unresolved/theme.css");
    assert_eq!(edges[0].target_id, stubs.records[0].id);
}

#[test]
fn repeated_reference_yields_one_edge() {
    let f = files(&[("app.css", 3)]);
    let html = r#"<link rel="stylesheet" href="app.css">
                      <link rel="stylesheet" href="app.css">"#;
    let (edges, _) = imports(html, "index.html", &f);
    assert_eq!(edges.len(), 1, "duplicate import deduped");
}

// --- assets (task 4.4) -------------------------------------------------

fn media(
    html: &str,
    relpath: &str,
    assets: &dyn AssetIndex,
    ids: &mut HtmlIds,
    stubs: &mut StubSink,
) -> Vec<EdgeRecord> {
    let els = parse_elements(html);
    asset_link_edges(&els, relpath, doc(), assets, ids, stubs)
}

#[test]
fn img_src_is_links_to_a_path_keyed_attachment_stub() {
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    let edges = media(
        r#"<img src="logo.png">"#,
        "index.html",
        &assets(&["logo.png"]),
        &mut ids,
        &mut stubs,
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].src_id, doc());
    assert_eq!(edges[0].properties, links_props(LinkGrade::Exact));
    assert_eq!(stubs.records.len(), 1);
    assert_eq!(stubs.records[0].pub_id, "html:logo.png");
    assert_eq!(stubs.records[0].kind, Kind::Attachment);
    assert!(stubs.records[0].external);
    assert_eq!(edges[0].target_id, stubs.records[0].id);
}

#[test]
fn other_media_tags_and_site_absolute_paths_resolve() {
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    // <video>/<source>/<iframe> all carry assets; a site-absolute /assets/x
    // roots at the workspace.
    let edges = media(
        r#"<video src="clip.mp4"></video><source src="/assets/track.vtt"><iframe src="frame.pdf"></iframe>"#,
        "pages/index.html",
        &assets(&["pages/clip.mp4", "assets/track.vtt", "pages/frame.pdf"]),
        &mut ids,
        &mut stubs,
    );
    assert_eq!(edges.len(), 3);
    assert!(edges
        .iter()
        .all(|e| e.properties == links_props(LinkGrade::Exact)));
    let pubs: std::collections::HashSet<&str> =
        stubs.records.iter().map(|s| s.pub_id.as_str()).collect();
    assert!(pubs.contains("html:pages/clip.mp4"));
    assert!(pubs.contains("html:assets/track.vtt"));
    assert!(pubs.contains("html:pages/frame.pdf"));
}

#[test]
fn different_spellings_collapse_to_one_stub() {
    // ../logo.png from pages/a.html and pages/b.html both canonicalize to
    // `logo.png` → the SAME attachment stub, across files (shared sink).
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    let avail = assets(&["logo.png"]);
    let a = media(
        r#"<img src="../logo.png">"#,
        "pages/a.html",
        &avail,
        &mut ids,
        &mut stubs,
    );
    let b = media(
        r#"<img src="../logo.png">"#,
        "pages/b.html",
        &avail,
        &mut ids,
        &mut stubs,
    );
    assert_eq!(a.len(), 1);
    assert_eq!(b.len(), 1);
    // One stub, two edges from the two document nodes (same target).
    assert_eq!(stubs.records.len(), 1);
    assert_eq!(stubs.records[0].pub_id, "html:logo.png");
    assert_eq!(a[0].target_id, b[0].target_id);
}

#[test]
fn missing_asset_is_dangling_by_written_string() {
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    let edges = media(
        r#"<img src="gone.png">"#,
        "index.html",
        &assets(&[]),
        &mut ids,
        &mut stubs,
    );
    assert_eq!(edges.len(), 1);
    assert_eq!(edges[0].properties, links_props(LinkGrade::Dangling));
    assert_eq!(stubs.records.len(), 1);
    assert_eq!(stubs.records[0].pub_id, "html:@unresolved/gone.png");
    assert_eq!(stubs.records[0].kind, Kind::Attachment);
}

#[test]
fn external_and_data_uri_media_are_skipped() {
    let mut ids = HtmlIds::new(1000);
    let mut stubs = StubSink::default();
    let edges = media(
        r#"<img src="https://cdn.example.com/a.png"><img src="data:image/png;base64,AAAA">"#,
        "index.html",
        &assets(&[]),
        &mut ids,
        &mut stubs,
    );
    assert!(edges.is_empty());
    assert!(stubs.records.is_empty());
}
