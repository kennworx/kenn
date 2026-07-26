//! `list_packages` — the package axis of the graph, as a query.
//!
//! The atlas renders this as markdown at index time; this answers the same
//! question on demand, without reading a file. Both go through
//! `kenn_indexer::atlas::coupling`, so a threshold or cap can never mean one
//! thing in the document and another at the prompt.

use std::collections::{BTreeSet, HashMap};

use kenn_store::api::Reader;

use kenn_indexer::atlas::coupling::{classify, couplings, Direction, PairWeights};
use kenn_indexer::atlas::domains::is_code_lang;
use kenn_indexer::atlas::model::Role;
use kenn_indexer::atlas::producer;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::error::McpError;
use crate::types::ListResponse;

use super::{internal, ServerState};

#[derive(Debug, Deserialize, Serialize, JsonSchema)]
pub struct ListPackagesArgs {
    /// Restrict to one package by exact name (as it appears in the atlas).
    /// Omit to list every package.
    #[serde(default)]
    pub package: Option<String>,
    /// Rows per response and the continuation cursor. The axis is computed
    /// whole and ordered deterministically, so this is a plain offset walk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pagination: Option<crate::types::Pagination>,
}

/// One coupled package: who, how heavily, and by which relations.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CouplingView {
    pub package: String,
    pub weight: u64,
    /// `relation=weight`, heaviest first — `implements` here marks a
    /// contract/implementer pair rather than incidental use.
    pub relations: Vec<String>,
}

/// One of a package's most-connected symbols — a resolvable handle to drill in
/// with `kenn get <id>` / `kenn list <rel> <id>`. Mirrors the atlas package
/// concept's central-symbol list.
#[derive(Debug, Serialize, JsonSchema)]
pub struct CentralSymbolView {
    /// The stable `pub_id` — feed it straight to `kenn get` / `kenn find` / `kenn
    /// list`.
    pub id: String,
    pub name: String,
    pub kind: String,
}

#[derive(Debug, Serialize, JsonSchema)]
pub struct PackageView {
    /// The package's own root symbol's `pub_id` — the crate root module (Rust:
    /// `rs:<crate>::crate`) or the root namespace (C#: `cs:<Namespace>`) — a
    /// resolvable handle to `kenn get` the package itself, shown on EVERY row.
    /// Empty when no root container resolves. ALWAYS serialized (never skipped)
    /// so every row carries the same fields and the listing stays a flat table.
    /// First column: it is the id a reader acts on.
    pub symbol: String,
    pub name: String,
    pub language: String,
    /// `provider` / `layer` / `consumer` / `tests` / `isolated` — where this
    /// package sits in the dependency graph.
    pub role: String,
    pub symbols: u64,
    /// How many packages depend on this one, and how many it depends on. These
    /// are the counts the role was derived from.
    pub used_by_count: u64,
    pub deps_count: u64,
    /// The package's root-module doc, verbatim — the only authored prose the
    /// atlas carries. Absent when the package has none; never synthesized.
    ///
    /// Populated only for a NAMED package. Its presence varies per package, so
    /// carrying it on every row would make the listing ragged and drop the whole
    /// table to JSON — the flat-row rule in design D2.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// Workspace-relative package root (the manifest's directory). Always
    /// serialized — it always resolves, falling back to the package name — so
    /// every row carries the same fields.
    pub resource: String,
    // NOT reported: `file_count` / `dir_counts`. The atlas counts the files of
    // EVERY symbol in the package, mapped through the aggregation rollup
    // (`sym_anchor` walks `aggregate_of`). A snapshot query sees only the
    // aggregate ROOTS, so counting from them undershoots (57 vs the atlas's 73
    // on this repo) and counting every def row overshoots (86). Reproducing the
    // rollup on the read path is disproportionate, and a count that disagrees
    // with the document is the exact defect this axis exists to remove — so the
    // field is omitted rather than approximated.
    /// The package's most-connected symbols, with resolvable ids to search on.
    /// Populated only when a single `package` was requested.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub central: Vec<CentralSymbolView>,
    /// Populated only when a single `package` was requested — the full listing
    /// would be quadratic and unreadable.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub used_by: Vec<CouplingView>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub deps: Vec<CouplingView>,
}

