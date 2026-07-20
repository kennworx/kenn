//! The atlas producer (`atlas` capability, task 3): turns the in-memory graph
//! the aggregation step already computed — symbols, files, the symbol→anchor
//! mapping, and the weighted directed aggregate graph — into OKF concepts, one per internal
//! anchor (a crate/package; `resolve_anchors` names it). Pure build + a disk
//! write; renders via [`super::okf`].
//!
//! The unit is the **anchor**, not the `packages` table: cargo crates never land
//! in `packages` (they come through rust-analyzer as symbol monikers), so a
//! package-keyed atlas would miss the whole Rust workspace. Anchors capture every
//! crate.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};

use kenn_model::{
    AggregateEdgeRecord, AggregateNodeRecord, AnalysisFlatCommunityRecord,
    AnalysisNodeMembershipRecord, Kind, ShortId, SymbolRecord,
};

use super::model::{AtlasShape, CentralSymbol, Concept, DomainConcept};
use super::okf;

const UNANCHORED: &str = "<unanchored>";
const MAX_CENTRAL: usize = 8;

/// A domain (cross-package flat community) must span >1 package and clear this
/// member floor to earn a concept — below it, a "community" is noise, not a
/// domain. `MAX_DOMAINS` bounds the axis to the heaviest clusters.
const MIN_DOMAIN_SIZE: usize = 4;
const MAX_DOMAINS: usize = 24;
const MAX_DOMAIN_PKGS: usize = 8;

/// Whether a `db_name` names a code language the atlas maps. Content languages
/// (markdown, text, css, sass, html) are documentation, not code packages: their
/// "symbols" are headings, so they must not seed a package, its central symbols,
/// or its language (design: the atlas maps the code).
fn is_code_lang(db_name: &str) -> bool {
    matches!(
        db_name,
        "rust" | "typescript" | "csharp" | "go" | "python" | "swift"
    )
}

/// Plurality language of a symbol set (ties broken by `db_name` descending, for
/// determinism), matching kenn's own convention. Empty when no symbol resolves.
fn plurality_language(syms: &[ShortId], symbols: &HashMap<ShortId, SymbolRecord>) -> String {
    let mut counts: HashMap<&'static str, usize> = HashMap::new();
    for &s in syms {
        if let Some(r) = symbols.get(&s) {
            *counts.entry(r.language.db_name()).or_default() += 1;
        }
    }
    counts
        .into_iter()
        .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(a.0)))
        .map_or_else(String::new, |(l, _)| l.to_string())
}

const MAX_DEPS: usize = 8;
const MAX_MEMBERS: usize = 6;

/// Full path segments that mark bundled example/sample/demo code. Symbols under
/// one are excluded from domain + central eligibility (like tests), so a demo app
/// referencing a library type never fabricates a "domain" or a "central" symbol.
const EXAMPLE_SEGMENTS: &[&str] = &[
    "example", "examples", "sample", "samples", "demo", "demos", "fixtures",
];

/// Whether `path` lies under an example/sample/demo/fixtures directory segment
/// (case-insensitive, full segment).
fn is_example_path(path: &str) -> bool {
    path.split(['/', '\\'])
        .any(|seg| EXAMPLE_SEGMENTS.contains(&seg.to_ascii_lowercase().as_str()))
}

/// A dominant package subdivides into source-directory `component` concepts only
/// when it has at least `MIN_SUBAREAS` sub-areas each holding at least
/// `MIN_SUBAREA_SYMBOLS` symbols; otherwise it stays one flat concept
/// (atlas-intra-package).
const MIN_SUBAREAS: usize = 2;
const MIN_SUBAREA_SYMBOLS: usize = 5;

/// The number of leading directory segments common to every path in `paths`
/// (filenames dropped first). A symbol's source sub-area is the directory segment
/// just past this depth (D2), so the common package wrapper (`Source`,
/// `Sources/<target>`, `src`, …) is stripped automatically.
fn dir_segments(p: &str) -> Vec<&str> {
    let mut s: Vec<&str> = p.split(['/', '\\']).collect();
    s.pop(); // filename
    s
}

fn common_dir_depth(paths: &[&str]) -> usize {
    let Some((first, rest)) = paths.split_first() else {
        return 0;
    };
    let mut prefix = dir_segments(first);
    for p in rest {
        let segs = dir_segments(p);
        let common = prefix
            .iter()
            .zip(segs.iter())
            .take_while(|(a, b)| a == b)
            .count();
        prefix.truncate(common);
        if prefix.is_empty() {
            return 0;
        }
    }
    prefix.len()
}

/// The source sub-area of `path` — its directory segment at `depth` (just past
/// the package's common prefix). `None` for a file directly in the prefix dir (a
/// root-level symbol, no sub-area).
fn subarea_of(path: &str, depth: usize) -> Option<&str> {
    let mut segs: Vec<&str> = path.split(['/', '\\']).collect();
    segs.pop(); // filename
    segs.get(depth).copied()
}

/// Conventional source-wrapper directory names — a package's real root is their
/// PARENT, not the wrapper itself, so the file histogram shows `src` as a
/// directory rather than crowning it the root. Case-insensitive.
const SOURCE_WRAPPERS: &[&str] = &["src", "source", "sources", "lib"];

fn is_source_wrapper(seg: &str) -> bool {
    SOURCE_WRAPPERS.contains(&seg.to_ascii_lowercase().as_str())
}

/// The common leading directory prefix of `paths` (filenames dropped), joined
/// with `/`. Empty when they share no leading directory.
fn common_dir_prefix(paths: &[&str]) -> String {
    let depth = common_dir_depth(paths);
    paths.first().map_or_else(String::new, |p| {
        let mut segs: Vec<&str> = p.split(['/', '\\']).collect();
        segs.pop();
        segs.truncate(depth);
        segs.join("/")
    })
}

/// The package root: the common directory of a package's def-files, with a
/// trailing conventional source-wrapper (`src`, `Source`, …) stripped. Language
/// agnostic — unlike keying on a literal `/src/`, it aligns the `## Files under`
/// heading + per-directory lines for `src/` (Rust/TS), `Source/` (Swift), and
/// flat (Go) layouts alike. Empty when the files share no leading directory (a
/// package spanning several top-level dirs) — the caller falls back to the anchor.
fn package_root(paths: &[&str]) -> String {
    let prefix = common_dir_prefix(paths);
    match prefix.rsplit_once('/') {
        Some((parent, leaf)) if is_source_wrapper(leaf) => parent.to_string(),
        None if is_source_wrapper(&prefix) => String::new(),
        _ => prefix,
    }
}

/// Summarize a package's member files as a per-directory count histogram (D7):
/// each file's directory is its parent relative to `root` (files directly under
/// the root use `(root)`); counts sorted descending, then by directory path.
/// `files` are workspace-relative def-file paths.
fn dir_histogram(files: &[&str], root: &str) -> Vec<(String, u64)> {
    let prefix = format!("{root}/");
    let mut counts: BTreeMap<&str, u64> = BTreeMap::new();
    for f in files {
        let rel = f.strip_prefix(&prefix).unwrap_or(f);
        let dir = rel.rsplit_once('/').map_or("(root)", |(d, _)| d);
        *counts.entry(dir).or_default() += 1;
    }
    let mut out: Vec<(String, u64)> = counts
        .into_iter()
        .map(|(d, c)| (d.to_string(), c))
        .collect();
    out.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Conventional root-module filenames per language, highest precedence first
/// (task 3.4). A package's `description` is seeded from this file's module doc.
/// Empty for languages with no file-level module doc (e.g. C#, whose docs are
/// per-member XML) — those packages get no seeded description.
fn root_file_names(language: &str) -> &'static [&'static str] {
    match language {
        "rust" => &["lib.rs", "main.rs"],
        "typescript" => &["index.ts", "index.tsx", "main.ts"],
        "python" => &["__init__.py", "__main__.py"],
        "go" => &["doc.go"],
        _ => &[],
    }
}

/// The final path segment of `p` (its filename).
fn basename(p: &str) -> &str {
    p.rsplit(['/', '\\']).next().unwrap_or(p)
}

/// Pick a package's root-module file id (task 3.4): among `pkg_files` (id, path),
/// the file matching the highest-precedence language root, ties broken by the
/// shallowest then lexicographically-first path (determinism). `None` when the
/// language has no root convention or no file matches.
fn pick_root_file(pkg_files: &[(ShortId, &str)], language: &str) -> Option<ShortId> {
    root_file_names(language).iter().find_map(|&want| {
        pkg_files
            .iter()
            .filter(|(_, p)| basename(p) == want)
            .min_by(|a, b| {
                a.1.matches('/')
                    .count()
                    .cmp(&b.1.matches('/').count())
                    .then_with(|| a.1.cmp(b.1))
            })
            .map(|&(id, _)| id)
    })
}