/// Ranks a package's aggregate nodes by weighted degree (in + out) and returns
/// the top `cap` `short_id`s — the same "central symbols" the atlas package
/// concept lists. Containers (module/namespace/package) are excluded (rollup
/// aggregates, not real types); test symbols are excluded unless the package is
/// test-dominant (then they ARE its API), matching the atlas rule.
#[must_use]
pub fn central_node_ids(
    nodes: &[kenn_store::AggregateNodeRow],
    edges: &[kenn_store::AggregateEdgeRow],
    want: &str,
    cap: usize,
) -> Vec<u32> {
    let mut degree: std::collections::HashMap<u32, u64> = std::collections::HashMap::new();
    for e in edges {
        *degree.entry(e.src_id).or_default() += u64::from(e.weight);
        *degree.entry(e.dst_id).or_default() += u64::from(e.weight);
    }
    let (mut total, mut tests) = (0u64, 0u64);
    for n in nodes
        .iter()
        .filter(|n| !n.external && n.anchor_name == want)
    {
        total += 1;
        if n.test {
            tests += 1;
        }
    }
    let test_dominant = tests * 2 > total;
    let mut cand: Vec<&kenn_store::AggregateNodeRow> = nodes
        .iter()
        .filter(|n| {
            !n.external
                && n.anchor_name == want
                && !n.name.is_empty()
                && !matches!(n.kind.as_str(), "module" | "namespace" | "package")
                && (test_dominant || !n.test)
        })
        .collect();
    cand.sort_by(|a, b| {
        degree
            .get(&b.id)
            .unwrap_or(&0)
            .cmp(degree.get(&a.id).unwrap_or(&0))
            .then_with(|| a.name.cmp(&b.name))
    });
    cand.into_iter().take(cap).map(|n| n.id).collect()
}

/// The `pub_id` a package's ROOT symbol WOULD have, constructed from its
/// language + anchor — the crate root (Rust: `rs:<crate>::crate`) or the root
/// namespace (C#: `cs:<Namespace>`, TS/Go/Python/Swift: `<prefix>:<anchor>`).
/// The root symbol is NOT an aggregate node (container namespaces are rolled up
/// out of the aggregate graph), so it can't be found there — but it IS in the
/// symbols table, so the handler constructs this candidate and verifies it with
/// `fetch_symbol`, only keeping it when it resolves. `None` for a language whose
/// prefix we don't know.
#[must_use]
pub fn candidate_root_pub_id(language: &str, anchor: &str) -> Option<String> {
    let (prefix, native) = match language {
        "rust" => ("rs", format!("{anchor}::crate")),
        "csharp" => ("cs", anchor.to_string()),
        "typescript" => ("ts", anchor.to_string()),
        "go" => ("go", anchor.to_string()),
        "python" => ("py", anchor.to_string()),
        "swift" => ("sw", anchor.to_string()),
        _ => return None,
    };
    Some(format!("{prefix}:{native}"))
}

/// A C# project's DEFAULT namespace: the common leading namespace of its member
/// native ids (each the `pub_id` sans `cs:` prefix, e.g. `Acme.Billing.Admin.
/// Foo`). Each member's type leaf (last dotted segment) is dropped, then the
/// longest shared run of leading segments is returned. `None` when members share
/// no root (or have no namespace part). Derived from where the types actually
/// live — so, unlike guessing from the assembly name, it's the real namespace.
#[must_use]
pub fn namespace_common_prefix(member_natives: &[&str]) -> Option<String> {
    let mut common: Option<Vec<&str>> = None;
    for native in member_natives {
        let segs: Vec<&str> = native.split('.').collect();
        // Drop the type leaf (last segment); the rest is the namespace path.
        let Some((_leaf, ns)) = segs.split_last() else {
            continue;
        };
        if ns.is_empty() {
            continue; // a bare top-level type has no namespace
        }
        common = Some(match common {
            None => ns.to_vec(),
            Some(c) => {
                let k = c.iter().zip(ns.iter()).take_while(|(a, b)| a == b).count();
                c.into_iter().take(k).collect()
            }
        });
        if common.as_ref().is_some_and(Vec::is_empty) {
            return None; // members diverge at the root — no shared namespace
        }
    }
    common.filter(|c| !c.is_empty()).map(|c| c.join("."))
}

const fn role_name(r: Role) -> &'static str {
    match r {
        Role::Provider => "provider",
        Role::Layer => "layer",
        Role::Consumer => "consumer",
        Role::Tests => "tests",
        Role::Isolated => "isolated",
    }
}

fn to_views(cs: Vec<kenn_indexer::atlas::model::Coupling>) -> Vec<CouplingView> {
    cs.into_iter()
        .map(|c| CouplingView {
            package: c.title,
            weight: c.weight,
            relations: c
                .relations
                .into_iter()
                .map(|(k, w)| format!("{k}={w}"))
                .collect(),
        })
        .collect()
}

/// The whole computation, given the graph rows — pure, so the rules below are
/// testable without a published snapshot behind them.
///
/// `want` selects a single package; when set, that package's full coupling is
/// included, otherwise only the counts (listing every package's couplings would
/// be quadratic and unreadable).
#[must_use]
pub fn package_views(
    nodes: &[kenn_store::AggregateNodeRow],
    edges: &[kenn_store::AggregateEdgeRow],
    want: Option<&str>,
) -> Vec<PackageView> {
    // Code languages only, matching the atlas producer's own anchor map. A
    // content language (markdown, html, css) has headings for "symbols", so an
    // anchor holding nothing else is a DOCUMENT directory, not a code package —
    // the atlas routes it to the documents axis. Without this filter the query
    // listed those anchors as packages and reported more packages than the atlas:
    // 4 vs 3 on one repo, 128 vs 125 on a 125-package solution, where the extras
    // held only markdown, html or css nodes.
    //
    // The same map drives the coupling pair weights below, so filtering here also
    // stops a markdown link from contributing to package coupling — which is
    // again what the producer does.
    let anchor: std::collections::HashMap<u32, &str> = nodes
        .iter()
        .filter(|n| !n.external && n.anchor_name != "<unanchored>" && is_code_lang(&n.language))
        .map(|n| (n.id, n.anchor_name.as_str()))
        .collect();

    let mut pair_w: PairWeights<'_> = PairWeights::default();
    for e in edges {
        if let (Some(&a), Some(&b)) = (anchor.get(&e.src_id), anchor.get(&e.dst_id)) {
            if a != b {
                *pair_w
                    .entry((a, b))
                    .or_default()
                    .entry(e.kind.db_name())
                    .or_default() += u64::from(e.weight);
            }
        }
    }

    // Per package: symbol count, test-dominance, plurality language.
    // Test-dominance matches the atlas — more test nodes than not.
    let mut syms: std::collections::HashMap<&str, (u64, u64)> = std::collections::HashMap::new();
    let mut langs: std::collections::HashMap<&str, std::collections::HashMap<&str, u64>> =
        std::collections::HashMap::new();
    for n in nodes.iter().filter(|n| anchor.contains_key(&n.id)) {
        let e = syms.entry(n.anchor_name.as_str()).or_default();
        e.0 += 1;
        if n.test {
            e.1 += 1;
        }
        *langs
            .entry(n.anchor_name.as_str())
            .or_default()
            .entry(n.language.as_str())
            .or_default() += 1;
    }

    // `anchor_lang` feeds concept-id construction inside `couplings`;
    // the ids are unused here but the shared fn owns that shape.
    let anchor_lang: std::collections::HashMap<&str, String> = langs
        .iter()
        .map(|(&a, m)| {
            let l = m
                .iter()
                .max_by(|x, y| x.1.cmp(y.1).then_with(|| y.0.cmp(x.0)))
                .map_or(String::new(), |(l, _)| (*l).to_string());
            (a, l)
        })
        .collect();

    let mut items: Vec<PackageView> = syms
        .iter()
        .filter(|(name, _)| want.is_none_or(|w| w == **name))
        .map(|(&name, &(total, tests))| {
            let deps = couplings(&pair_w, &anchor_lang, name, Direction::Out);
            let used_by = couplings(&pair_w, &anchor_lang, name, Direction::In);
            // Pre-cap weights: the render cap must not move a package between
            // roles, or the printed role contradicts the counts beside it.
            let role = classify(tests * 2 > total, used_by.weight, deps.weight);
            let (deps, deps_total) = (deps.rows, deps.total);
            let (used_by, used_by_total) = (used_by.rows, used_by.total);
            // Detail only for a single named package.
            let detail = want.is_some();
            PackageView {
                name: name.to_string(),
                language: anchor_lang.get(name).cloned().unwrap_or_default(),
                role: role_name(role).to_string(),
                symbols: total,
                used_by_count: used_by_total,
                deps_count: deps_total,
                // `symbol` and `central` need the reader (pub_id resolution); the
                // handler fills them after this pure pass.
                symbol: String::new(),
                central: Vec::new(),
                description: None,
                resource: String::new(),
                used_by: if detail {
                    to_views(used_by)
                } else {
                    Vec::new()
                },
                deps: if detail { to_views(deps) } else { Vec::new() },
            }
        })
        .collect();

    // Most-depended-on first: on a 125-package solution the ones
    // everything rests on must lead, not whichever sorts first.
    items.sort_by(|a, b| {
        b.used_by_count
            .cmp(&a.used_by_count)
            .then_with(|| a.name.cmp(&b.name))
    });
    items
}