/// Subdivide a dominant package into source-directory `component` concepts
/// (atlas-intra-package). Called only for the dominant anchor; returns empty when
/// the package is flat or its sub-areas don't clear the thresholds. Components
/// describe the PRODUCTION structure — test and example symbols are excluded so
/// the common prefix is the source root and the areas are its real subdirs.
#[expect(
    clippy::too_many_arguments,
    reason = "the projected slices/maps the component pass needs; grouping them into a struct only adds indirection"
)]
fn build_components(
    anchor: &str,
    language: &str,
    syms: &[ShortId],
    anchor_central: &[ShortId],
    symbols: &HashMap<ShortId, SymbolRecord>,
    files: &HashMap<ShortId, String>,
    primary_def_file: &HashMap<ShortId, ShortId>,
    primary_def_range: &HashMap<ShortId, (u32, u32)>,
    degree: &HashMap<ShortId, u64>,
) -> Vec<Concept> {
    let path_of = |sid: ShortId| -> Option<&str> {
        primary_def_file
            .get(&sid)
            .and_then(|f| files.get(f))
            .map(String::as_str)
    };
    let prod_syms: Vec<ShortId> = syms
        .iter()
        .copied()
        .filter(|&s| symbols.get(&s).is_some_and(|r| !r.test))
        .filter(|&s| path_of(s).is_some_and(|p| !is_example_path(p)))
        .collect();
    let paths: Vec<&str> = prod_syms.iter().filter_map(|&s| path_of(s)).collect();
    let depth = common_dir_depth(&paths);
    let common_prefix = common_dir_prefix(&paths);
    let mut sub_syms: BTreeMap<&str, Vec<ShortId>> = BTreeMap::new();
    for &s in &prod_syms {
        if let Some(sa) = path_of(s).and_then(|p| subarea_of(p, depth)) {
            sub_syms.entry(sa).or_default().push(s);
        }
    }
    sub_syms.retain(|_, v| v.len() >= MIN_SUBAREA_SYMBOLS);
    if sub_syms.len() < MIN_SUBAREAS {
        return Vec::new();
    }
    let mut central_by_sub: BTreeMap<&str, Vec<ShortId>> = BTreeMap::new();
    for &n in anchor_central {
        if let Some(sa) = path_of(n).and_then(|p| subarea_of(p, depth)) {
            if sub_syms.contains_key(sa) {
                central_by_sub.entry(sa).or_default().push(n);
            }
        }
    }
    let mut components = Vec::with_capacity(sub_syms.len());
    for (&sa, sub_ids) in &sub_syms {
        let mut ranked = central_by_sub.get(sa).cloned().unwrap_or_default();
        ranked.sort_by(|&a, &b| {
            degree
                .get(&b)
                .unwrap_or(&0)
                .cmp(degree.get(&a).unwrap_or(&0))
                .then_with(|| symbols[&a].name.cmp(&symbols[&b].name))
        });
        let c_central: Vec<CentralSymbol> = ranked
            .iter()
            .take(MAX_CENTRAL)
            .map(|&s| central_symbol(s, symbols, primary_def_file, files, primary_def_range))
            .collect();
        // A component maps ONE source directory, so it lists ALL its files
        // (unlike a package, which summarizes by a per-directory histogram).
        let mut fc: HashMap<&str, usize> = HashMap::new();
        for &s in sub_ids {
            if let Some(p) = path_of(s) {
                *fc.entry(p).or_default() += 1;
            }
        }
        let mut mv: Vec<(&str, usize)> = fc.into_iter().collect();
        mv.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let c_members: Vec<String> = mv.into_iter().map(|(p, _)| p.to_string()).collect();
        let c_resource = if common_prefix.is_empty() {
            sa.to_string()
        } else {
            format!("{common_prefix}/{sa}")
        };
        components.push(Concept {
            concept_type: "component".to_string(),
            id: okf::concept_id(language, &format!("{anchor}/{sa}")),
            title: format!("{anchor} / {sa}"),
            description: None,
            resource: c_resource,
            language: language.to_string(),
            test: false,
            symbols: sub_ids.len() as u64,
            deps: Vec::new(),
            central: c_central,
            members: c_members,
            file_count: 0,
            dir_counts: Vec::new(),
            parent: Some(okf::concept_id(language, anchor)),
            components: Vec::new(),
        });
    }
    components
}

/// Project an aggregate node (its leaf symbol) into a [`CentralSymbol`]: name,
/// stable `pub_id`, workspace-relative path, and its primary def range. Shared
/// by the package and domain central-symbol lists.
fn central_symbol(
    s: ShortId,
    symbols: &HashMap<ShortId, SymbolRecord>,
    primary_def_file: &HashMap<ShortId, ShortId>,
    files: &HashMap<ShortId, String>,
    primary_def_range: &HashMap<ShortId, (u32, u32)>,
) -> CentralSymbol {
    let (line_start, line_end) = primary_def_range.get(&s).copied().unwrap_or((0, 0));
    let path = primary_def_file
        .get(&s)
        .and_then(|f| files.get(f))
        .map_or_else(String::new, String::clone);
    CentralSymbol {
        name: symbols.get(&s).map_or_else(String::new, |r| r.name.clone()),
        pub_id: symbols
            .get(&s)
            .map_or_else(String::new, |r| r.pub_id.clone()),
        path,
        line_start,
        line_end,
    }
}