/// The files each package's symbols are defined in, keyed by package.
///
/// Applies the SAME filters the producer's `sym_anchor` does — code languages
/// only, containers excluded — and takes each symbol's PRIMARY def file (the
/// smallest file id), as the producer does. Without the filters a README under
/// `crates/` counts as one of the package's files; without the primary-file rule
/// a partial class declared across three files counts three times.
///
/// Pure, so the rules are testable without a published snapshot behind them.
#[must_use]
pub fn package_def_files<'a>(
    nodes: &'a [kenn_store::AggregateNodeRow],
    def_files: &[(u32, u32)],
    path_of: &HashMap<u32, &'a str>,
) -> HashMap<&'a str, BTreeSet<&'a str>> {
    let sym_pkg: HashMap<u32, &str> = nodes
        .iter()
        .filter(|n| {
            !n.external
                && n.anchor_name != "<unanchored>"
                && is_code_lang(&n.language)
                && !matches!(n.kind.as_str(), "module" | "namespace" | "package")
        })
        .map(|n| (n.id, n.anchor_name.as_str()))
        .collect();

    let mut primary: HashMap<u32, u32> = HashMap::new();
    for (sym, file) in def_files {
        primary
            .entry(*sym)
            .and_modify(|f| {
                if file < f {
                    *f = *file;
                }
            })
            .or_insert(*file);
    }

    let mut out: HashMap<&str, BTreeSet<&str>> = HashMap::new();
    for (sym, file) in &primary {
        if let (Some(&pkg), Some(&path)) = (sym_pkg.get(sym), path_of.get(file)) {
            out.entry(pkg).or_default().insert(path);
        }
    }
    out
}

/// A package's root directory and (when asked) its root-module doc.
///
/// `paths` are the package's def files; `files` is every indexed path, because
/// the root doc is picked from ALL files under the root — a Rust `lib.rs` holds
/// the crate `//!` doc yet defines no first-class symbol. Both the root and the
/// doc come from the producer's own helpers, never reimplemented here.
///
/// Pure, so the rules are testable without a published snapshot.
#[must_use]
pub fn package_root_and_doc(
    paths: &BTreeSet<&str>,
    files: &[(u32, &str)],
    docs: &HashMap<u32, &str>,
    language: &str,
    fallback_name: &str,
    want_doc: bool,
) -> (String, Option<String>) {
    let list: Vec<&str> = paths.iter().copied().collect();
    let root = producer::package_root(&list);
    let resource = if root.is_empty() {
        fallback_name.to_string()
    } else {
        root
    };
    if !want_doc {
        return (resource, None);
    }
    let prefix = format!("{resource}/");
    let candidates: Vec<(u32, &str)> = files
        .iter()
        .filter(|(_, p)| p.starts_with(&prefix))
        .copied()
        .collect();
    let doc = producer::pick_root_file(&candidates, language)
        .and_then(|fid| docs.get(&fid))
        .filter(|d| !d.is_empty())
        .map(|d| (*d).to_string());
    (resource, doc)
}

/// Fill every package's root directory and (when `detail`) its root-module doc.
///
/// Pure: takes the three scans' rows, so the whole rule set is testable without
/// a published snapshot. The async shell below only fetches.
pub fn fill_package_metadata(
    files: &[kenn_store::FileRow],
    def_files: &[(u32, u32)],
    file_docs: &[(u32, String)],
    nodes: &[kenn_store::AggregateNodeRow],
    items: &mut [PackageView],
    detail: bool,
) {
    let path_of: HashMap<u32, &str> = files.iter().map(|f| (f.id, f.path.as_str())).collect();
    let docs: HashMap<u32, &str> = file_docs.iter().map(|(id, d)| (*id, d.as_str())).collect();
    let all: Vec<(u32, &str)> = files.iter().map(|f| (f.id, f.path.as_str())).collect();
    let pkg_files = package_def_files(nodes, def_files, &path_of);

    for pv in items.iter_mut() {
        if let Some(paths) = pkg_files.get(pv.name.as_str()) {
            // `detail` gates the doc: its presence varies per package, so
            // carrying it on every row makes the listing ragged and drops the
            // whole table to JSON (design D2).
            let (resource, doc) =
                package_root_and_doc(paths, &all, &docs, &pv.language, &pv.name, detail);
            pv.resource = resource;
            pv.description = doc;
        }
    }
}

/// Fetch the three scans [`fill_package_metadata`] needs. I/O only.
async fn attach_package_metadata(
    h: &super::state::ReadyView,
    nodes: &[kenn_store::AggregateNodeRow],
    items: &mut [PackageView],
    detail: bool,
) -> Result<(), McpError> {
    let files = h.read.scan_files().await.map_err(internal)?;
    let def_files = h.read.scan_def_files().await.map_err(internal)?;
    let file_docs = h.read.scan_file_docs().await.map_err(internal)?;
    fill_package_metadata(&files, &def_files, &file_docs, nodes, items, detail);
    Ok(())
}

/// List packages with their role and coupling counts, or one package with its
/// full typed coupling in both directions.
///
/// Reads the published snapshot's aggregate graph rather than re-projecting
/// per-symbol edges: that graph is already rolled up and weighted, and it is
/// the same input the atlas builds from.
pub async fn list_packages(
    state: &ServerState,
    args: &ListPackagesArgs,
) -> Result<ListResponse<PackageView>, McpError> {
    let want = args.package.clone();
    let args_pagination = args.pagination.clone();
    state
        .with_db(|h| async move {
            let nodes = h.read.scan_aggregate_nodes().await.map_err(internal)?;
            let edges = h.read.scan_aggregate_edges().await.map_err(internal)?;

            // Anchor per node id, internal + anchored only. External nodes
            // are vendored dependencies, not packages of this workspace.
            let mut items = package_views(&nodes, &edges, want.as_deref());

            // Page BEFORE the per-item enrichment below. Each row costs a
            // `fetch_symbol` to verify its root pub_id, plus a possible symbols
            // scan for the C# namespace fallback and a def/doc scan for metadata;
            // doing that for 125 packages to return 20 is waste the caller never
            // asked for. `package_views` already orders deterministically
            // (most-depended-on first), which is what makes an offset walk sound.
            let (mut items, next) = super::support::page_axis_items(
                std::mem::take(&mut items),
                args_pagination.as_ref(),
                h.snapshot_id,
            )?;

            // Every row carries `symbol` — the package's own root symbol's pub_id
            // — so the bare overview is actionable (`kenn get <symbol>`) without a
            // second command. Constructed from language + anchor, then verified
            // against the symbols table so we never print an id that won't resolve.
            for pv in &mut items {
                if let Some(cand) = candidate_root_pub_id(&pv.language, &pv.name) {
                    if h.read
                        .fetch_symbol(&pv.language, &cand)
                        .await
                        .map_err(internal)?
                        .is_some()
                    {
                        pv.symbol = cand;
                    }
                }
            }

            // C# fallback: a project often declares into a namespace whose name
            // differs from the assembly (`Acme.Billing.Data` the *.csproj vs the
            // `Acme.Billing` namespace its types live in), and that namespace is
            // NOT constructible from the anchor. Derive the project's DEFAULT
            // namespace from where its types actually live — the common namespace
            // of its members — and use it when it resolves. One symbols scan,
            // taken only when a C# package still lacks a symbol.
            if items
                .iter()
                .any(|p| p.symbol.is_empty() && p.language == "csharp")
            {
                let syms = h.read.scan_symbols().await.map_err(internal)?;
                let native_of: std::collections::HashMap<u32, &str> = syms
                    .iter()
                    .filter_map(|s| s.pub_id.split_once(':').map(|(_, n)| (s.id, n)))
                    .collect();
                let namespaces: std::collections::HashSet<&str> = syms
                    .iter()
                    .filter(|s| s.kind == "namespace")
                    .map(|s| s.pub_id.as_str())
                    .collect();
                for pv in items
                    .iter_mut()
                    .filter(|p| p.symbol.is_empty() && p.language == "csharp")
                {
                    let member_natives: Vec<&str> = nodes
                        .iter()
                        .filter(|n| {
                            !n.external
                                && n.anchor_name == pv.name
                                && !matches!(n.kind.as_str(), "module" | "namespace" | "package")
                        })
                        .filter_map(|n| native_of.get(&n.id).copied())
                        .collect();
                    if let Some(ns) = namespace_common_prefix(&member_natives) {
                        let cand = format!("cs:{ns}");
                        if namespaces.contains(cand.as_str()) {
                            pv.symbol = cand;
                        }
                    }
                }
            }

            attach_package_metadata(&h, &nodes, &mut items, want.is_some()).await?;

            // For a single named package, attach its full central-symbol list with
            // resolvable ids — the reader turns each ranked node into a pub_id so
            // the caller can `kenn get` / `kenn list` it straight away.
            if let Some(w) = want.as_deref() {
                let ids = central_node_ids(&nodes, &edges, w, MAX_CENTRAL);
                let mut central = Vec::with_capacity(ids.len());
                for id in ids {
                    if let Some(sym) = h
                        .read
                        .fetch_symbol_by_short_id(id)
                        .await
                        .map_err(internal)?
                    {
                        central.push(CentralSymbolView {
                            id: sym.pub_id,
                            name: sym.name,
                            kind: sym.kind,
                        });
                    }
                }
                if let Some(pv) = items.iter_mut().find(|p| p.name == w) {
                    pv.central = central;
                }
            }

            Ok(ListResponse { items, next })
        })
        .await
}