/// Build the concept set + shape header from the aggregation-stage data. Every
/// value is a structural fact; no prose is synthesized.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "the producer's inputs are the aggregation-stage maps it projects; grouping them into a struct only adds indirection"
)]
#[expect(
    clippy::too_many_lines,
    reason = "a linear per-anchor producer; splitting the body scatters the shared degree/dep/anchor state"
)]
#[expect(
    clippy::implicit_hasher,
    reason = "the only caller passes std-default HashMaps; generalizing over BuildHasher adds noise"
)]
#[expect(
    clippy::indexing_slicing,
    reason = "every `[..]` is a HashMap keyed by an id taken from the same map (by_anchor/symbols) — absence is impossible"
)]
pub fn build_concepts(
    symbols: &HashMap<ShortId, SymbolRecord>,
    files: &HashMap<ShortId, String>,
    primary_def_file: &HashMap<ShortId, ShortId>,
    aggregate_of: &HashMap<ShortId, ShortId>,
    anchors: &HashMap<ShortId, (u32, String)>,
    nodes: &[AggregateNodeRecord],
    edges: &[AggregateEdgeRecord],
    membership: &[AnalysisNodeMembershipRecord],
    flat: &[AnalysisFlatCommunityRecord],
    primary_def_range: &HashMap<ShortId, (u32, u32)>,
    file_docs: &HashMap<ShortId, String>,
    shape_meta: &ShapeMeta<'_>,
) -> (Vec<Concept>, Vec<DomainConcept>, AtlasShape) {
    // Each internal (non-external) symbol → its anchor name, skipping the
    // unanchored sentinel. Container kinds (namespace/module/package) are excluded:
    // a C# namespace is `pkg = 0` by design (it spans assemblies), so it would
    // path-fall-back to a bare-name anchor (`Billing`) and spawn a shadow
    // concept alongside the real package (`Acme.Billing`). Containers carry no
    // central symbols anyway, so dropping them from the grouping only removes the
    // shadow — language-agnostic, since it keys on the kind the ingester provides.
    let sym_anchor: HashMap<ShortId, &str> = symbols
        .values()
        .filter(|s| {
            !s.external
                && is_code_lang(s.language.db_name())
                && !matches!(s.kind, Kind::Namespace | Kind::Module | Kind::Package)
        })
        .filter_map(|s| {
            let agg = aggregate_of.get(&s.id)?;
            let name = anchors.get(agg).map(|(_, n)| n.as_str())?;
            (name != UNANCHORED).then_some((s.id, name))
        })
        .collect();

    // Aggregate node → anchor name (code, anchored nodes only). The aggregate
    // graph — not the raw per-symbol edges — is the authority for centrality and
    // cross-package deps: it is already weighted (KEPT_EDGE_KINDS) and its
    // containers are collapsed by the rollup, so it ranks real types rather than
    // the namespaces a raw incidence count would crown.
    let node_anchor: HashMap<ShortId, &str> = nodes
        .iter()
        .filter(|n| !n.external && is_code_lang(n.language.db_name()))
        .filter_map(|n| (n.anchor_name != UNANCHORED).then_some((n.id, n.anchor_name.as_str())))
        .collect();

    // Weighted degree per aggregate node = Σ incident aggregate-edge weight
    // (in + out) — the god-node metric (semantic coupling), not a raw count.
    let mut degree: HashMap<ShortId, u64> = HashMap::new();
    for e in edges {
        *degree.entry(e.src_id).or_default() += u64::from(e.weight);
        *degree.entry(e.dst_id).or_default() += u64::from(e.weight);
    }

    // Directed cross-anchor dependency weights: A → B carries the summed weight
    // of every aggregate edge from a node in A to a node in B. Direction is
    // preserved (the aggregate rollup is directed).
    let mut dep_w: HashMap<&str, HashMap<&str, u64>> = HashMap::new();
    for e in edges {
        if let (Some(&a), Some(&b)) = (node_anchor.get(&e.src_id), node_anchor.get(&e.dst_id)) {
            if a != b {
                *dep_w.entry(a).or_default().entry(b).or_default() += u64::from(e.weight);
            }
        }
    }

    // Central-symbol candidates: aggregate nodes grouped by anchor, code +
    // non-container. Containers (module/namespace/package) are the rollup's
    // fallback aggregates, not real types — never central (the `cs:…Admin`
    // namespace bug this rewire fixes at the source).
    //
    // Test handling is by package, not by symbol: a *production* package hides
    // its test classes (they'd crowd out the real API), but a *test-dominant*
    // package — a `*.Test` project with more test classes than production ones —
    // includes them, because testing IS its purpose, so those classes are its
    // central symbols.
    let mut central_nodes: HashMap<&str, Vec<ShortId>> = HashMap::new();
    let mut test_nodes: HashMap<&str, Vec<ShortId>> = HashMap::new();
    // The domain-eligible node set: non-container, non-test, code + anchored — a
    // domain's hub + central list must be real types/functions, never a
    // module/namespace (the same exclusion the package central list applies, so
    // domains aren't crowned by a `tests`/`mod`-named container).
    let mut domain_eligible: HashSet<ShortId> = HashSet::new();
    for n in nodes {
        if n.name.is_empty() || matches!(n.kind, Kind::Package | Kind::Module | Kind::Namespace) {
            continue;
        }
        // Example/sample/demo code never seeds a domain or a central list (like
        // tests): a bundled demo referencing a library type must not fabricate a
        // "domain". It still counts in the package member/symbol totals below.
        let is_example = primary_def_file
            .get(&n.id)
            .and_then(|f| files.get(f))
            .is_some_and(|p| is_example_path(p));
        if is_example {
            continue;
        }
        if let Some(&anchor) = node_anchor.get(&n.id) {
            if n.test {
                test_nodes.entry(anchor).or_default().push(n.id);
            } else {
                domain_eligible.insert(n.id);
                central_nodes.entry(anchor).or_default().push(n.id);
            }
        }
    }
    // Single-dominant: the top code anchor owns at least half the (non-test,
    // non-example) production nodes — computed BEFORE the test-dominant merge
    // below, so a test package's classes don't dilute the share. Gates the
    // intra-package domain rule in `build_domains`.
    let total_prod: usize = central_nodes.values().map(Vec::len).sum();
    let top_prod: usize = central_nodes.values().map(Vec::len).max().unwrap_or(0);
    let single_dominant = total_prod > 0 && top_prod * 2 > total_prod;
    // The dominant anchor (top production-node count) is the one subdivided into
    // source-directory components; `None` unless the repo is single-dominant.
    let dominant_anchor: Option<&str> = single_dominant
        .then(|| {
            central_nodes
                .iter()
                .max_by(|a, b| a.1.len().cmp(&b.1.len()).then_with(|| b.0.cmp(a.0)))
                .map(|(&a, _)| a)
        })
        .flatten();
    let mut test_dominant: HashSet<&str> = HashSet::new();
    for (anchor, mut tests) in test_nodes {
        let prod = central_nodes.get(anchor).map_or(0, Vec::len);
        if tests.len() > prod {
            test_dominant.insert(anchor);
            central_nodes.entry(anchor).or_default().append(&mut tests);
        }
    }

    // Group symbols by anchor.
    let mut by_anchor: HashMap<&str, Vec<ShortId>> = HashMap::new();
    for (&sid, &anchor) in &sym_anchor {
        by_anchor.entry(anchor).or_default().push(sid);
    }

    let path_of = |sid: ShortId| -> Option<&str> {
        primary_def_file
            .get(&sid)
            .and_then(|f| files.get(f))
            .map(String::as_str)
    };

    let mut names: Vec<&str> = by_anchor.keys().copied().collect();
    names.sort_unstable();

    // Per-anchor plurality language, resolved up front: a concept's id carries a
    // language prefix (case-collision safety on case-insensitive filesystems),
    // so dep/domain links to OTHER anchors must look up the target's language to
    // reproduce its exact id.
    let anchor_lang: HashMap<&str, String> = by_anchor
        .iter()
        .map(|(&a, syms)| (a, plurality_language(syms, symbols)))
        .collect();

    let mut concepts = Vec::with_capacity(names.len());
    let mut total_syms: u64 = 0;
    let mut test_syms: u64 = 0;
    for anchor in names {
        let syms = &by_anchor[anchor];
        total_syms += syms.len() as u64;
        test_syms += syms
            .iter()
            .filter(|&&s| symbols.get(&s).is_some_and(|r| r.test))
            .count() as u64;

        let language = anchor_lang.get(anchor).cloned().unwrap_or_default();

        // Central: this anchor's aggregate nodes, ranked by weighted degree (ties
        // by name). The candidate set + container exclusion were computed once in
        // `central_nodes`; here we rank and project the top MAX_CENTRAL. Each node
        // id is the aggregate's leaf symbol, so its pub_id / path / range resolve
        // straight from the symbol maps.
        let mut ranked: Vec<ShortId> = central_nodes.get(anchor).cloned().unwrap_or_default();
        ranked.sort_by(|&a, &b| {
            degree
                .get(&b)
                .unwrap_or(&0)
                .cmp(degree.get(&a).unwrap_or(&0))
                .then_with(|| symbols[&a].name.cmp(&symbols[&b].name))
        });
        let central: Vec<CentralSymbol> = ranked
            .iter()
            .take(MAX_CENTRAL)
            .map(|&s| central_symbol(s, symbols, primary_def_file, files, primary_def_range))
            .collect();

        // Members: distinct def-files, summarized as a per-directory histogram
        // (D7 — total file count + count-per-dir, no top-N truncation).
        let mut file_set: HashSet<&str> = HashSet::new();
        for &s in syms {
            if let Some(p) = path_of(s) {
                file_set.insert(p);
            }
        }
        let mut def_files: Vec<&str> = file_set.into_iter().collect();
        def_files.sort_unstable();
        let file_count = def_files.len() as u64;

        // Directed deps, heaviest first.
        let mut deps: Vec<(&str, u64)> = dep_w
            .get(anchor)
            .map(|m| m.iter().map(|(&k, &v)| (k, v)).collect())
            .unwrap_or_default();
        deps.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let deps: Vec<String> = deps
            .into_iter()
            .take(MAX_DEPS)
            .map(|(a, _)| okf::concept_id(anchor_lang.get(a).map_or("", String::as_str), a))
            .collect();

        // resource: the package root — the common directory of its files minus a
        // conventional source-wrapper (language-agnostic, so the `## Files under`
        // heading + per-dir lines align for non-`src` layouts too). Falls back to
        // the anchor name when the files share no common directory.
        let root = package_root(&def_files);
        let resource = if root.is_empty() {
            anchor.to_string()
        } else {
            root
        };
        let dir_counts = dir_histogram(&def_files, &resource);

        // Description: the module doc of the package's root file, verbatim (3.4 +
        // 8.4). The root file is picked from ALL files under the package root — not
        // just symbol-def files — because a Rust `lib.rs` defines only the crate
        // module + re-exports (no first-class symbol), yet holds the crate's `//!`
        // doc. `None` when the language has no root convention or the file carries
        // no module doc — never synthesized.
        let root_prefix = format!("{resource}/");
        let root_candidates: Vec<(ShortId, &str)> = files
            .iter()
            .filter(|(_, p)| p.starts_with(&root_prefix))
            .map(|(&id, p)| (id, p.as_str()))
            .collect();
        let description = pick_root_file(&root_candidates, &language)
            .and_then(|fid| file_docs.get(&fid))
            .filter(|d| !d.is_empty())
            .cloned();

        // Source-directory components: subdivide the dominant package by its
        // top-level source subdirectories (atlas-intra-package). Non-dominant
        // packages — and every package in a multi-package repo — stay flat.
        let components = if Some(anchor) == dominant_anchor {
            build_components(
                anchor,
                &language,
                syms,
                central_nodes.get(anchor).map_or(&[][..], Vec::as_slice),
                symbols,
                files,
                primary_def_file,
                primary_def_range,
                &degree,
            )
        } else {
            Vec::new()
        };
        let component_ids: Vec<String> = components.iter().map(|c| c.id.clone()).collect();

        concepts.push(Concept {
            concept_type: "package".to_string(),
            id: okf::concept_id(&language, anchor),
            title: anchor.to_string(),
            description,
            resource,
            language,
            test: test_dominant.contains(anchor),
            symbols: syms.len() as u64,
            deps,
            central,
            members: Vec::new(),
            file_count,
            dir_counts,
            parent: None,
            components: component_ids,
        });
        concepts.extend(components);
    }

    // Non-code first-party documents: top-level dirs that hold indexed files but
    // aren't a code package (openspec, docs, claude-plugins, …). Code dirs are
    // whatever the code anchors' files live under (e.g. crates/, indexers/).
    let code_dirs: HashSet<&str> = sym_anchor
        .keys()
        .filter_map(|&s| path_of(s))
        .filter_map(|p| p.split('/').next())
        .collect();
    let mut document_files: BTreeMap<&str, Vec<&str>> = BTreeMap::new();
    for path in files.values() {
        let Some((top, _)) = path.split_once('/') else {
            continue; // root-level files (README, CLAUDE.md) aren't a document dir
        };
        if code_dirs.contains(top) {
            continue;
        }
        document_files.entry(top).or_default().push(path.as_str());
    }
    for (dir, mut paths) in document_files {
        paths.sort_unstable();
        let members = paths
            .iter()
            .take(MAX_MEMBERS)
            .map(|p| (*p).to_string())
            .collect();
        concepts.push(Concept {
            concept_type: "document".to_string(),
            id: format!(
                "documents/{}",
                okf::collapse_underscores(&dir.replace(['/', '\\'], "_"))
            ),
            title: dir.to_string(),
            description: None,
            resource: dir.to_string(),
            language: String::new(),
            test: false,
            symbols: paths.len() as u64, // for a document, the file count
            deps: Vec::new(),
            central: Vec::new(),
            members,
            file_count: 0,
            dir_counts: Vec::new(),
            parent: None,
            components: Vec::new(),
        });
    }

    let mut languages: Vec<String> = concepts
        .iter()
        .map(|c| c.language.clone())
        .filter(|l| !l.is_empty())
        .collect();
    languages.sort_unstable();
    languages.dedup();
    #[expect(
        clippy::cast_possible_truncation,
        reason = "a 0..=100 percentage always fits u8"
    )]
    let test_ratio_pct = (100 * test_syms).checked_div(total_syms).unwrap_or(0) as u8;
    let shape = AtlasShape {
        name: shape_meta.workspace_name.to_string(),
        languages,
        packages: concepts
            .iter()
            .filter(|c| c.concept_type == "package")
            .count(),
        symbols: total_syms,
        test_ratio_pct,
        freshness: shape_meta.freshness.to_string(),
        timestamp: shape_meta.timestamp.to_string(),
    };
    let domains = build_domains(
        membership,
        flat,
        &node_anchor,
        &anchor_lang,
        &domain_eligible,
        &degree,
        symbols,
        primary_def_file,
        files,
        primary_def_range,
        single_dominant,
    );
    (concepts, domains, shape)
}