/// Central symbols shown for a single named package — matches the atlas cap.
const MAX_CENTRAL: usize = 8;

#[cfg(test)]
mod tests {
    use super::*;
    use kenn_model::EdgeKind;
    use kenn_store::{AggregateEdgeRow, AggregateNodeRow};

    fn node(id: u32, anchor: &str, external: bool, test: bool) -> AggregateNodeRow {
        AggregateNodeRow {
            id,
            kind: "class".into(),
            name: format!("N{id}"),
            language: "rust".into(),
            external,
            test,
            example: false,
            anchor_id: 0,
            anchor_name: anchor.into(),
        }
    }
    /// An anchor holding only CONTENT-language nodes is a document directory, not
    /// a code package. The atlas routes it to the documents axis; this query used
    /// to list it as a package and so reported more packages than the atlas —
    /// 4 vs 3 on one repo, 128 vs 125 on a 125-package solution, the extras
    /// holding only markdown, html or css.
    ///
    /// Mutation-checked: dropping `is_code_lang` from the `anchor` filter in
    /// `package_views` makes `docs` a package and this assertion sees 2.
    #[test]
    fn a_content_only_anchor_is_not_a_package() {
        let content = |id: u32, anchor: &str, lang: &str| AggregateNodeRow {
            language: lang.into(),
            kind: "document".into(),
            ..node(id, anchor, false, false)
        };
        let nodes = vec![
            // A real code package.
            node(1, "app", false, false),
            node(2, "app", false, false),
            // Anchors that hold nothing but content — one per content language.
            content(10, "docs", "markdown"),
            content(11, "site", "html"),
            content(12, "styles", "css"),
        ];
        let edges = vec![edge(1, 2, EdgeKind::Calls, 5)];
        let got = package_views(&nodes, &edges, None);
        let names: Vec<&str> = got.iter().map(|p| p.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["app"],
            "only the code anchor is a package; content dirs are the documents axis"
        );
    }

    fn edge(src: u32, dst: u32, kind: EdgeKind, weight: u32) -> AggregateEdgeRow {
        AggregateEdgeRow {
            src_id: src,
            dst_id: dst,
            kind,
            weight,
        }
    }

    /// Central symbols are the package's aggregate nodes ranked by weighted
    /// degree, with containers (module/namespace/package) and — in a
    /// production-dominant package — test symbols excluded. Mutation-checked:
    /// dropping the container filter surfaces `Mod` (degree 500); dropping the
    /// test filter surfaces the test node.
    #[test]
    fn central_node_ids_ranks_by_degree_excluding_containers_and_tests() {
        let mut nodes = vec![
            node(1, "p", false, false),
            node(2, "p", false, false),
            node(4, "p", false, true), // a test class in a production package
            node(5, "other", false, false), // another package
        ];
        nodes.push(AggregateNodeRow {
            id: 3,
            kind: "module".into(), // the crate-root container — never central
            name: "crate".into(),
            language: "rust".into(),
            external: false,
            test: false,
            example: false,
            anchor_id: 0,
            anchor_name: "p".into(),
        });
        let edges = vec![
            edge(1, 2, EdgeKind::Calls, 100), // N1 +100, N2 +100
            edge(1, 3, EdgeKind::Calls, 500), // N1 +500, crate +500 (excluded)
            edge(4, 1, EdgeKind::Calls, 50),  // N1 +50, test N4 +50 (excluded)
        ];
        let ids = central_node_ids(&nodes, &edges, "p", 8);
        assert_eq!(
            ids,
            vec![1, 2],
            "N1 (deg 650) then N2 (100); the crate module is a container, N4 is a test symbol, N5 is another package"
        );
    }

    /// The overview's per-package `symbol` is the package's own ROOT — the crate
    /// root (Rust) or root namespace (C#) — constructed from language + anchor,
    /// not a ranked member. The handler verifies it against the symbols table
    /// before printing; this covers the construction.
    #[test]
    fn candidate_root_pub_id_is_the_package_root() {
        assert_eq!(
            candidate_root_pub_id("rust", "kenn-store").as_deref(),
            Some("rs:kenn-store::crate")
        );
        assert_eq!(
            candidate_root_pub_id("csharp", "Acme.Util").as_deref(),
            Some("cs:Acme.Util")
        );
        assert_eq!(candidate_root_pub_id("brainfuck", "x"), None);
    }

    /// A C# project with no own-name namespace falls back to its DEFAULT
    /// namespace — the common root of where its types live — even across
    /// sub-namespaces. Each member's type leaf is dropped first.
    #[test]
    fn namespace_common_prefix_is_the_default_namespace() {
        let members = [
            "Acme.Billing.AccessExtensions",
            "Acme.Billing.Admin.Pages.LoginModel",
            "Acme.Billing.Feed.Event",
        ];
        assert_eq!(
            namespace_common_prefix(&members).as_deref(),
            Some("Acme.Billing")
        );
        // A single member: drop its type leaf.
        assert_eq!(
            namespace_common_prefix(&["Foo.Bar.Baz"]).as_deref(),
            Some("Foo.Bar")
        );
        // No shared root → no default namespace.
        assert_eq!(namespace_common_prefix(&["A.X", "B.Y"]), None);
    }

    /// The listing answers "which packages does everything rest on" — so the
    /// most-depended-on must lead, and roles must match what the atlas would
    /// render for the same graph (both go through `atlas::coupling`).
    #[test]
    fn lists_packages_most_depended_on_first() {
        let nodes = vec![
            node(1, "core", false, false),
            node(2, "app", false, false),
            node(3, "util", false, false),
        ];
        let edges = vec![
            edge(2, 1, EdgeKind::Calls, 100),  // app → core
            edge(3, 1, EdgeKind::TypeUse, 10), // util → core
            edge(2, 3, EdgeKind::Calls, 5),    // app → util
        ];
        let v = package_views(&nodes, &edges, None);
        assert_eq!(
            v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["core", "util", "app"],
            "two dependents, then one, then none"
        );
        assert_eq!(v[0].role, "provider");
        assert_eq!(v[2].role, "consumer");
        // The listing carries counts only — full coupling would be quadratic.
        assert!(v[0].used_by.is_empty(), "no detail without a named package");
    }

    /// Naming a package adds its coupling, in both directions, with relations.
    #[test]
    fn a_named_package_carries_its_coupling() {
        let nodes = vec![node(1, "core", false, false), node(2, "app", false, false)];
        let edges = vec![
            edge(2, 1, EdgeKind::Calls, 7),
            edge(2, 1, EdgeKind::Implements, 3),
        ];
        let v = package_views(&nodes, &edges, Some("core"));
        assert_eq!(v.len(), 1, "filtered to the named package");
        assert_eq!(v[0].used_by.len(), 1);
        assert_eq!(v[0].used_by[0].package, "app");
        assert_eq!(v[0].used_by[0].weight, 10);
        assert_eq!(
            v[0].used_by[0].relations,
            vec!["calls=7".to_string(), "implements=3".to_string()],
            "heaviest relation first"
        );
    }

    /// External nodes are vendored dependencies, not packages of this
    /// workspace, and `<unanchored>` is the sentinel for symbols that resolved
    /// to no package at all. Counting either would invent packages the repo
    /// does not have. Mutation-checked: dropping the filter yields 3 entries.
    #[test]
    fn external_and_unanchored_nodes_are_not_packages() {
        let nodes = vec![
            node(1, "core", false, false),
            node(2, "some-crate", true, false),
            node(3, "<unanchored>", false, false),
        ];
        let v = package_views(&nodes, &[], None);
        assert_eq!(
            v.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(),
            vec!["core"]
        );
        assert_eq!(v[0].role, "isolated", "no coupling either way");
    }

    /// Test-dominance is a MAJORITY of the package's nodes, matching the atlas
    /// rule (more test nodes than production). Mutation-checked: `>=` instead
    /// of `>` classifies an evenly-split package as tests.
    #[test]
    fn test_dominance_needs_a_majority() {
        let mostly = vec![
            node(1, "p", false, true),
            node(2, "p", false, true),
            node(3, "p", false, false),
        ];
        assert_eq!(package_views(&mostly, &[], None)[0].role, "tests");
        let even = vec![node(1, "p", false, true), node(2, "p", false, false)];
        assert_ne!(
            package_views(&even, &[], None)[0].role,
            "tests",
            "a 50/50 split is not test-dominant"
        );
    }