/// Build the **domain** concepts — cross-package flat-Louvain communities, the
/// second atlas axis. Pure projection of the persisted analysis (`membership` +
/// `flat`, both read back from the snapshot the hook wrote); never recomputes
/// clustering. Only communities that span >1 package (`cross_anchor`) and clear
/// [`MIN_DOMAIN_SIZE`] qualify — a single-package community just duplicates its
/// package concept. `node_anchor` (code + anchored nodes) and `degree` (the
/// weighted-degree metric) are the same maps the package pass computed.
#[expect(
    clippy::too_many_arguments,
    reason = "the same aggregation-stage maps build_concepts already holds; a struct only adds indirection"
)]
fn build_domains(
    membership: &[AnalysisNodeMembershipRecord],
    flat: &[AnalysisFlatCommunityRecord],
    node_anchor: &HashMap<ShortId, &str>,
    anchor_lang: &HashMap<&str, String>,
    eligible: &HashSet<ShortId>,
    degree: &HashMap<ShortId, u64>,
    symbols: &HashMap<ShortId, SymbolRecord>,
    primary_def_file: &HashMap<ShortId, ShortId>,
    files: &HashMap<ShortId, String>,
    primary_def_range: &HashMap<ShortId, (u32, u32)>,
    single_dominant: bool,
) -> Vec<DomainConcept> {
    // Cross-anchor communities are always the domain axis (the structure packages
    // can't show). For a single-dominant repo (a monolithic library), also keep
    // within-anchor communities — otherwise a one-package repo has no domains.
    let keep: HashSet<u32> = flat
        .iter()
        .filter(|f| (f.cross_anchor || single_dominant) && f.size as usize >= MIN_DOMAIN_SIZE)
        .map(|f| f.community_id)
        .collect();
    if keep.is_empty() {
        return Vec::new();
    }

    // Members per kept community, restricted to code + anchored nodes (the same
    // set the package centrality ranks) — a container/external/content node
    // never seeds a domain's central list.
    let mut members: HashMap<u32, Vec<ShortId>> = HashMap::new();
    for m in membership {
        if keep.contains(&m.flat_community_id) && eligible.contains(&m.short_id) {
            members
                .entry(m.flat_community_id)
                .or_default()
                .push(m.short_id);
        }
    }

    let name_of = |s: ShortId| symbols.get(&s).map_or("", |r| r.name.as_str());
    let mut built: Vec<DomainConcept> = Vec::new();
    let mut cids: Vec<u32> = members.keys().copied().collect();
    cids.sort_unstable();
    for cid in cids {
        let Some(nodes) = members.get(&cid) else {
            continue;
        };
        if nodes.len() < MIN_DOMAIN_SIZE {
            continue; // enough code members after filtering to be a real domain
        }
        let mut ranked = nodes.clone();
        ranked.sort_by(|&a, &b| {
            degree
                .get(&b)
                .unwrap_or(&0)
                .cmp(degree.get(&a).unwrap_or(&0))
                .then_with(|| name_of(a).cmp(name_of(b)))
        });
        let central: Vec<CentralSymbol> = ranked
            .iter()
            .take(MAX_CENTRAL)
            .map(|&s| central_symbol(s, symbols, primary_def_file, files, primary_def_range))
            .collect();
        let Some(title) = central
            .first()
            .map(|c| c.name.clone())
            .filter(|t| !t.is_empty())
        else {
            continue; // no nameable hub → not a useful concept
        };

        // Spanned packages, heaviest (most members) first.
        let mut pkg_count: HashMap<&str, usize> = HashMap::new();
        for &s in &ranked {
            if let Some(&a) = node_anchor.get(&s) {
                *pkg_count.entry(a).or_default() += 1;
            }
        }
        let mut pkgs: Vec<(&str, usize)> = pkg_count.into_iter().collect();
        if !single_dominant && pkgs.len() < 2 {
            // In a multi-package repo, a community whose real types (after
            // excluding containers + tests) live in ONE package is that package
            // concept's job, not a domain. A single-dominant repo keeps it — a
            // within-library semantic cluster IS the useful axis there.
            continue;
        }
        pkgs.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(b.0)));
        let packages: Vec<String> = pkgs
            .into_iter()
            .take(MAX_DOMAIN_PKGS)
            .map(|(a, _)| okf::concept_id(anchor_lang.get(a).map_or("", String::as_str), a))
            .collect();

        built.push(DomainConcept {
            id: okf::domain_id(&title),
            title,
            size: ranked.len() as u64,
            packages,
            central,
        });
    }

    // Surface the heaviest domains (ties by title); bound the axis.
    built.sort_by(|a, b| b.size.cmp(&a.size).then_with(|| a.title.cmp(&b.title)));
    built.truncate(MAX_DOMAINS);
    dedupe_domain_ids(&mut built);
    built
}

/// Ensure domain concept ids are unique — two communities can share a hub name
/// (hence id). Deterministic: the sorted-then-truncated order is stable, so a
/// collision always resolves the same way.
fn dedupe_domain_ids(domains: &mut [DomainConcept]) {
    let mut seen: HashSet<String> = HashSet::new();
    for d in domains.iter_mut() {
        if seen.contains(&d.id) {
            let mut n = 2;
            while !seen.insert(format!("{}-{n}", d.id)) {
                n += 1;
            }
            d.id = format!("{}-{n}", d.id);
        } else {
            seen.insert(d.id.clone());
        }
    }
}

/// What the pipeline threads in so the producer can write the bundle: the output
/// directory (the run's `atlas/`, carried on publish) plus the header facts the
/// aggregation stage doesn't know (workspace name, freshness, timestamp).
pub struct AtlasContext {
    pub out_dir: PathBuf,
    /// Repo root — used to drop concepts whose dir is gitignored.
    pub source_root: PathBuf,
    pub workspace_name: String,
    pub freshness: String,
    pub timestamp: String,
}

/// Header facts that don't come from the per-anchor loop. Languages, package
/// count, symbol count, and the test ratio are all derived inside
/// [`build_concepts`] from the code symbols themselves.
pub struct ShapeMeta<'a> {
    pub workspace_name: &'a str,
    pub freshness: &'a str,
    pub timestamp: &'a str,
}