    /// The render cap must not move a package between roles, or the printed
    /// `role` contradicts the `used_by_count` printed beside it. 30 dependents
    /// (weight 3 each = 90 in) and one heavy dependency (25 out): the honest
    /// instability is 25/115 = 0.22 → provider. Summing only the 24 RENDERED
    /// dependents sees 72 in, so 25/97 = 0.26 → layer.
    ///
    /// Mutation-checked: classifying from `used_by.rows` instead of
    /// `used_by.weight` in `package_views` turns this row into `layer`.
    #[test]
    fn a_capped_dependent_list_does_not_change_the_role() {
        let mut nodes = vec![node(1, "core", false, false)];
        let mut edges = Vec::new();
        for i in 0..30u32 {
            let id = 100 + i;
            let anchor: &'static str = Box::leak(format!("dep{i:02}").into_boxed_str());
            nodes.push(node(id, anchor, false, false));
            // dependent → core, one call edge of weight 3
            edges.push(edge(id, 1, EdgeKind::Calls, 3));
        }
        nodes.push(node(2, "util", false, false));
        edges.push(edge(1, 2, EdgeKind::Calls, 25));

        let views = package_views(&nodes, &edges, Some("core"));
        let core = views.iter().find(|v| v.name == "core").expect("core");
        assert_eq!(
            core.used_by_count, 30,
            "the honest pre-cap count is printed"
        );
        assert_eq!(
            core.role, "provider",
            "30 dependents vs one dependency is a foundation; the 24-row cap must not demote it"
        );
    }

    /// A package's files are its symbols' PRIMARY def files, filtered the way
    /// the producer filters: code languages only, containers excluded. Each rule
    /// was a real divergence while writing this — counting every def row
    /// overshot the atlas (86 vs 73), and dropping the language filter counted a
    /// README under `crates/` as one of the package's files.
    ///
    /// Mutation-checked per rule (reverse-edit, never `git checkout`): removing
    /// `is_code_lang` admits the markdown file, taking every def row instead of
    /// the primary admits `b_alt.rs`, and dropping the container filter admits
    /// the module's own file.
    #[test]
    fn package_files_are_primary_defs_of_code_symbols() {
        let nodes = vec![
            node(1, "core", false, false),
            node(2, "core", false, false),
            {
                let mut n = node(3, "core", false, false);
                n.language = "markdown".into(); // a doc under crates/core
                n
            },
            {
                let mut n = node(4, "core", false, false);
                n.kind = "module".into(); // a container, not a real type
                n
            },
        ];
        // Symbol 2 is a partial type declared in two files: only the primary
        // (smallest file id) counts.
        let def_files = vec![(1u32, 10u32), (2, 11), (2, 12), (3, 20), (4, 30)];
        let path_of: HashMap<u32, &str> = [
            (10, "crates/core/src/a.rs"),
            (11, "crates/core/src/b.rs"),
            (12, "crates/core/src/b_alt.rs"),
            (20, "crates/core/README.md"),
            (30, "crates/core/src/mod.rs"),
        ]
        .into_iter()
        .collect();

        let got = package_def_files(&nodes, &def_files, &path_of);
        let core: Vec<&str> = got["core"].iter().copied().collect();
        assert_eq!(
            core,
            vec!["crates/core/src/a.rs", "crates/core/src/b.rs"],
            "primary defs of code, non-container symbols only"
        );
    }

    /// The root is the package's common directory; the doc is its root file's
    /// module doc, VERBATIM, and only when asked. A package with no doc reports
    /// none rather than a synthesized one — the atlas rule.
    #[test]
    fn root_and_doc_come_from_the_package_root() {
        let paths: BTreeSet<&str> = ["crates/core/src/a.rs", "crates/core/src/b.rs"]
            .into_iter()
            .collect();
        let files = vec![
            (1u32, "crates/core/src/lib.rs"),
            (2, "crates/core/src/a.rs"),
            (3, "elsewhere/x.rs"),
        ];
        let docs: HashMap<u32, &str> = [(1u32, "The core crate.")].into_iter().collect();

        let (root, doc) = package_root_and_doc(&paths, &files, &docs, "rust", "core", true);
        assert_eq!(
            root, "crates/core",
            "the common directory minus the src wrapper"
        );
        assert_eq!(
            doc.as_deref(),
            Some("The core crate."),
            "the root file's module doc, verbatim"
        );

        // A bare listing asks for no doc, so none is resolved.
        let (_, none) = package_root_and_doc(&paths, &files, &docs, "rust", "core", false);
        assert!(none.is_none(), "the doc is detail, not a listing column");
    }

    /// An undocumented package omits the field — nothing is invented.
    #[test]
    fn an_undocumented_package_reports_no_description() {
        let paths: BTreeSet<&str> = ["crates/bare/src/a.rs"].into_iter().collect();
        let files = vec![(1u32, "crates/bare/src/lib.rs")];
        let (_, doc) = package_root_and_doc(&paths, &files, &HashMap::new(), "rust", "bare", true);
        assert!(doc.is_none(), "no doc in the store means no description");
    }
}