/// Write the bundle to `out_dir`: one concept file per package under
/// `packages/`, the reserved `index.md`, and an append-preserved `log.md`.
/// Returns the `index.md` path.
///
/// # Errors
/// Any filesystem error while creating the bundle directory or writing a file.
pub fn write_bundle(
    out_dir: &Path,
    shape: &AtlasShape,
    concepts: &[Concept],
    domains: &[DomainConcept],
) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(out_dir.join("packages"))?;
    for c in concepts {
        // Concept id is `packages/<name>`; the file is `<id>.md` under out_dir.
        let path = out_dir.join(format!("{}.md", c.id));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, okf::render_concept(c))?;
    }
    for d in domains {
        // Domain id is `domains/<hub-slug>`; same `<id>.md` layout.
        let path = out_dir.join(format!("{}.md", d.id));
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(path, okf::render_domain(d))?;
    }
    let index_path = out_dir.join(okf::INDEX_MD);
    std::fs::write(&index_path, okf::render_index(shape, concepts, domains))?;

    let log_path = out_dir.join(okf::LOG_MD);
    let existing = std::fs::read_to_string(&log_path).ok();
    let date = shape
        .timestamp
        .split('T')
        .next()
        .unwrap_or(&shape.timestamp);
    let summary = format!(
        "Indexed {} packages ({} symbols) at {}.",
        concepts.len(),
        shape.symbols,
        shape.freshness
    );
    std::fs::write(
        &log_path,
        okf::render_log(existing.as_deref(), date, &summary),
    )?;
    Ok(index_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_model::{EdgeKind, Kind, Language};

    fn sym(id: ShortId, name: &str, lang: Language, external: bool, test: bool) -> SymbolRecord {
        SymbolRecord {
            id,
            pub_id: format!("rs:pkg::{name}"),
            language: lang,
            pkg_id: 0,
            kind: Kind::Function,
            name: name.to_string(),
            enclosing_sym_id: 0,
            partial: false,
            nargs: 0,
            targs: 0,
            external,
            test,
        }
    }

    fn meta() -> ShapeMeta<'static> {
        ShapeMeta {
            workspace_name: "ws",
            freshness: "HEAD abc",
            timestamp: "2026-07-14T00:00:00Z",
        }
    }

    /// A Rust aggregate node. `kind` drives the container-exclusion rule
    /// (module/namespace/package are dropped from central symbols).
    fn agg_node(
        id: ShortId,
        name: &str,
        kind: Kind,
        anchor_id: u32,
        anchor: &str,
    ) -> AggregateNodeRecord {
        agg_node_t(id, name, kind, false, anchor_id, anchor)
    }

    fn agg_node_t(
        id: ShortId,
        name: &str,
        kind: Kind,
        test: bool,
        anchor_id: u32,
        anchor: &str,
    ) -> AggregateNodeRecord {
        AggregateNodeRecord {
            id,
            kind,
            name: name.to_string(),
            language: Language::Rust,
            external: false,
            test,
            anchor_id,
            anchor_name: anchor.to_string(),
        }
    }

    fn agg_edge(src: ShortId, dst: ShortId, weight: u32) -> AggregateEdgeRecord {
        AggregateEdgeRecord {
            src_id: src,
            dst_id: dst,
            kind: EdgeKind::Calls,
            weight,
        }
    }

    /// Two rust anchors (alpha: Foo,Bar; beta: Baz), a markdown symbol, an
    /// external symbol, an unanchored symbol, a document dir (docs), and aggregate
    /// edges Foo→Baz (cross-anchor, w3) + Foo→Bar (intra-alpha, w3).
    struct Fx {
        symbols: HashMap<ShortId, SymbolRecord>,
        files: HashMap<ShortId, String>,
        pdf: HashMap<ShortId, ShortId>,
        agg: HashMap<ShortId, ShortId>,
        anchors: HashMap<ShortId, (u32, String)>,
        nodes: Vec<AggregateNodeRecord>,
        edges: Vec<AggregateEdgeRecord>,
        pdr: HashMap<ShortId, (u32, u32)>,
    }

    fn fixture() -> Fx {
        let symbols: HashMap<ShortId, SymbolRecord> = [
            sym(1, "Foo", Language::Rust, false, false),
            sym(2, "Bar", Language::Rust, false, false),
            sym(3, "Baz", Language::Rust, false, false),
            sym(4, "Heading", Language::Markdown, false, false),
            sym(5, "Ext", Language::Rust, true, false),
            sym(6, "Orphan", Language::Rust, false, false),
        ]
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
        let files = [
            (10, "alpha/src/foo.rs"),
            (11, "alpha/src/bar.rs"),
            (20, "beta/src/baz.rs"),
            (30, "docs/readme.md"),
        ]
        .into_iter()
        .map(|(i, p)| (i, p.to_string()))
        .collect();
        let pdf = [(1, 10), (2, 11), (3, 20), (4, 30), (6, 10)]
            .into_iter()
            .collect();
        let agg = symbols.keys().map(|&k| (k, k)).collect();
        let anchors = [
            (1, (1u32, "alpha".to_string())),
            (2, (1, "alpha".to_string())),
            (3, (2, "beta".to_string())),
            (5, (1, "alpha".to_string())),
            (6, (0, UNANCHORED.to_string())),
        ]
        .into_iter()
        .collect();
        let pdr = [(1, (10u32, 40u32)), (2, (5, 8)), (3, (1, 20))]
            .into_iter()
            .collect();
        Fx {
            symbols,
            files,
            pdf,
            agg,
            anchors,
            nodes: vec![
                agg_node(1, "Foo", Kind::Function, 1, "alpha"),
                agg_node(2, "Bar", Kind::Function, 1, "alpha"),
                agg_node(3, "Baz", Kind::Function, 2, "beta"),
            ],
            edges: vec![agg_edge(1, 3, 3), agg_edge(1, 2, 3)],
            pdr,
        }
    }

    fn build(f: &Fx) -> (Vec<Concept>, AtlasShape) {
        let (concepts, _domains, shape) = build_concepts(
            &f.symbols,
            &f.files,
            &f.pdf,
            &f.agg,
            &f.anchors,
            &f.nodes,
            &f.edges,
            &[],
            &[],
            &f.pdr,
            &HashMap::new(),
            &meta(),
        );
        (concepts, shape)
    }

    fn membership(short_id: ShortId, community: u32) -> AnalysisNodeMembershipRecord {
        AnalysisNodeMembershipRecord {
            short_id,
            flat_community_id: community,
            anchored_leaf_community_id: 0,
        }
    }

    fn flat_community(id: u32, size: u32, cross_anchor: bool) -> AnalysisFlatCommunityRecord {
        AnalysisFlatCommunityRecord {
            community_id: id,
            size,
            total_weight: u64::from(size) * 2,
            cross_anchor,
            primary_anchor_id: 1,
            primary_anchor_name: "alpha".to_string(),
        }
    }

    /// Four code classes across two packages (alpha: A,B; beta: C,D); A is the
    /// hub (heaviest weighted degree). Returns the inputs `build_concepts` needs.
    #[expect(
        clippy::type_complexity,
        reason = "a test fixture returning the aggregation-stage maps build_concepts takes"
    )]
    fn domain_inputs() -> (
        HashMap<ShortId, SymbolRecord>,
        HashMap<ShortId, String>,
        HashMap<ShortId, ShortId>,
        HashMap<ShortId, ShortId>,
        HashMap<ShortId, (u32, String)>,
        Vec<AggregateNodeRecord>,
        Vec<AggregateEdgeRecord>,
    ) {
        let s = |id, name| sym(id, name, Language::Rust, false, false);
        let symbols: HashMap<ShortId, SymbolRecord> = [s(1, "A"), s(2, "B"), s(3, "C"), s(4, "D")]
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let files: HashMap<ShortId, String> = [(10, "alpha/src/a.rs"), (20, "beta/src/c.rs")]
            .into_iter()
            .map(|(i, p)| (i, p.to_string()))
            .collect();
        let pdf: HashMap<ShortId, ShortId> =
            [(1, 10), (2, 10), (3, 20), (4, 20)].into_iter().collect();
        let agg: HashMap<ShortId, ShortId> = (1..=4).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> = [
            (1, (1u32, "alpha".to_string())),
            (2, (1, "alpha".to_string())),
            (3, (2, "beta".to_string())),
            (4, (2, "beta".to_string())),
        ]
        .into_iter()
        .collect();
        let nodes = vec![
            agg_node(1, "A", Kind::Class, 1, "alpha"),
            agg_node(2, "B", Kind::Class, 1, "alpha"),
            agg_node(3, "C", Kind::Class, 2, "beta"),
            agg_node(4, "D", Kind::Class, 2, "beta"),
        ];
        // A is incident to three w3 edges → degree 9, the hub.
        let edges = vec![
            agg_edge(1, 2, 3),
            agg_edge(1, 3, 3),
            agg_edge(1, 4, 3),
            agg_edge(2, 3, 1),
        ];
        (symbols, files, pdf, agg, anchors, nodes, edges)
    }

    fn domains_for(
        membership: &[AnalysisNodeMembershipRecord],
        flat: &[AnalysisFlatCommunityRecord],
    ) -> Vec<DomainConcept> {
        let (symbols, files, pdf, agg, anchors, nodes, edges) = domain_inputs();
        build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &edges,
            membership,
            flat,
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        )
        .1
    }

    #[test]
    fn cross_package_community_becomes_a_domain() {
        let membership = vec![
            membership(1, 1),
            membership(2, 1),
            membership(3, 1),
            membership(4, 1),
        ];
        let flat = vec![flat_community(1, 4, true)];
        let domains = domains_for(&membership, &flat);
        assert_eq!(domains.len(), 1);
        let d = &domains[0];
        assert_eq!(d.title, "A", "hub = highest-weighted-degree member");
        assert_eq!(d.id, "domains/A");
        assert_eq!(d.size, 4);
        assert_eq!(d.central[0].name, "A");
        // Spans both packages (2 members each → tie broken by name).
        assert_eq!(
            d.packages,
            vec![
                okf::concept_id("rust", "alpha"),
                okf::concept_id("rust", "beta")
            ]
        );
    }

    #[test]
    fn single_package_community_is_not_a_domain() {
        // cross_anchor = false → the package axis already covers it.
        let membership = vec![
            membership(1, 1),
            membership(2, 1),
            membership(3, 1),
            membership(4, 1),
        ];
        let flat = vec![flat_community(1, 4, false)];
        assert!(domains_for(&membership, &flat).is_empty());
    }

    #[test]
    fn cross_package_community_below_size_floor_is_filtered() {
        // cross_anchor but only 3 members (< MIN_DOMAIN_SIZE).
        let membership = vec![membership(1, 1), membership(2, 1), membership(3, 1)];
        let flat = vec![flat_community(1, 3, true)];
        assert!(domains_for(&membership, &flat).is_empty());
    }

    #[test]
    fn cross_anchor_flag_but_single_package_after_filtering_is_dropped() {
        // The flat record is flagged cross_anchor (its raw community spanned
        // packages via container/test nodes), but every eligible member resolves
        // to one package → the package concept covers it, so no domain.
        let s = |id, name| sym(id, name, Language::Rust, false, false);
        // Two balanced anchors (alpha holds the community, beta is separate) so the
        // repo is NOT single-dominant — the single-package-after-filtering drop is a
        // multi-package invariant, not the single-dominant intra-package rule.
        let symbols: HashMap<ShortId, SymbolRecord> = [
            s(1, "A"),
            s(2, "B"),
            s(3, "C"),
            s(4, "D"),
            s(5, "E"),
            s(6, "F"),
            s(7, "G"),
            s(8, "H"),
        ]
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
        let files: HashMap<ShortId, String> = [(10, "alpha/src/a.rs"), (30, "beta/src/b.rs")]
            .into_iter()
            .map(|(i, p)| (i, p.to_string()))
            .collect();
        let pdf: HashMap<ShortId, ShortId> = [
            (1, 10),
            (2, 10),
            (3, 10),
            (4, 10),
            (5, 30),
            (6, 30),
            (7, 30),
            (8, 30),
        ]
        .into_iter()
        .collect();
        let agg: HashMap<ShortId, ShortId> = (1..=8).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> = (1..=8)
            .map(|i| {
                let (pkg, name) = if i <= 4 {
                    (1u32, "alpha")
                } else {
                    (2u32, "beta")
                };
                (i, (pkg, name.to_string()))
            })
            .collect();
        let nodes = vec![
            agg_node(1, "A", Kind::Class, 1, "alpha"),
            agg_node(2, "B", Kind::Class, 1, "alpha"),
            agg_node(3, "C", Kind::Class, 1, "alpha"),
            agg_node(4, "D", Kind::Class, 1, "alpha"),
            agg_node(5, "E", Kind::Class, 2, "beta"),
            agg_node(6, "F", Kind::Class, 2, "beta"),
            agg_node(7, "G", Kind::Class, 2, "beta"),
            agg_node(8, "H", Kind::Class, 2, "beta"),
        ];
        let edges = vec![agg_edge(1, 2, 3), agg_edge(1, 3, 3), agg_edge(1, 4, 3)];
        let membership = vec![
            membership(1, 1),
            membership(2, 1),
            membership(3, 1),
            membership(4, 1),
        ];
        let flat = vec![flat_community(1, 4, true)];
        let (_c, domains, _s) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &edges,
            &membership,
            &flat,
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        assert!(
            domains.is_empty(),
            "single-package-after-filtering community is not a domain"
        );
    }

    #[test]
    fn single_dominant_repo_forms_an_intra_package_domain() {
        // One anchor `mono` owns every production node → single-dominant. A
        // community WITHIN it (cross_anchor = false — the package axis alone would
        // never surface it) still becomes a domain, so a monolithic library gets
        // intra-package domains (D4 rule b).
        let s = |id, name| sym(id, name, Language::Rust, false, false);
        let symbols: HashMap<ShortId, SymbolRecord> = [s(1, "A"), s(2, "B"), s(3, "C"), s(4, "D")]
            .into_iter()
            .map(|r| (r.id, r))
            .collect();
        let files: HashMap<ShortId, String> = (1..=4)
            .map(|i| (10 + i, format!("mono/src/m{i}.rs")))
            .collect();
        let pdf: HashMap<ShortId, ShortId> = (1..=4).map(|i| (i, 10 + i)).collect();
        let agg: HashMap<ShortId, ShortId> = (1..=4).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> =
            (1..=4).map(|i| (i, (1u32, "mono".to_string()))).collect();
        let nodes = vec![
            agg_node(1, "A", Kind::Class, 1, "mono"),
            agg_node(2, "B", Kind::Class, 1, "mono"),
            agg_node(3, "C", Kind::Class, 1, "mono"),
            agg_node(4, "D", Kind::Class, 1, "mono"),
        ];
        let edges = vec![agg_edge(1, 2, 3), agg_edge(1, 3, 3), agg_edge(1, 4, 3)];
        let membership = vec![
            membership(1, 1),
            membership(2, 1),
            membership(3, 1),
            membership(4, 1),
        ];
        let flat = vec![flat_community(1, 4, false)]; // NOT cross_anchor
        let (_c, domains, _s) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &edges,
            &membership,
            &flat,
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        // Mutation-check backing (§9): dropping the `|| single_dominant` relaxation
        // in build_domains drops this intra-package (cross_anchor=false) domain.
        assert_eq!(
            domains.len(),
            1,
            "single-dominant repo surfaces an intra-package domain"
        );
        assert_eq!(domains[0].title, "A", "hub = highest-degree member");
    }

    #[test]
    fn example_code_neither_fabricates_a_domain_nor_appears_central() {
        // Two balanced real anchors (lib, other) → the repo is NOT single-dominant.
        // A bundled example (`app/examples/demo.rs`) is the ONLY cross-boundary link
        // into the lib community. Suppressed from domain + central eligibility, the
        // community collapses to one package → no domain, and the demo type is
        // nobody's central symbol (D5).
        let s = |id, name| sym(id, name, Language::Rust, false, false);
        let symbols: HashMap<ShortId, SymbolRecord> = [
            s(1, "Core"),
            s(2, "Util"),
            s(3, "Helper"),
            s(4, "P"),
            s(5, "Q"),
            s(6, "R"),
            s(7, "Demo"),
        ]
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
        let files: HashMap<ShortId, String> = [
            (11, "lib/src/core.rs"),
            (12, "lib/src/util.rs"),
            (13, "lib/src/helper.rs"),
            (14, "other/src/p.rs"),
            (15, "other/src/q.rs"),
            (16, "other/src/r.rs"),
            (17, "app/examples/demo.rs"),
        ]
        .into_iter()
        .map(|(i, p)| (i, p.to_string()))
        .collect();
        let pdf: HashMap<ShortId, ShortId> = (1..=7).map(|i| (i, 10 + i)).collect();
        let agg: HashMap<ShortId, ShortId> = (1..=7).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> = [
            (1, (1u32, "lib".to_string())),
            (2, (1, "lib".to_string())),
            (3, (1, "lib".to_string())),
            (4, (2, "other".to_string())),
            (5, (2, "other".to_string())),
            (6, (2, "other".to_string())),
            (7, (3, "app".to_string())),
        ]
        .into_iter()
        .collect();
        let nodes = vec![
            agg_node(1, "Core", Kind::Class, 1, "lib"),
            agg_node(2, "Util", Kind::Class, 1, "lib"),
            agg_node(3, "Helper", Kind::Class, 1, "lib"),
            agg_node(4, "P", Kind::Class, 2, "other"),
            agg_node(5, "Q", Kind::Class, 2, "other"),
            agg_node(6, "R", Kind::Class, 2, "other"),
            agg_node(7, "Demo", Kind::Class, 3, "app"),
        ];
        let edges = vec![agg_edge(7, 1, 3), agg_edge(1, 2, 3), agg_edge(1, 3, 3)];
        let membership = vec![
            membership(1, 1),
            membership(2, 1),
            membership(3, 1),
            membership(7, 1),
        ];
        let flat = vec![flat_community(1, 4, true)];
        let (concepts, domains, _s) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &edges,
            &membership,
            &flat,
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        // Mutation-check backing (§9): removing the example `continue` in the
        // eligibility loop re-admits Demo → the community spans lib+app → a spurious
        // domain forms and Demo becomes central.
        assert!(
            domains.is_empty(),
            "an example-only cross link fabricates no domain"
        );
        let central: Vec<&str> = concepts
            .iter()
            .flat_map(|c| c.central.iter().map(|s| s.name.as_str()))
            .collect();
        assert!(
            !central.contains(&"Demo"),
            "the example type is nobody's central symbol"
        );
        assert!(
            central.contains(&"Core"),
            "the real library type is still central"
        );
    }

    #[test]
    fn groups_by_anchor_excluding_noncode_external_unanchored() {
        let (concepts, shape) = build(&fixture());
        let pkgs: Vec<&str> = concepts
            .iter()
            .filter(|c| c.concept_type == "package")
            .map(|c| c.title.as_str())
            .collect();
        assert_eq!(pkgs, vec!["alpha", "beta"]);
        assert_eq!(shape.packages, 2);
        assert_eq!(shape.symbols, 3, "only the 3 anchored code symbols");
        assert_eq!(shape.languages, vec!["rust".to_string()]);
        // markdown / external / unanchored appear in no concept.
        let central: Vec<&str> = concepts
            .iter()
            .flat_map(|c| c.central.iter().map(|s| s.name.as_str()))
            .collect();
        assert!(
            !central.contains(&"Heading")
                && !central.contains(&"Ext")
                && !central.contains(&"Orphan")
        );
    }

    #[test]
    fn container_namespace_does_not_form_a_shadow_anchor() {
        // A C# namespace is pkg=0 → path-anchors to the bare dir name
        // ("Billing"), while the real class carries the package name
        // ("Acme.Billing"). The namespace must NOT spawn a shadow concept.
        let ns = SymbolRecord {
            kind: Kind::Namespace,
            ..sym(1, "Billing", Language::Csharp, false, false)
        };
        let cls = SymbolRecord {
            kind: Kind::Class,
            ..sym(2, "Widget", Language::Csharp, false, false)
        };
        let symbols: HashMap<ShortId, SymbolRecord> =
            [ns, cls].into_iter().map(|s| (s.id, s)).collect();
        let agg: HashMap<ShortId, ShortId> = [(1, 1), (2, 2)].into();
        let anchors: HashMap<ShortId, (u32, String)> = [
            (1, (1u32, "Billing".to_string())),   // namespace → path fallback
            (2, (2, "Acme.Billing".to_string())), // class → package name
        ]
        .into_iter()
        .collect();
        let files = [(10, "Billing/src/a.cs"), (20, "Billing/src/b.cs")]
            .into_iter()
            .map(|(i, p)| (i, p.to_string()))
            .collect();
        let pdf = [(1, 10), (2, 20)].into_iter().collect();
        let (concepts, _, _) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &[],
            &[],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        let pkgs: Vec<&str> = concepts
            .iter()
            .filter(|c| c.concept_type == "package")
            .map(|c| c.title.as_str())
            .collect();
        assert!(pkgs.contains(&"Acme.Billing"), "real package present");
        assert!(
            !pkgs.contains(&"Billing"),
            "namespace must not form a shadow package, got {pkgs:?}"
        );
        // …and it must not leak into a `document` either (the dir holds code).
        assert!(!concepts
            .iter()
            .any(|c| c.concept_type == "document" && c.title == "Billing"));
    }

    #[test]
    fn central_ranked_by_degree_with_pubid_and_range() {
        let (concepts, _) = build(&fixture());
        let alpha = concepts.iter().find(|c| c.title == "alpha").unwrap();
        // Foo weighted degree 6 (two incident w3 edges), Bar 3 → Foo first.
        assert_eq!(alpha.central[0].name, "Foo");
        assert_eq!(alpha.central[0].pub_id, "rs:pkg::Foo");
        assert_eq!(
            (alpha.central[0].line_start, alpha.central[0].line_end),
            (10, 40)
        );
        assert_eq!(alpha.central[1].name, "Bar");
    }

    /// The bug this rewire fixes: a container aggregate node (a namespace/module)
    /// with the highest weighted degree must NOT surface as a central symbol —
    /// only the real types under it do. Guards the `cs:…Admin` namespace regression.
    #[test]
    fn container_aggregate_excluded_from_central_despite_highest_degree() {
        let symbols: HashMap<ShortId, SymbolRecord> = [
            sym(1, "Admin", Language::Rust, false, false),
            sym(2, "Widget", Language::Rust, false, false),
            sym(3, "Gadget", Language::Rust, false, false),
        ]
        .into_iter()
        .map(|s| (s.id, s))
        .collect();
        let agg: HashMap<ShortId, ShortId> = symbols.keys().map(|&k| (k, k)).collect();
        let anchors: HashMap<ShortId, (u32, String)> = [
            (1, (1u32, "alpha".to_string())),
            (2, (1, "alpha".to_string())),
            (3, (1, "alpha".to_string())),
        ]
        .into_iter()
        .collect();
        // Admin is a namespace with the heaviest degree (6); Widget/Gadget are
        // real classes (degree 4 each).
        let nodes = vec![
            agg_node(1, "Admin", Kind::Namespace, 1, "alpha"),
            agg_node(2, "Widget", Kind::Class, 1, "alpha"),
            agg_node(3, "Gadget", Kind::Class, 1, "alpha"),
        ];
        let edges = vec![agg_edge(1, 2, 3), agg_edge(1, 3, 3), agg_edge(2, 3, 1)];
        let (concepts, _, _) = build_concepts(
            &symbols,
            &HashMap::new(),
            &HashMap::new(),
            &agg,
            &anchors,
            &nodes,
            &edges,
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        let alpha = concepts.iter().find(|c| c.title == "alpha").unwrap();
        let names: Vec<&str> = alpha.central.iter().map(|s| s.name.as_str()).collect();
        assert!(
            !names.contains(&"Admin"),
            "namespace must not be central, got {names:?}"
        );
        // Both real classes remain, tie broken by name.
        assert_eq!(names, vec!["Gadget", "Widget"]);
    }

    #[test]
    fn central_includes_test_classes_only_for_test_dominant_package() {
        // T: all test (test-dominant) → shows test classes.
        // P: 1 production + 2 test (test-dominant) → includes test classes.
        // R: 2 production + 1 test (production-dominant) → test excluded.
        let s = |id, name, test| sym(id, name, Language::Rust, false, test);
        let symbols: HashMap<ShortId, SymbolRecord> = [
            s(1, "TestA", true),
            s(2, "TestB", true),
            s(3, "Prod", false),
            s(4, "PT1", true),
            s(5, "PT2", true),
            s(6, "R1", false),
            s(7, "R2", false),
            s(8, "RT", true),
        ]
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
        let agg: HashMap<ShortId, ShortId> = (1..=8).map(|i| (i, i)).collect();
        let a = |name: &str| (0u32, name.to_string());
        let anchors: HashMap<ShortId, (u32, String)> = [
            (1, a("T")),
            (2, a("T")),
            (3, a("P")),
            (4, a("P")),
            (5, a("P")),
            (6, a("R")),
            (7, a("R")),
            (8, a("R")),
        ]
        .into_iter()
        .collect();
        let nodes = vec![
            agg_node_t(1, "TestA", Kind::Class, true, 0, "T"),
            agg_node_t(2, "TestB", Kind::Class, true, 0, "T"),
            agg_node_t(3, "Prod", Kind::Class, false, 0, "P"),
            agg_node_t(4, "PT1", Kind::Class, true, 0, "P"),
            agg_node_t(5, "PT2", Kind::Class, true, 0, "P"),
            agg_node_t(6, "R1", Kind::Class, false, 0, "R"),
            agg_node_t(7, "R2", Kind::Class, false, 0, "R"),
            agg_node_t(8, "RT", Kind::Class, true, 0, "R"),
        ];
        // Every node needs an incident edge to earn a weighted degree.
        let edges = vec![
            agg_edge(1, 2, 3),
            agg_edge(3, 4, 3),
            agg_edge(4, 5, 3),
            agg_edge(6, 7, 3),
            agg_edge(7, 8, 3),
        ];
        let (concepts, _, _) = build_concepts(
            &symbols,
            &HashMap::new(),
            &HashMap::new(),
            &agg,
            &anchors,
            &nodes,
            &edges,
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        let central = |title: &str| -> Vec<String> {
            let mut v: Vec<String> = concepts
                .iter()
                .find(|c| c.title == title)
                .unwrap()
                .central
                .iter()
                .map(|s| s.name.clone())
                .collect();
            v.sort();
            v
        };
        assert_eq!(
            central("T"),
            vec!["TestA", "TestB"],
            "test-only → test classes"
        );
        assert_eq!(
            central("P"),
            vec!["PT1", "PT2", "Prod"],
            "test-dominant → production + test classes"
        );
        assert_eq!(
            central("R"),
            vec!["R1", "R2"],
            "production-dominant → test excluded"
        );
        // Test-dominant packages are flagged (→ `tests` tag); production ones not.
        let is_test = |title: &str| concepts.iter().find(|c| c.title == title).unwrap().test;
        assert!(is_test("T") && is_test("P"), "test packages flagged");
        assert!(!is_test("R"), "production package not flagged");
    }

    #[test]
    fn deps_are_directed() {
        let (concepts, _) = build(&fixture());
        let alpha = concepts.iter().find(|c| c.title == "alpha").unwrap();
        let beta = concepts.iter().find(|c| c.title == "beta").unwrap();
        assert_eq!(alpha.deps, vec![okf::concept_id("rust", "beta")]);
        assert!(
            beta.deps.is_empty(),
            "beta has no outgoing cross-anchor edge"
        );
    }

    #[test]
    fn non_code_dir_becomes_a_document_concept() {
        let (concepts, _) = build(&fixture());
        let document = concepts
            .iter()
            .find(|c| c.concept_type == "document")
            .unwrap();
        assert_eq!(document.title, "docs");
        assert!(document.central.is_empty());
        assert_eq!(document.members, vec!["docs/readme.md".to_string()]);
    }

    #[test]
    fn dir_histogram_counts_every_file_per_parent_dir() {
        // D7: every file counted (no top-N cap), grouped by exact parent dir
        // relative to the package root; root-level files use `.`; sorted
        // count-desc then path.
        let files = [
            "Foo/src/Core/a.rs",
            "Foo/src/Core/b.rs",
            "Foo/src/Core/c.rs",
            "Foo/src/Features/d.rs",
            "Foo/src/Features/e.rs",
            "Foo/src/top.rs",
            "Foo/root.rs",
        ];
        let h = dir_histogram(&files, "Foo");
        assert_eq!(
            h,
            vec![
                ("src/Core".to_string(), 3),
                ("src/Features".to_string(), 2),
                ("(root)".to_string(), 1),
                ("src".to_string(), 1),
            ]
        );
    }

    #[test]
    fn package_root_is_language_agnostic_and_strips_source_wrappers() {
        // `src/` layout spanning several top dirs → the parent of the divergence.
        assert_eq!(
            package_root(&["crates/foo/src/a.rs", "crates/foo/tests/b.rs"]),
            "crates/foo"
        );
        // All files under one `src/` → back off the wrapper (Mutation-check §9:
        // dropping the wrapper strip yields `crates/foo/src`, crowning the wrapper).
        assert_eq!(
            package_root(&["crates/foo/src/a.rs", "crates/foo/src/b.rs"]),
            "crates/foo"
        );
        // Flat Go layout, no wrapper → the package dir itself.
        assert_eq!(package_root(&["pkg/foo/a.go", "pkg/foo/b.go"]), "pkg/foo");
        // Swift `Source/` wrapper is stripped like `src`.
        assert_eq!(
            package_root(&["mod/Source/Net/a.swift", "mod/Source/Core/b.swift"]),
            "mod"
        );
        // No shared leading directory → empty (caller falls back to the anchor).
        assert_eq!(package_root(&["a/x.rs", "b/y.rs"]), "");
    }

    #[test]
    fn pick_root_file_is_language_keyed_and_deterministic() {
        let files: &[(ShortId, &str)] = &[
            (1, "crates/foo/src/util.rs"),
            (2, "crates/foo/src/lib.rs"),
            (3, "crates/foo/src/main.rs"),
        ];
        // Rust: lib.rs wins over main.rs (precedence order).
        assert_eq!(pick_root_file(files, "rust"), Some(2));
        // TypeScript keys on index.ts.
        let ts: &[(ShortId, &str)] = &[(5, "pkg/src/other.ts"), (6, "pkg/src/index.ts")];
        assert_eq!(pick_root_file(ts, "typescript"), Some(6));
        // No matching root file → None.
        let leafless: &[(ShortId, &str)] = &[(1, "crates/foo/src/util.rs")];
        assert_eq!(pick_root_file(leafless, "rust"), None);
        // A language with no root convention → None.
        assert_eq!(pick_root_file(files, "csharp"), None);
    }

    #[test]
    fn package_description_seeds_from_root_module_doc_verbatim() {
        let symbols: HashMap<ShortId, SymbolRecord> = [
            sym(1, "A", Language::Rust, false, false),
            sym(2, "B", Language::Rust, false, false),
        ]
        .into_iter()
        .map(|r| (r.id, r))
        .collect();
        // `lib.rs` (file 10) carries the crate doc but NO first-class symbol — the
        // real Rust shape. The symbols live in util.rs/helper.rs, so the seed must
        // find lib.rs among the package's files, not just its symbol-def files.
        let files: HashMap<ShortId, String> = [
            (10, "mono/src/lib.rs"),
            (11, "mono/src/util.rs"),
            (12, "mono/src/helper.rs"),
        ]
        .into_iter()
        .map(|(i, p)| (i, p.to_string()))
        .collect();
        let pdf: HashMap<ShortId, ShortId> = [(1, 11), (2, 12)].into_iter().collect();
        let agg: HashMap<ShortId, ShortId> = [(1, 1), (2, 2)].into_iter().collect();
        let anchors: HashMap<ShortId, (u32, String)> = [
            (1, (1u32, "mono".to_string())),
            (2, (1, "mono".to_string())),
        ]
        .into_iter()
        .collect();
        let nodes = vec![
            agg_node(1, "A", Kind::Class, 1, "mono"),
            agg_node(2, "B", Kind::Class, 1, "mono"),
        ];
        let doc = "The mono crate.\nMore detail.";
        let file_docs: HashMap<ShortId, String> = [(10u32, doc.to_string())].into_iter().collect();
        let build = |fd: &HashMap<ShortId, String>| {
            build_concepts(
                &symbols,
                &files,
                &pdf,
                &agg,
                &anchors,
                &nodes,
                &[agg_edge(1, 2, 1)],
                &[],
                &[],
                &HashMap::new(),
                fd,
                &meta(),
            )
            .0
        };
        // The root file's (`lib.rs`) module doc becomes the description, verbatim.
        // Mutation-check backing (§9): breaking the verbatim copy fails this exact
        // equality (incl. the embedded newline).
        let with_doc = build(&file_docs);
        let mono = with_doc.iter().find(|c| c.title == "mono").unwrap();
        assert_eq!(mono.description.as_deref(), Some(doc));
        // No module doc for the root file → no description (never synthesized).
        let no_doc = build(&HashMap::new());
        let mono = no_doc.iter().find(|c| c.title == "mono").unwrap();
        assert!(mono.description.is_none());
    }

    #[test]
    fn package_members_report_total_and_dir_counts_without_a_cap() {
        // A single flat package of EIGHT files (> MAX_MEMBERS = 6): the package
        // concept summarizes them as a total + per-directory counts, never the
        // old truncated top-6 file list.
        let symbols: HashMap<ShortId, SymbolRecord> = (1..=8)
            .map(|i| sym(i, &format!("S{i}"), Language::Rust, false, false))
            .map(|r| (r.id, r))
            .collect();
        let files: HashMap<ShortId, String> = (1..=8)
            .map(|i| (100 + i, format!("mono/src/f{i}.rs")))
            .collect();
        let pdf: HashMap<ShortId, ShortId> = (1..=8).map(|i| (i, 100 + i)).collect();
        let agg: HashMap<ShortId, ShortId> = (1..=8).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> =
            (1..=8).map(|i| (i, (1u32, "mono".to_string()))).collect();
        let nodes: Vec<AggregateNodeRecord> = (1..=8)
            .map(|i| agg_node(i, &format!("S{i}"), Kind::Function, 1, "mono"))
            .collect();
        let (concepts, _domains, _shape) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &[agg_edge(1, 2, 3)],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        let mono = concepts.iter().find(|c| c.title == "mono").unwrap();
        assert!(
            mono.members.is_empty(),
            "a package summarizes via dir_counts, not a file list"
        );
        // Mutation-check backing (§9): reinstating a `MAX_MEMBERS` cap would drop
        // this below 8; the flat package is not subdivided, so all files land in
        // one `src` bucket.
        assert_eq!(mono.file_count, 8);
        assert_eq!(mono.dir_counts, vec![("src".to_string(), 8)]);
        assert!(mono.components.is_empty(), "flat package → no components");
    }

    /// Ten symbols across `src/Core` (5) + `src/Features` (5) under one dominant
    /// anchor. Both sub-areas clear `MIN_SUBAREA_SYMBOLS`, and there are
    /// `MIN_SUBAREAS` of them, so the package subdivides into two `component`s.
    #[expect(
        clippy::type_complexity,
        reason = "a test fixture returning the aggregation-stage maps build_concepts takes"
    )]
    fn structured_mono() -> (
        HashMap<ShortId, SymbolRecord>,
        HashMap<ShortId, String>,
        HashMap<ShortId, ShortId>,
        HashMap<ShortId, ShortId>,
        HashMap<ShortId, (u32, String)>,
        Vec<AggregateNodeRecord>,
    ) {
        let symbols: HashMap<ShortId, SymbolRecord> = (1..=10)
            .map(|i| sym(i, &format!("S{i}"), Language::Rust, false, false))
            .map(|r| (r.id, r))
            .collect();
        let files: HashMap<ShortId, String> = (1..=10)
            .map(|i| {
                let area = if i <= 5 { "Core" } else { "Features" };
                (100 + i, format!("mono/src/{area}/s{i}.rs"))
            })
            .collect();
        let pdf: HashMap<ShortId, ShortId> = (1..=10).map(|i| (i, 100 + i)).collect();
        let agg: HashMap<ShortId, ShortId> = (1..=10).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> =
            (1..=10).map(|i| (i, (1u32, "mono".to_string()))).collect();
        let nodes: Vec<AggregateNodeRecord> = (1..=10)
            .map(|i| agg_node(i, &format!("S{i}"), Kind::Class, 1, "mono"))
            .collect();
        (symbols, files, pdf, agg, anchors, nodes)
    }

    #[test]
    fn dominant_structured_package_subdivides_into_components() {
        let (symbols, files, pdf, agg, anchors, nodes) = structured_mono();
        let (concepts, _domains, _shape) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &[agg_edge(1, 2, 3)],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        let pkg = concepts.iter().find(|c| c.title == "mono").unwrap();
        assert_eq!(
            pkg.components.len(),
            2,
            "two source sub-areas → two components"
        );
        let comp = |t: &str| concepts.iter().find(|c| c.title == t).unwrap();
        let core = comp("mono / Core");
        assert_eq!(core.concept_type, "component");
        assert_eq!(core.parent.as_deref(), Some("packages/rust_mono"));
        assert_eq!(core.resource, "mono/src/Core");
        assert_eq!(core.symbols, 5);
        assert_eq!(core.members.len(), 5, "a component lists ALL its files");
        assert!(
            core.dir_counts.is_empty(),
            "a component uses the flat file list"
        );
        // Mutation-check backing (§9): a flat package (structured_mono collapsed
        // into one dir) must NOT subdivide — covered by the flat-package test; here
        // both sub-areas are present so components exist.
        assert!(concepts.iter().any(|c| c.title == "mono / Features"));
    }

    #[test]
    fn dominant_package_with_one_qualifying_subarea_stays_flat() {
        // `src/Core` has 5 symbols (clears MIN_SUBAREA_SYMBOLS); `src/Api` has 2
        // (dropped). Only ONE sub-area qualifies — below MIN_SUBAREAS — so the
        // dominant package is NOT subdivided (a lone trivial component is noise).
        let symbols: HashMap<ShortId, SymbolRecord> = (1..=7)
            .map(|i| sym(i, &format!("S{i}"), Language::Rust, false, false))
            .map(|r| (r.id, r))
            .collect();
        let files: HashMap<ShortId, String> = (1..=7)
            .map(|i| {
                let p = if i <= 5 {
                    format!("mono/src/Core/c{i}.rs")
                } else {
                    format!("mono/src/Api/a{i}.rs")
                };
                (100 + i, p)
            })
            .collect();
        let pdf: HashMap<ShortId, ShortId> = (1..=7).map(|i| (i, 100 + i)).collect();
        let agg: HashMap<ShortId, ShortId> = (1..=7).map(|i| (i, i)).collect();
        let anchors: HashMap<ShortId, (u32, String)> =
            (1..=7).map(|i| (i, (1u32, "mono".to_string()))).collect();
        let nodes: Vec<AggregateNodeRecord> = (1..=7)
            .map(|i| agg_node(i, &format!("S{i}"), Kind::Class, 1, "mono"))
            .collect();
        let (concepts, _domains, _shape) = build_concepts(
            &symbols,
            &files,
            &pdf,
            &agg,
            &anchors,
            &nodes,
            &[agg_edge(1, 2, 3)],
            &[],
            &[],
            &HashMap::new(),
            &HashMap::new(),
            &meta(),
        );
        let mono = concepts.iter().find(|c| c.title == "mono").unwrap();
        // Mutation-check backing (§9): dropping the `< MIN_SUBAREAS` guard makes the
        // lone `Core` sub-area wrongly sprout a single component.
        assert!(
            mono.components.is_empty(),
            "one qualifying sub-area → stay flat"
        );
        assert!(!concepts.iter().any(|c| c.concept_type == "component"));
    }

    #[test]
    fn write_bundle_writes_index_concepts_and_log() {
        let f = fixture();
        let (concepts, shape) = build(&f);
        let dir = tempfile::TempDir::new().unwrap();
        let index = write_bundle(dir.path(), &shape, &concepts, &[]).unwrap();
        assert!(index.exists());
        assert!(dir.path().join("packages/rust_alpha.md").exists());
        assert!(dir.path().join("documents/docs.md").exists());
        assert!(dir.path().join("log.md").exists());
    }
}
